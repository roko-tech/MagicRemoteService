//! Minimal HTTP to the local settings server: a GET for `status`, and JSON calls for the
//! settings window.

use serde_json::Value;
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

/// A JSON call to the local API. POSTs carry the header the API asks of every change. `timeout`
/// bounds the wait for the answer: pairing and installing on a TV can take minutes.
pub fn call(port: u16, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, String> {
    let down = |e: std::io::Error| format!("The service isn't answering ({e}).");
    let mut s = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(2)).map_err(down)?;
    s.set_read_timeout(Some(timeout)).map_err(down)?;
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if method == "POST" {
        request += &format!("X-Requested-With: MagicRemoteService\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
    }
    request += "\r\n";
    request += &body;
    s.write_all(request.as_bytes()).map_err(down)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(down)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("The service sent a response this window can't read.")?;
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(json)
    } else {
        Err(json.get("error").and_then(Value::as_str).map_or_else(|| format!("The service answered {status}."), str::to_string))
    }
}
