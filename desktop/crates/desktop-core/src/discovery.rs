// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding agents on the local network: they advertise `_ztop._tcp`
//! over multicast DNS when started with `--tls` (the engine's decision
//! 0044), with the instance name `ztop on <hostname>` and a TXT
//! `protocol` entry.

use std::collections::BTreeMap;
use std::net::IpAddr;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use ztop_core::client::PROTOCOL_VERSION;

/// The service type the agent advertises.
pub const SERVICE_TYPE: &str = "_ztop._tcp.local.";

/// One advertised agent, ready for the add-host form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The full DNS-SD name, unique on the network.
    pub fullname: String,
    /// The machine's name for the card: `ztop on mini` is `mini`.
    pub name: String,
    /// The address to save: loopback for this machine, else an IP.
    pub host: String,
    pub port: u16,
    /// The protocol version from the TXT record, when it has one.
    pub protocol: Option<u32>,
}

impl Found {
    /// An agent without a protocol entry is offered anyway; one that
    /// names another version is listed but cannot be added.
    pub fn compatible(&self) -> bool {
        self.protocol.is_none_or(|p| p == PROTOCOL_VERSION)
    }
}

/// `ztop on mini._ztop._tcp.local.` is `mini`; any other instance is
/// shown as it is.
pub fn display_name(fullname: &str) -> String {
    let instance = fullname
        .strip_suffix(SERVICE_TYPE)
        .map_or(fullname, |s| s.trim_end_matches('.'));
    match instance.strip_prefix("ztop on ") {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => instance.to_string(),
    }
}

/// The address to save for an advertised agent. One on this machine
/// is reached on loopback, because the default agent binds `127.0.0.1`
/// only and would refuse its LAN address. Otherwise an IPv4 address
/// beats the `.local` name, which needs nss-mdns to resolve on Linux;
/// IPv6 link-local addresses need a scope the record cannot keep.
pub fn address_for(host: &str, addresses: &[IpAddr], local_hostname: &str) -> String {
    if !local_hostname.is_empty() && normalized(host) == normalized(local_hostname) {
        return "127.0.0.1".into();
    }
    let mut v4: Vec<&IpAddr> = addresses.iter().filter(|a| a.is_ipv4()).collect();
    v4.sort();
    if let Some(address) = v4.first() {
        return address.to_string();
    }
    let mut v6: Vec<&IpAddr> = addresses
        .iter()
        .filter(|a| match a {
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
            IpAddr::V4(_) => false,
        })
        .collect();
    v6.sort();
    v6.first()
        .map_or_else(|| host.trim_end_matches('.').to_string(), |a| a.to_string())
}

fn normalized(name: &str) -> String {
    let name = name.trim_end_matches('.').to_lowercase();
    name.strip_suffix(".local").unwrap_or(&name).to_string()
}

/// A running browse. Dropping it stops the multicast daemon.
pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
    found: BTreeMap<String, Found>,
    local_hostname: String,
}

impl Browser {
    pub fn start() -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let events = daemon.browse(SERVICE_TYPE).map_err(|e| e.to_string())?;
        Ok(Self {
            daemon,
            events,
            found: BTreeMap::new(),
            local_hostname: ztop_core::host::hostname(),
        })
    }

    /// Takes in what the network said since the last call; true when
    /// the list changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(service) => {
                    let addresses: Vec<IpAddr> = service
                        .get_addresses()
                        .iter()
                        .map(|a| a.to_ip_addr())
                        .collect();
                    let found = Found {
                        fullname: service.get_fullname().to_string(),
                        name: display_name(service.get_fullname()),
                        host: address_for(service.get_hostname(), &addresses, &self.local_hostname),
                        port: service.get_port(),
                        protocol: service
                            .get_property_val_str("protocol")
                            .and_then(|p| p.trim().parse().ok()),
                    };
                    if self.found.get(&found.fullname) != Some(&found) {
                        self.found.insert(found.fullname.clone(), found);
                        changed = true;
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    changed |= self.found.remove(&fullname).is_some();
                }
                _ => {}
            }
        }
        changed
    }

    /// What is on the network now, by name.
    pub fn found(&self) -> Vec<Found> {
        let mut found: Vec<Found> = self.found.values().cloned().collect();
        found.sort_by_key(|a| a.name.to_lowercase());
        found
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instance_name_gives_the_machine_name() {
        assert_eq!(display_name("ztop on mini._ztop._tcp.local."), "mini");
        assert_eq!(display_name("printer._ztop._tcp.local."), "printer");
        assert_eq!(display_name("ztop on ._ztop._tcp.local."), "ztop on ");
    }

    #[test]
    fn this_machine_is_loopback_and_others_prefer_ipv4() {
        let ips =
            |list: &[&str]| -> Vec<IpAddr> { list.iter().map(|a| a.parse().unwrap()).collect() };
        assert_eq!(
            address_for("Mini.local.", &ips(&["192.0.2.7"]), "mini"),
            "127.0.0.1"
        );
        assert_eq!(
            address_for(
                "studio.local.",
                &ips(&["fe80::1", "2001:db8::5", "192.0.2.9"]),
                "mini"
            ),
            "192.0.2.9"
        );
        assert_eq!(
            address_for("studio.local.", &ips(&["fe80::1", "2001:db8::5"]), "mini"),
            "2001:db8::5"
        );
        assert_eq!(
            address_for("studio.local.", &ips(&["fe80::1"]), "mini"),
            "studio.local"
        );
    }

    #[test]
    fn another_protocol_is_listed_but_not_compatible() {
        let found = |protocol| Found {
            fullname: "ztop on a._ztop._tcp.local.".into(),
            name: "a".into(),
            host: "192.0.2.1".into(),
            port: 7100,
            protocol,
        };
        assert!(found(None).compatible());
        assert!(found(Some(PROTOCOL_VERSION)).compatible());
        assert!(!found(Some(PROTOCOL_VERSION + 1)).compatible());
    }
}
