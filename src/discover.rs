//! Finds LG webOS TVs on the local networks with SSDP, so setup can offer them instead of asking
//! for an IP address.

use crate::win::net;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

/// What LG's own remote apps search for; webOS TVs answer it (the C2 does, on webOS 25).
const SEARCH_TARGET: &str = "urn:lge-com:service:webos-second-screen:1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundTv {
    pub ip: Ipv4Addr,
    /// Such as "webOS TV OLED48C26LA"; "LG TV" when the TV doesn't say.
    pub name: String,
}

/// The TVs that answer within `wait`, by address.
pub fn find_tvs(wait: Duration) -> Vec<FoundTv> {
    let request = format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {SEARCH_TARGET}\r\n\r\n");
    let group = SocketAddr::from((Ipv4Addr::new(239, 255, 255, 250), 1900));
    // One socket per network: with several (Wi-Fi, a VPN, Hyper-V switches) Windows would send the
    // search out of only one of them
    let sockets: Vec<UdpSocket> = net::interfaces()
        .into_iter()
        .filter_map(|i| {
            let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
            s.bind(&SocketAddr::from((i.ip, 0)).into()).ok()?;
            s.set_multicast_if_v4(&i.ip).ok()?;
            s.set_multicast_ttl_v4(2).ok()?;
            let s: UdpSocket = s.into();
            s.set_read_timeout(Some(Duration::from_millis(40))).ok()?;
            for _ in 0..2 {
                let _ = s.send_to(request.as_bytes(), group);
            }
            Some(s)
        })
        .collect();
    let mut answers: Vec<(Ipv4Addr, String)> = Vec::new();
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline && !sockets.is_empty() {
        for s in &sockets {
            if let Ok((n, SocketAddr::V4(from))) = s.recv_from(&mut buf) {
                let reply = String::from_utf8_lossy(&buf[..n]);
                if let Some(location) = location_of(&reply) {
                    if !answers.iter().any(|(ip, _)| ip == from.ip()) {
                        answers.push((*from.ip(), location));
                    }
                }
            }
        }
    }
    let mut tvs: Vec<FoundTv> =
        answers.into_iter().map(|(ip, location)| FoundTv { ip, name: friendly_name(ip, &location).unwrap_or_else(|| "LG TV".into()) }).collect();
    tvs.sort_by_key(|t| t.ip);
    tvs
}

/// The description URL of a reply to our search, if it is one.
fn location_of(reply: &str) -> Option<String> {
    let mut lines = reply.split("\r\n");
    if !lines.next()?.starts_with("HTTP/1.1 200") {
        return None;
    }
    let mut st = false;
    let mut location = None;
    for line in lines {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim().to_ascii_lowercase().as_str() {
            "st" => st = value.trim().eq_ignore_ascii_case(SEARCH_TARGET),
            "location" => location = Some(value.trim().to_string()),
            _ => {}
        }
    }
    location.filter(|_| st)
}

/// The TV's name from its UPnP description. Only fetched from the TV that answered, never from
/// another host a reply names.
fn friendly_name(ip: Ipv4Addr, location: &str) -> Option<String> {
    let rest = location.strip_prefix("http://")?;
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let addr: SocketAddr = host.parse().ok()?;
    if addr.ip() != IpAddr::V4(ip) {
        return None;
    }
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n").ok()?;
    let mut body = Vec::new();
    let _ = s.take(64 * 1024).read_to_end(&mut body);
    name_in_description(&String::from_utf8_lossy(&body))
}

fn name_in_description(xml: &str) -> Option<String> {
    let start = xml.find("<friendlyName>")? + "<friendlyName>".len();
    let raw = &xml[start..start + xml[start..].find("</friendlyName>")?];
    let name = raw.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
    let name = name.trim().trim_start_matches("[LG]").trim();
    (!name.is_empty()).then(|| name.chars().take(60).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_our_search_give_their_description() {
        let tv = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: http://192.168.8.145:1177/\r\nSERVER: WebOS/4.1.0 UPnP/1.0\r\nST: urn:lge-com:service:webos-second-screen:1\r\n\r\n";
        assert_eq!(location_of(tv).as_deref(), Some("http://192.168.8.145:1177/"));
        // Other devices (a router, a speaker) answer with other search targets
        let router = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.8.1:5431/gatedesc.xml\r\nST: upnp:rootdevice\r\n\r\n";
        assert_eq!(location_of(router), None);
        assert_eq!(location_of("NOTIFY * HTTP/1.1\r\nST: urn:lge-com:service:webos-second-screen:1\r\n\r\n"), None);
    }

    #[test]
    fn names_come_from_the_description() {
        let xml = "<root><device><friendlyName>[LG] webOS TV OLED48C26LA</friendlyName><manufacturer>LG Electronics</manufacturer></device></root>";
        assert_eq!(name_in_description(xml).as_deref(), Some("webOS TV OLED48C26LA"));
        assert_eq!(name_in_description("<friendlyName>Living &amp; dining</friendlyName>").as_deref(), Some("Living & dining"));
        assert_eq!(name_in_description("<friendlyName> </friendlyName>"), None);
        assert_eq!(name_in_description("no name here"), None);
    }

    #[test]
    fn descriptions_are_only_read_from_the_tv_that_answered() {
        assert_eq!(friendly_name(Ipv4Addr::new(192, 168, 8, 145), "http://10.0.0.1:80/"), None);
        assert_eq!(friendly_name(Ipv4Addr::new(192, 168, 8, 145), "https://192.168.8.145/"), None);
    }

    #[test]
    #[ignore = "searches the real network; finds the TVs that are on"]
    fn finds_the_tvs_on_this_network() {
        let tvs = find_tvs(Duration::from_secs(3));
        println!("{tvs:?}");
        assert!(!tvs.is_empty(), "no TV answered");
    }
}
