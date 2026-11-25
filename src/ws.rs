//! The small part of RFC 6455 the TV app uses: the opening handshake and single-frame messages.

pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Sent for every refused upgrade: a definitive answer stops HTTP clients retrying on a reset.
pub const RESPONSE_403: &[u8] = b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

#[derive(Debug, PartialEq, Eq)]
pub struct Handshake {
    pub key: String,
    /// Request target, such as `/?t=TOKEN`.
    pub target: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeError {
    NotGet,
    MissingKey,
}

/// Index just past the blank line ending the request head, if it has arrived.
pub fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

pub fn parse_handshake(head: &str) -> Result<Handshake, HandshakeError> {
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let mut parts = request.split_ascii_whitespace();
    if parts.next() != Some("GET") {
        return Err(HandshakeError::NotGet);
    }
    let target = parts.next().ok_or(HandshakeError::NotGet)?.to_string();
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/")) {
        return Err(HandshakeError::NotGet);
    }
    let key = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("sec-websocket-key"))
        .map(|(_, value)| value.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or(HandshakeError::MissingKey)?;
    Ok(Handshake { key, target })
}

/// The value of query parameter `name` in a request target, percent-decoded.
pub fn query_param(target: &str, name: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Compares secrets in time that does not depend on where they first differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn accept_key(key: &str) -> String {
    let mut sha = sha1_smol::Sha1::new();
    sha.update(key.trim().as_bytes());
    sha.update(GUID.as_bytes());
    base64(&sha.digest().bytes())
}

pub fn response_101(key: &str) -> String {
    format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(key))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            s.push(if i <= c.len() { T[(n >> shift) as usize & 63] as char } else { '=' });
        }
    }
    s
}

/// One frame decoded in place: `payload` indexes the (unmasked) buffer.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: std::ops::Range<usize>,
    /// Bytes the frame occupies, header included.
    pub len: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooLarge(u64),
}

/// Decodes the frame at the start of `buf`, unmasking its payload in place.
/// `Ok(None)` means more bytes are needed.
pub fn decode(buf: &mut [u8], max_payload: usize) -> Result<Option<Frame>, FrameError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let fin = buf[0] & 0x80 != 0;
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    let (len, mut at) = match buf[1] & 0x7F {
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            (u64::from_be_bytes(buf[2..10].try_into().unwrap()), 10)
        }
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
        }
        n => (n as u64, 2),
    };
    if len > max_payload as u64 {
        return Err(FrameError::TooLarge(len));
    }
    let len = len as usize;
    let mask = if masked {
        if buf.len() < at + 4 {
            return Ok(None);
        }
        let m = [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
        at += 4;
        Some(m)
    } else {
        None
    };
    if buf.len() - at < len {
        return Ok(None);
    }
    if let Some(m) = mask {
        for (i, b) in buf[at..at + len].iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    Ok(Some(Frame { fin, opcode, payload: at..at + len, len: at + len }))
}

/// Encodes one unmasked, final frame as a server sends it.
pub fn encode(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 10);
    f.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => f.push(n as u8),
        n if n <= u16::MAX as usize => {
            f.push(126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(payload);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDSHAKE: &str = "GET /?t=ab%2Fc HTTP/1.1\r\nHost: 192.168.1.2:41230\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";

    #[test]
    fn accept_key_matches_rfc_6455_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn base64_pads_every_tail_length() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn parses_handshake_and_token() {
        assert_eq!(head_end(HANDSHAKE.as_bytes()), Some(HANDSHAKE.len()));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n"), None);
        let h = parse_handshake(HANDSHAKE).unwrap();
        assert_eq!(h.key, "dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(query_param(&h.target, "t").as_deref(), Some("ab/c"));
        assert_eq!(query_param("/?x=1&t=", "t").as_deref(), Some(""));
        assert_eq!(query_param("/?tt=1", "t"), None);
        assert_eq!(query_param("/", "t"), None);
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%41"), "%zzA");
    }

    #[test]
    fn rejects_bad_handshakes() {
        assert_eq!(parse_handshake("POST / HTTP/1.1\r\nSec-WebSocket-Key: x\r\n\r\n"), Err(HandshakeError::NotGet));
        assert_eq!(parse_handshake("GET / HTTP/1.1\r\nHost: x\r\n\r\n"), Err(HandshakeError::MissingKey));
        assert_eq!(parse_handshake("GET / HTTP/1.1\r\nSec-WebSocket-Key:  \r\n\r\n"), Err(HandshakeError::MissingKey));
        assert_eq!(parse_handshake("\u{16}\u{3}\u{1}"), Err(HandshakeError::NotGet));
    }

    #[test]
    fn constant_time_eq_compares_content_and_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn decodes_masked_frames_split_across_reads() {
        // "Hello" masked, from RFC 6455 section 5.7
        let frame = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        for cut in 0..frame.len() {
            assert_eq!(decode(&mut frame[..cut].to_vec(), 64), Ok(None), "cut at {cut}");
        }
        let mut buf = frame.to_vec();
        buf.extend_from_slice(&[0x82, 0x00]);
        let f = decode(&mut buf, 64).unwrap().unwrap();
        assert_eq!((f.fin, f.opcode, f.len), (true, OP_TEXT, 11));
        assert_eq!(&buf[f.payload], b"Hello");
    }

    #[test]
    fn decodes_extended_lengths_and_enforces_the_cap() {
        let mut buf = encode(OP_BINARY, &[7u8; 300]);
        let f = decode(&mut buf, 1024).unwrap().unwrap();
        assert_eq!((f.payload.len(), f.len), (300, 304));
        assert_eq!(decode(&mut buf, 299), Err(FrameError::TooLarge(300)));
        let mut big = encode(OP_BINARY, &vec![0u8; 70_000]);
        assert_eq!(&big[..2], &[0x82, 127]);
        assert_eq!(decode(&mut big, 100_000).unwrap().unwrap().payload.len(), 70_000);
        let mut huge = vec![0x82, 127, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(decode(&mut huge, 65_536), Err(FrameError::TooLarge(u64::MAX)));
    }

    #[test]
    fn encodes_the_frames_the_c_sharp_service_sent() {
        assert_eq!(encode(OP_CLOSE, &[]), [0x88, 0x00]);
        assert_eq!(encode(OP_PING, &[]), [0x89, 0x00]);
        assert_eq!(encode(OP_PING, &[0x01]), [0x89, 0x01, 0x01]);
        assert_eq!(encode(OP_BINARY, &[0x01]), [0x82, 0x01, 0x01]);
        assert_eq!(encode(OP_BINARY, &[0x03, 0xFF]), [0x82, 0x02, 0x03, 0xFF]);
    }
}
