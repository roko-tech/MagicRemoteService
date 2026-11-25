//! A minimal HTTP GET to the local settings server, for `status`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

pub fn get(port: u16, path: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(2))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n")?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| std::io::Error::other("bad response"))?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err(std::io::Error::other(head.lines().next().unwrap_or_default().to_string()));
    }
    Ok(body.to_string())
}
