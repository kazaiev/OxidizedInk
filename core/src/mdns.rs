//! Answering for the reader's name on the local network (multicast DNS),
//! so the Wi-Fi page is at `http://oxidized.local/` as well as at its
//! address. Only the IPv4 address is answered for.

use alloc::vec::Vec;

/// The reader's name on the network.
pub const NAME: &str = "oxidized.local";
/// mDNS's port and IPv4 group, 224.0.0.251.
pub const PORT: u16 = 5353;
pub const GROUP: [u8; 4] = [224, 0, 0, 251];

const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// In a question: answer straight to the asker. In an answer: this record
/// replaces any other for the name.
const TOP_BIT: u16 = 0x8000;
/// How long others may keep the answer, in seconds.
const TTL: u32 = 120;

/// The reply to `query` if it asks for `name`'s address: the packet, and
/// whether it goes straight back to the asker rather than to the group.
/// `from_port` is the query's source port: other than 5353, the asker is a
/// plain DNS resolver, which wants a plain DNS reply.
pub fn answer(query: &[u8], name: &str, ip: [u8; 4], from_port: u16) -> Option<(Vec<u8>, bool)> {
    let word = |at: usize| Some(u16::from_be_bytes([*query.get(at)?, *query.get(at + 1)?]));
    // A standard query: no response bit, opcode 0.
    if word(2)? & 0xf800 != 0 {
        return None;
    }
    let questions = word(4)?;
    let mut at = 12;
    for _ in 0..questions {
        let (asked, next) = read_name(query, at)?;
        let (qtype, qclass) = (word(next)?, word(next + 2)?);
        let start = at;
        at = next + 4;
        if !asked.eq_ignore_ascii_case(name) || !matches!(qtype, TYPE_A | TYPE_ANY) || qclass & !TOP_BIT != CLASS_IN {
            continue;
        }
        let legacy = from_port != PORT;
        let mut out = Vec::new();
        // A legacy resolver gets its id and question back.
        let id = if legacy { word(0)? } else { 0 };
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&0x8400u16.to_be_bytes());
        out.extend_from_slice(&(legacy as u16).to_be_bytes());
        out.extend_from_slice(&[0, 1, 0, 0, 0, 0]);
        if legacy {
            push_name(&mut out, name);
            out.extend_from_slice(&query[start + encoded_len(query, start)?..at]);
        }
        push_record(&mut out, name, ip, if legacy { 10 } else { TTL }, !legacy);
        return Some((out, legacy || qclass & TOP_BIT != 0));
    }
    None
}

/// An unasked answer telling the network the name's address, as a reader
/// sends when it joins.
pub fn announce(name: &str, ip: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::from([0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
    push_record(&mut out, name, ip, TTL, true);
    out
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

fn push_record(out: &mut Vec<u8>, name: &str, ip: [u8; 4], ttl: u32, flush: bool) {
    push_name(out, name);
    out.extend_from_slice(&TYPE_A.to_be_bytes());
    out.extend_from_slice(&(CLASS_IN | if flush { TOP_BIT } else { 0 }).to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&ip);
}

/// Reads the name at `at`, following compression pointers: its labels joined
/// by dots, and where what follows it starts.
fn read_name(packet: &[u8], mut at: usize) -> Option<(alloc::string::String, usize)> {
    let mut name = alloc::string::String::new();
    let mut end = None;
    for _ in 0..32 {
        let len = *packet.get(at)? as usize;
        match len {
            0 => return Some((name, end.unwrap_or(at + 1))),
            l if l & 0xc0 == 0xc0 => {
                end.get_or_insert(at + 2);
                at = (l & 0x3f) << 8 | *packet.get(at + 1)? as usize;
            }
            l if l < 64 => {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(core::str::from_utf8(packet.get(at + 1..at + 1 + l)?).ok()?);
                at += 1 + l;
            }
            _ => return None,
        }
    }
    None
}

/// How many bytes the name at `at` takes where it is.
fn encoded_len(packet: &[u8], at: usize) -> Option<usize> {
    read_name(packet, at).map(|(_, next)| next - at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A query for `name`, type `qtype`, class `qclass`, with id `id`.
    fn query(id: u16, name: &str, qtype: u16, qclass: u16) -> Vec<u8> {
        let mut q = vec![];
        q.extend_from_slice(&id.to_be_bytes());
        q.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        push_name(&mut q, name);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&qclass.to_be_bytes());
        q
    }

    const IP: [u8; 4] = [192, 168, 1, 148];

    #[test]
    fn answers_for_its_name_only() {
        let (reply, unicast) = answer(&query(0, "Oxidized.Local", TYPE_A, CLASS_IN), NAME, IP, PORT).unwrap();
        assert!(!unicast);
        assert_eq!(&reply[..12], &[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(&reply[reply.len() - 4..], &IP);
        assert_eq!(reply, announce(NAME, IP));
        assert!(answer(&query(0, "printer.local", TYPE_A, CLASS_IN), NAME, IP, PORT).is_none());
        // IPv6 addresses (AAAA) aren't answered.
        assert!(answer(&query(0, NAME, 28, CLASS_IN), NAME, IP, PORT).is_none());
        // Replies aren't answered.
        let mut reply_packet = query(0, NAME, TYPE_A, CLASS_IN);
        reply_packet[2] = 0x84;
        assert!(answer(&reply_packet, NAME, IP, PORT).is_none());
        assert!(answer(&[0, 0, 0], NAME, IP, PORT).is_none());
    }

    #[test]
    fn unicast_and_legacy_queries() {
        let (_, unicast) = answer(&query(0, NAME, TYPE_A, CLASS_IN | TOP_BIT), NAME, IP, PORT).unwrap();
        assert!(unicast);
        // A plain resolver: its id, the question, a short TTL, no flush bit.
        let q = query(0x1234, NAME, TYPE_ANY, CLASS_IN);
        let (reply, unicast) = answer(&q, NAME, IP, 40000).unwrap();
        assert!(unicast);
        assert_eq!(&reply[..6], &[0x12, 0x34, 0x84, 0, 0, 1]);
        assert_eq!(&reply[12..12 + q.len() - 12], &q[12..]);
        // The record ends with type, class, TTL, length and address.
        let record = &reply[q.len()..];
        let n = record.len();
        assert_eq!(&record[n - 12..n - 10], &[0, 1]);
        assert_eq!(&record[n - 10..n - 6], &[0, 0, 0, 10]);
    }

    #[test]
    fn compressed_names() {
        // Two questions, the second pointing into the first: "local" at 21.
        let mut q = vec![0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        push_name(&mut q, "printer.local");
        q.extend_from_slice(&[0, 1, 0, 1]);
        q.extend_from_slice(&[8]);
        q.extend_from_slice(b"oxidized");
        q.extend_from_slice(&[0xc0, 20, 0, 1, 0, 1]);
        let (reply, _) = answer(&q, NAME, IP, PORT).unwrap();
        assert_eq!(&reply[reply.len() - 4..], &IP);
    }
}
