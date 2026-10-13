//! Which source on the network an address belongs to, for counting what one source does.

use std::net::{IpAddr, Ipv6Addr};

/// A source is an address, except that all of an IPv6 /64 is one (a host is given the whole of one, so counting
/// addresses would give an attacker as many as it likes), and an IPv4 address written as IPv6 is the IPv4 one.
pub type Source = IpAddr;

/// The source an address belongs to.
pub fn source_of(address: IpAddr) -> Source {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        },
        v4 => v4,
    }
}
