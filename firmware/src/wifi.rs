//! Wi-Fi for the transfer screen: joins the network from `WIFI.TXT` and
//! hands browser connections to the core's `App::serve`.
//!
//! esp-radio runs on the esp-rtos scheduler, which can't be stopped once
//! started and doesn't mix with the light sleep in `buttons`. So Wi-Fi gets
//! a boot of its own (see `main`): the board restarts into it, starts the
//! scheduler and the radio before the reader allocates anything (the radio
//! needs internal RAM), never light-sleeps, and restarts again to leave.
//!
//! The network stack is smoltcp, polled from the main loop: esp-radio's
//! driver is made for embassy-net, but its receive and transmit calls only
//! use the async context to wake a task, so a no-op waker does.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use embassy_net_driver::Driver;
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_hal::time::{Duration, Instant};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ConnectionError, ControllerConfig, DisconnectReason, Interface, PowerSaveMode,
    WifiController,
};
use inkreader_core::http::Connection;
use inkreader_core::ui::Wifi;
use inkreader_core::Error;
use log::{info, warn};
use smoltcp::iface::{Config as IfaceConfig, Interface as Iface, SocketHandle, SocketSet};
use smoltcp::socket::{dhcpv4, tcp};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address};

use crate::psram;

/// How long joining the network may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A browser connection that sends or takes nothing for this long is
/// dropped.
const STALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Sockets listening for browsers: one serves while the others take the
/// browser's next connections.
const LISTENERS: usize = 3;
const RX_BUFFER: usize = 32 * 1024;
const TX_BUFFER: usize = 8 * 1024;

/// Lets the Wi-Fi threads and the idle task run.
pub fn pause(ms: u64) {
    esp_rtos::CurrentThreadHandle::get().delay(Duration::from_millis(ms));
}

/// Runs a future on this thread, polling it every millisecond, until it
/// finishes or `timeout` passes.
fn block_on<F: Future>(fut: F, timeout: Duration) -> Option<F::Output> {
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    let deadline = Instant::now() + timeout;
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        pause(1);
    }
}

fn now() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(Instant::now().duration_since_epoch().as_micros() as i64)
}

/// esp-radio's station interface as a smoltcp device.
struct Device(Interface);
struct Rx<T>(T);
struct Tx<T>(T);

impl smoltcp::phy::Device for Device {
    type RxToken<'a> = Rx<<Interface as Driver>::RxToken<'a>>;
    type TxToken<'a> = Tx<<Interface as Driver>::TxToken<'a>>;

    fn receive(&mut self, _: smoltcp::time::Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut cx = Context::from_waker(Waker::noop());
        Driver::receive(&mut self.0, &mut cx).map(|(r, t)| (Rx(r), Tx(t)))
    }

    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        let mut cx = Context::from_waker(Waker::noop());
        Driver::transmit(&mut self.0, &mut cx).map(Tx)
    }

    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities {
        let c = Driver::capabilities(&self.0);
        let mut caps = smoltcp::phy::DeviceCapabilities::default();
        caps.medium = smoltcp::phy::Medium::Ethernet;
        caps.max_transmission_unit = c.max_transmission_unit;
        caps.max_burst_size = c.max_burst_size;
        caps
    }
}

impl<T: embassy_net_driver::RxToken> smoltcp::phy::RxToken for Rx<T> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        self.0.consume(|buf| f(buf))
    }
}

impl<T: embassy_net_driver::TxToken> smoltcp::phy::TxToken for Tx<T> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        self.0.consume(len, f)
    }
}

/// The radio, the network stack and the sockets.
pub struct Net {
    controller: WifiController<'static>,
    device: Device,
    iface: Iface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    listeners: [SocketHandle; LISTENERS],
    /// From DHCP, once there is one.
    pub address: Option<Ipv4Address>,
}

impl Net {
    /// Starts the radio for the network in `wifi`. The esp-rtos scheduler
    /// must be running.
    pub fn new(device: WIFI<'static>, wifi: &Wifi) -> Result<Net, String> {
        let ssid = wifi.ssid.as_str().try_into().map_err(|_| String::from("the network name is too long"))?;
        let auth = if wifi.password.is_empty() {
            AuthenticationMethodConfig::Open
        } else {
            let password = wifi.password.as_str().try_into().map_err(|_| String::from("the password is too long"))?;
            AuthenticationMethodConfig::Wpa2Personal(password)
        };
        let config = Config::Station(StationConfig::default().with_ssid(ssid).with_authentication(auth));
        // Fewer buffers than the defaults: internal RAM is short.
        let mut controller = WifiController::new(
            device,
            ControllerConfig::default()
                .with_initial_config(config)
                .with_static_rx_buf_num(6)
                .with_dynamic_rx_buf_num(16)
                .with_dynamic_tx_buf_num(12)
                .with_ampdu_tx_enable(false),
        )
        .map_err(|e| format!("Wi-Fi didn't start ({:?})", e))?;
        // Power saving makes the radio doze between beacons, which slows
        // DHCP and transfers; the reader is only in Wi-Fi mode briefly.
        if let Err(e) = controller.set_power_saving(PowerSaveMode::None) {
            warn!("can't turn Wi-Fi power saving off: {:?}", e);
        }
        let mut device = Device(Interface::station());
        let rng = Rng::new();
        let mut config = IfaceConfig::new(HardwareAddress::Ethernet(EthernetAddress(device.0.mac_address())));
        config.random_seed = (rng.random() as u64) << 32 | rng.random() as u64;
        let iface = Iface::new(config, &mut device, now());
        let mut sockets = SocketSet::new(vec![]);
        let mut dhcp = dhcpv4::Socket::new();
        // Ask again every 2 s rather than 10 if the router doesn't answer.
        let mut retry = dhcpv4::RetryConfig::default();
        retry.discover_timeout = smoltcp::time::Duration::from_secs(2);
        dhcp.set_retry_config(retry);
        let dhcp = sockets.add(dhcp);
        let listeners = core::array::from_fn(|_| {
            let rx = tcp::SocketBuffer::new(psram::slice(RX_BUFFER, || 0u8));
            let tx = tcp::SocketBuffer::new(psram::slice(TX_BUFFER, || 0u8));
            sockets.add(tcp::Socket::new(rx, tx))
        });
        Ok(Net { controller, device, iface, sockets, dhcp, listeners, address: None })
    }

