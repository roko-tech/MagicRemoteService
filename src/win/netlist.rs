//! Whether Windows calls a network public. Its firewall then keeps the TV out, since the rules
//! the installer adds cover private and domain networks only.

use super::net::{self, Interface};
use std::net::Ipv4Addr;
use windows::core::GUID;
use windows::Win32::Networking::NetworkListManager::{
    INetwork, INetworkListManager, NetworkListManager, NLM_NETWORK_CATEGORY_PRIVATE, NLM_NETWORK_CATEGORY_PUBLIC,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

/// A network this PC is connected to, by the adapter it's on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// The adapter's id, `{GUID}`, as the interface list names it.
    pub adapter: String,
    /// The network's id, `{GUID}`, to change its type.
    pub network: String,
    /// The name Windows shows, such as the Wi-Fi's.
    pub name: String,
    pub public: bool,
}

fn text(id: &GUID) -> String {
    format!("{{{id:?}}}")
}

fn manager() -> windows::core::Result<INetworkListManager> {
    unsafe {
        // Once per thread; repeated calls are harmless
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)
    }
}

/// The networks this PC is connected to, one per adapter connection.
pub fn connections() -> windows::core::Result<Vec<Connection>> {
    let mut out = Vec::new();
    unsafe {
        let all = manager()?.GetNetworkConnections()?;
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            // The end of the list is S_FALSE, which is still Ok: it fetches nothing
            all.Next(&mut slot, Some(&mut fetched))?;
            let Some(connection) = slot[0].take().filter(|_| fetched == 1) else { break };
            let network = connection.GetNetwork()?;
            out.push(Connection {
                adapter: text(&connection.GetAdapterId()?),
                network: text(&network.GetNetworkId()?),
                name: network.GetName()?.to_string(),
                public: network.GetCategory()? == NLM_NETWORK_CATEGORY_PUBLIC,
            });
        }
    }
    Ok(out)
}

/// The network of the interface with address `ip`, if Windows calls it public.
pub fn public_network_of(ip: Ipv4Addr, interfaces: &[Interface], connections: &[Connection]) -> Option<Connection> {
    let interface = interfaces.iter().find(|i| i.ip == ip)?;
    connections.iter().find(|c| c.public && c.adapter.eq_ignore_ascii_case(&interface.adapter)).cloned()
}

/// The network this PC's address `ip` is on, if Windows calls it public.
pub fn public_network(ip: Ipv4Addr) -> Option<Connection> {
    public_network_of(ip, &net::interfaces(), &connections().ok()?)
}

fn network(id: GUID) -> Result<INetwork, String> {
    manager().and_then(|m| unsafe { m.GetNetwork(id) }).map_err(|e| format!("Windows doesn't know that network: {}", e.message()))
}

/// Makes the network with id `network` (`{GUID}`) private, so the firewall lets the TV in. Needs
/// an administrator.
pub fn make_private(network_id: &str) -> Result<String, String> {
    let id = GUID::try_from(network_id.trim_matches(['{', '}'])).map_err(|_| format!("{network_id} isn't a network id."))?;
    let net = network(id)?;
    let name = unsafe { net.GetName() }.map(|n| n.to_string()).unwrap_or_default();
    unsafe { net.SetCategory(NLM_NETWORK_CATEGORY_PRIVATE) }.map_err(|e| format!("Windows didn't make \u{201c}{name}\u{201d} private: {}", e.message()))?;
    // Read it back: a policy on the PC can keep the network public
    if unsafe { network(id)?.GetCategory() }.is_ok_and(|c| c == NLM_NETWORK_CATEGORY_PUBLIC) {
        return Err(format!("Windows kept \u{201c}{name}\u{201d} public. A policy on this PC may decide the network type."));
    }
    Ok(format!("\u{201c}{name}\u{201d} is now a private network."))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIFI: &str = "{6B29FC40-CA47-1067-B31D-00DD010662DA}";
    const VPN: &str = "{0F8FAD5B-D9CB-469F-A165-70867728950E}";

    fn interface(ip: [u8; 4], adapter: &str) -> Interface {
        Interface { ip: ip.into(), mac: String::new(), mask: Ipv4Addr::new(255, 255, 255, 0), name: String::new(), gateway: true, adapter: adapter.into() }
    }

    fn connection(adapter: &str, public: bool) -> Connection {
        Connection { adapter: adapter.into(), network: "{11111111-2222-3333-4444-555555555555}".into(), name: "Home".into(), public }
    }

    #[test]
    fn a_public_network_on_the_tvs_interface_is_reported() {
        let interfaces = [interface([192, 168, 1, 10], WIFI), interface([10, 8, 0, 2], VPN)];
        let found = public_network_of(Ipv4Addr::new(192, 168, 1, 10), &interfaces, &[connection(WIFI, true)]);
        assert_eq!(found.map(|c| c.name), Some("Home".into()));
        // Adapter ids match whatever their case
        let lower = connection(&WIFI.to_lowercase(), true);
        assert!(public_network_of(Ipv4Addr::new(192, 168, 1, 10), &interfaces, &[lower]).is_some());
    }

    #[test]
    fn private_networks_and_other_adapters_are_not() {
        let interfaces = [interface([192, 168, 1, 10], WIFI), interface([10, 8, 0, 2], VPN)];
        let ip = Ipv4Addr::new(192, 168, 1, 10);
        assert_eq!(public_network_of(ip, &interfaces, &[connection(WIFI, false)]), None);
        // A public VPN doesn't stand between the TV and the PC
        assert_eq!(public_network_of(ip, &interfaces, &[connection(WIFI, false), connection(VPN, true)]), None);
        // No interface has that address
        assert_eq!(public_network_of(Ipv4Addr::new(192, 168, 7, 7), &interfaces, &[connection(WIFI, true)]), None);
    }

    #[test]
    fn only_a_network_id_is_accepted() {
        assert!(make_private("Home; del *").unwrap_err().contains("isn't a network id"));
    }

    #[test]
    #[ignore = "reads this PC's networks"]
    fn reads_this_pcs_networks() {
        let list = connections().expect("the Network List Manager answers");
        for c in &list {
            println!("{c:?}");
        }
        let interfaces = net::interfaces();
        let first = interfaces.first().expect("a network");
        assert!(list.iter().any(|c| c.adapter.eq_ignore_ascii_case(&first.adapter)), "{first:?} has a connection");
    }
}
