//! Answers the TV service's SSDP search, so a TV without a configured IP finds the PC.

use crate::log;
use crate::state::Shared;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const PORT: u16 = 1900;
pub const USN: &str = "urn:magicremoteservice:service:remote:1";

pub fn is_search(msg: &str) -> bool {
    msg.contains("M-SEARCH") && msg.contains(USN)
}

pub fn response(local_ip: Ipv4Addr, ws_port: u16) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nST: {USN}\r\nUSN: {USN}\r\nLOCATION: ws://{local_ip}:{ws_port}\r\nCACHE-CONTROL: max-age=1800\r\nSERVER: MagicRemoteService/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Our address on the route to `remote` (connecting a UDP socket sends nothing).
fn local_ip_towards(remote: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((remote, 1)).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) => Some(*a.ip()),
        SocketAddr::V6(_) => None,
    }
}

pub fn bind(interfaces: &[Ipv4Addr]) -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    // Join on every interface, not just the default one, so a PC with several networks answers on all
    let mut joined = false;
    for ip in interfaces {
        joined |= s.join_multicast_v4(&MULTICAST, ip).is_ok();
    }
    if !joined {
        s.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED)?;
    }
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    Ok(s.into())
}

pub fn run(socket: UdpSocket, shared: Arc<Shared>, ws_port: u16) {
    let mut buf = [0u8; 2048];
    while !shared.stopping() {
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        let SocketAddr::V4(from) = from else { continue };
        if !is_search(&String::from_utf8_lossy(&buf[..n])) {
            continue;
        }
        if let Some(ip) = local_ip_towards(*from.ip()) {
            let _ = socket.send_to(response(ip, ws_port).as_bytes(), from);
            log::debug(format!("SSDP: answered {}", from.ip()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_only_our_search_with_a_location_service_js_parses() {
        let search = format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {USN}\r\n\r\n");
        assert!(is_search(&search));
        assert!(!is_search("M-SEARCH * HTTP/1.1\r\nST: ssdp:all\r\n\r\n"));
        let r = response(Ipv4Addr::new(192, 168, 1, 20), 41230);
        assert!(r.contains(USN));
        // service.js: /LOCATION:\s*ws:\/\/(\S+):(\d+)/i
        assert!(r.contains("\r\nLOCATION: ws://192.168.1.20:41230\r\n"));
    }
}
