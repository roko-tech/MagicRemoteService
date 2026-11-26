//! The PC's IPv4 interfaces, for the TV config's IP, MAC (Wake-on-LAN) and subnet mask.

use super::from_wide;
use std::net::Ipv4Addr;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub ip: Ipv4Addr,
    pub mac: String,
    pub mask: Ipv4Addr,
    pub name: String,
    /// Has a default gateway: the network the TV is most likely on.
    pub gateway: bool,
    /// The adapter's id, `{GUID}`.
    pub adapter: String,
}

pub fn mask(prefix: u8) -> Ipv4Addr {
    Ipv4Addr::from(if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32) as u32) })
}

/// Up, non-loopback IPv4 interfaces; those with a gateway first, since `install.bat` takes the first.
pub fn interfaces() -> Vec<Interface> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut len: u32 = 16 * 1024;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        buf = vec![0u64; len as usize / 8 + 1];
        let r = unsafe { GetAdaptersAddresses(AF_INET as u32, flags, std::ptr::null(), buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut len) };
        if r == 0 {
            break;
        }
        if r != 111 {
            // anything but ERROR_BUFFER_OVERFLOW
            return Vec::new();
        }
    }
    let mut out = Vec::new();
    unsafe {
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let ad = &*a;
            a = ad.Next;
            if ad.OperStatus != 1 || ad.IfType == 24 {
                // not IfOperStatusUp, or IF_TYPE_SOFTWARE_LOOPBACK
                continue;
            }
            let mac = ad.PhysicalAddress[..ad.PhysicalAddressLength as usize].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
            let name = from_wide(std::slice::from_raw_parts(ad.FriendlyName, (0..).take_while(|&i| *ad.FriendlyName.add(i) != 0).count()));
            let gateway = !ad.FirstGatewayAddress.is_null();
            let adapter = if ad.AdapterName.is_null() { String::new() } else { std::ffi::CStr::from_ptr(ad.AdapterName as _).to_string_lossy().into_owned() };
            let mut u = ad.FirstUnicastAddress;
            while !u.is_null() {
                let ua = &*u;
                u = ua.Next;
                let sa = ua.Address.lpSockaddr;
                if sa.is_null() || (*sa).sa_family != AF_INET {
                    continue;
                }
                let sin = &*(sa as *const SOCKADDR_IN);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                out.push(Interface { ip, mac: mac.clone(), mask: mask(ua.OnLinkPrefixLength), name: name.clone(), gateway, adapter: adapter.clone() });
            }
        }
    }
    out.sort_by_key(|i| !i.gateway);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_lengths_become_masks() {
        assert_eq!(mask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(mask(20), Ipv4Addr::new(255, 255, 240, 0));
        assert_eq!(mask(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(mask(32), Ipv4Addr::new(255, 255, 255, 255));
    }

    #[test]
    fn lists_at_least_this_machines_network() {
        let list = interfaces();
        assert!(list.iter().all(|i| !i.ip.is_loopback()));
    }
}
