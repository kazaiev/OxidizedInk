//! The server side of HTTP/1.1, as much as the reader's upload page needs:
//! one request per connection, its body streamed in chunks so a large book
//! never sits in memory whole.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Error;

/// A TCP connection to a browser (a smoltcp socket on the device, a
/// `std::net::TcpStream` in the simulator).
pub trait Connection {
    /// Reads at least one byte into `buf`, or returns 0 once the browser has
    /// closed its side.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error>;
    /// Writes all of `data`.
    fn write(&mut self, data: &[u8]) -> Result<(), Error>;
}

/// Longest request line and headers accepted.
const HEAD_MAX: usize = 4096;

pub struct Request {
    pub method: String,
    /// The path without its query, still percent-encoded.
    pub path: String,
    /// The body's length (`Content-Length`), 0 if there is none.
    pub length: u64,
    /// The browser waits for "100 Continue" before sending the body.
    expect_continue: bool,
    /// Body bytes that arrived with the head.
    rest: Vec<u8>,
}

impl Request {
    /// Reads a request's line and headers.
    pub fn read(conn: &mut impl Connection) -> Result<Request, Error> {
        let mut head = Vec::new();
        let mut buf = [0u8; 512];
        let end = loop {
            if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if head.len() > HEAD_MAX {
                return Err(Error::Http("request head too long"));
            }
            let n = conn.read(&mut buf)?;
            if n == 0 {
                return Err(Error::Io);
            }
            head.extend_from_slice(&buf[..n]);
        };
        let rest = head.split_off(end + 4);
        let text = core::str::from_utf8(&head[..end]).map_err(|_| Error::Http("request head isn't UTF-8"))?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next().unwrap_or("").split(' ');
        let (Some(method), Some(target)) = (first.next(), first.next()) else {
            return Err(Error::Http("bad request line"));
        };
        let mut req = Request {
            method: method.into(),
            path: target.split('?').next().unwrap_or("").into(),
            length: 0,
            expect_continue: false,
            rest,
        };
        for line in lines {
            let Some((name, value)) = line.split_once(':') else { continue };
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                req.length = value.parse().map_err(|_| Error::Http("bad Content-Length"))?;
            } else if name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue") {
                req.expect_continue = true;
            }
        }
        Ok(req)
    }

    /// Reads the body, handing it to `f` a chunk at a time.
    pub fn body(
        &mut self,
        conn: &mut impl Connection,
        mut f: impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.expect_continue {
            conn.write(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        let mut left = self.length;
        let first = core::mem::take(&mut self.rest);
        let take = (first.len() as u64).min(left) as usize;
        if take > 0 {
            f(&first[..take])?;
            left -= take as u64;
        }
        let mut buf = alloc::vec![0u8; 8192];
        while left > 0 {
            let want = (buf.len() as u64).min(left) as usize;
            let n = conn.read(&mut buf[..want])?;
            if n == 0 {
                return Err(Error::Io);
            }
            f(&buf[..n])?;
            left -= n as u64;
        }
        Ok(())
    }
}

/// Sends a whole response and asks the browser to close the connection.
/// `status` is the code and reason, such as "200 OK".
pub fn respond(conn: &mut impl Connection, status: &str, content_type: &str, body: &[u8]) -> Result<(), Error> {
    respond_head(conn, status, content_type, body.len() as u64, "")?;
    conn.write(body)
}

/// Sends a response's status and headers, for a body of `length` bytes
/// that the caller writes next. `extra` is more header lines, each ending
/// in CRLF.
pub fn respond_head(
    conn: &mut impl Connection,
    status: &str,
    content_type: &str,
    length: u64,
    extra: &str,
) -> Result<(), Error> {
    let head = alloc::format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        status, content_type, length, extra
    );
    conn.write(head.as_bytes())
}

/// Decodes `%XX` escapes in a path segment. None if the result isn't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Appends `s` to `out` as a JSON string, quotes included.
pub fn push_json(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A connection that hands out its input a few bytes at a time.
    struct Script {
        input: Vec<u8>,
        at: usize,
        output: Vec<u8>,
    }

    impl Connection for Script {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
            let n = buf.len().min(7).min(self.input.len() - self.at);
            buf[..n].copy_from_slice(&self.input[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }

        fn write(&mut self, data: &[u8]) -> Result<(), Error> {
            self.output.extend_from_slice(data);
            Ok(())
        }
    }

    #[test]
    fn reads_head_and_streams_body() {
        let body: Vec<u8> = (0..20000u32).map(|i| i as u8).collect();
        let mut input = b"PUT /books/A%20b.epub?x=1 HTTP/1.1\r\nHost: r\r\ncontent-length: 20000\r\nExpect: 100-continue\r\n\r\n".to_vec();
        input.extend_from_slice(&body);
        let mut conn = Script { input, at: 0, output: vec![] };
        let mut req = Request::read(&mut conn).unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str(), req.length), ("PUT", "/books/A%20b.epub", 20000));
        let mut got = vec![];
        req.body(&mut conn, |c| {
            got.extend_from_slice(c);
            Ok(())
        })
        .unwrap();
        assert_eq!(got, body);
        assert_eq!(conn.output, b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    #[test]
    fn decodes_and_escapes() {
        assert_eq!(percent_decode("A%20b%C3%A9.epub").as_deref(), Some("A bé.epub"));
        assert_eq!(percent_decode("100%").as_deref(), Some("100%"));
        let mut s = String::new();
        push_json(&mut s, "a\"b\\c\n");
        assert_eq!(s, "\"a\\\"b\\\\c\\u000a\"");
    }
}