    /// Joins the network, and asks for an address again. On failure, says
    /// why, for the screen.
    pub fn connect(&mut self) -> Result<(), String> {
        self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).reset();
        if self.controller.is_connected() {
            return Ok(());
        }
        let t = Instant::now();
        match block_on(self.controller.connect_async(), CONNECT_TIMEOUT) {
            Some(Ok(_)) => {
                info!("Wi-Fi joined in {} ms", t.elapsed().as_millis());
                Ok(())
            }
            Some(Err(ConnectionError::Failed(info))) => {
                warn!("Wi-Fi: {:?}", info.reason);
                Err(String::from(match info.reason {
                    DisconnectReason::NoAccessPointFound
                    | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
                    | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
                    | DisconnectReason::NoAccessPointFoundInRssiThreshold => "the network wasn't found",
                    DisconnectReason::AuthenticationFailed
                    | DisconnectReason::FourWayHandshakeTimeout
                    | DisconnectReason::HandshakeTimeout => "the password was refused",
                    _ => "the network didn't accept the reader",
                }))
            }
            Some(Err(e)) => Err(format!("Wi-Fi error ({:?})", e)),
            None => Err(String::from("it took too long")),
        }
    }

    /// Moves the network stack on: sends and receives, takes an address from
    /// DHCP, and listens again on closed sockets. Returns true if the
    /// address changed.
    pub fn poll(&mut self) -> bool {
        self.iface.poll(now(), &mut self.device, &mut self.sockets);
        let mut changed = false;
        match self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll() {
            Some(dhcpv4::Event::Configured(c)) => {
                info!("DHCP: {}", c.address);
                self.iface.update_ip_addrs(|a| {
                    a.clear();
                    let _ = a.push(IpCidr::Ipv4(c.address));
                });
                if let Some(router) = c.router {
                    let _ = self.iface.routes_mut().add_default_ipv4_route(router);
                }
                self.address = Some(c.address.address());
                changed = true;
            }
            // The socket also says this once when it starts.
            Some(dhcpv4::Event::Deconfigured) if self.address.is_some() => {
                self.iface.update_ip_addrs(|a| a.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.address = None;
                changed = true;
            }
            _ => {}
        }
        for &h in &self.listeners {
            let s = self.sockets.get_mut::<tcp::Socket>(h);
            if !s.is_open() {
                let _ = s.listen(80);
            }
        }
        changed
    }

    /// A socket with a browser's request waiting, if there is one.
    pub fn request(&mut self) -> Option<SocketHandle> {
        self.listeners.iter().copied().find(|&h| self.sockets.get::<tcp::Socket>(h).can_recv())
    }

    /// The connection on socket `h`, for `App::serve`.
    pub fn connection(&mut self, h: SocketHandle) -> Conn<'_> {
        Conn { net: self, socket: h, received: 0 }
    }

    /// Ends the connection on socket `h`, once what was written is sent.
    pub fn close(&mut self, h: SocketHandle) {
        self.sockets.get_mut::<tcp::Socket>(h).close();
    }

    /// Milliseconds smoltcp can wait before it next needs polling, at most
    /// `max`.
    pub fn idle_ms(&mut self, max: u64) -> u64 {
        self.iface.poll_delay(now(), &self.sockets).map_or(max, |d| d.total_millis().min(max))
    }
}

/// A browser connection on one of `Net`'s sockets.
pub struct Conn<'a> {
    net: &'a mut Net,
    socket: SocketHandle,
    /// Bytes read so far.
    pub received: usize,
}

impl Conn<'_> {
    fn socket(&mut self) -> &mut tcp::Socket<'static> {
        self.net.sockets.get_mut::<tcp::Socket>(self.socket)
    }
}

impl Connection for Conn<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let deadline = Instant::now() + STALL_TIMEOUT;
        loop {
            let s = self.socket();
            if s.can_recv() {
                let n = s.recv_slice(buf).map_err(|_| Error::Io)?;
                self.received += n;
                return Ok(n);
            }
            if !s.may_recv() {
                return Ok(0);
            }
            if Instant::now() >= deadline {
                return Err(Error::Io);
            }
            self.net.poll();
            if !self.socket().can_recv() {
                pause(1);
            }
        }
    }

    fn write(&mut self, mut data: &[u8]) -> Result<(), Error> {
        let mut deadline = Instant::now() + STALL_TIMEOUT;
        while !data.is_empty() {
            let s = self.socket();
            if !s.may_send() {
                return Err(Error::Io);
            }
            if s.can_send() {
                let n = s.send_slice(data).map_err(|_| Error::Io)?;
                data = &data[n..];
                deadline = Instant::now() + STALL_TIMEOUT;
            } else if Instant::now() >= deadline {
                return Err(Error::Io);
            }
            self.net.poll();
            if !data.is_empty() && !self.socket().can_send() {
                pause(1);
            }
        }
        Ok(())
    }
}
