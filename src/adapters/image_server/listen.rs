//! Opens the server's listening socket, and says which IP versions it accepts, so the mDNS
//! announcement can match what is really served.
//!
//! The wildcard IPv6 address `[::]` is opened as a dual-stack socket: one socket that takes
//! IPv4 clients too (as `::ffff:a.b.c.d`). Whether a `[::]` socket does that by default depends on
//! the system (`net.ipv6.bindv6only` on Linux, Windows and some BSDs say no), so it is set
//! explicitly instead of left to chance. A host without IPv6, or one that can't mix the two,
//! gets an IPv4 socket instead, and says so.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

/// Which IP versions a listener accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Families {
    Both,
    V4,
    V6,
}

impl fmt::Display for Families {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Both => "IPv4 and IPv6",
            Self::V4 => "IPv4 only",
            Self::V6 => "IPv6 only",
        })
    }
}

pub struct Bound {
    pub listener: TcpListener,
    pub families: Families,
}

/// Listens on `address`. `[::]` accepts both IP versions where the host allows it; any other
/// address accepts only its own.
pub fn bind(address: SocketAddr) -> anyhow::Result<Bound> {
    let (socket, address, families) = match address {
        SocketAddr::V4(_) => (Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)), address, Families::V4),
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => match dual_stack() {
            Ok(socket) => (Ok(socket), address, Families::Both),
            Err(e) => {
                log::warn!("No dual-stack IPv6 socket here ({e}); listening on IPv4 only");
                let v4 = SocketAddr::from((Ipv4Addr::UNSPECIFIED, v6.port()));
                (Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)), v4, Families::V4)
            }
        },
        // A specific IPv6 address is only ever IPv6.
        SocketAddr::V6(_) => (
            Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)).and_then(|socket| {
                socket.set_only_v6(true)?;
                Ok(socket)
            }),
            address,
            Families::V6,
        ),
    };
    let socket = socket.with_context(|| format!("Failed to create a socket for {address}"))?;
    let listener = open(socket, address).with_context(|| format!("Failed to listen on {address}"))?;
    Ok(Bound { listener, families })
}

fn dual_stack() -> std::io::Result<Socket> {
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_only_v6(false)?;
    Ok(socket)
}

fn open(socket: Socket, address: SocketAddr) -> std::io::Result<TcpListener> {
    // What tokio's own `bind` does: a restarted server may reuse the port while old connections linger.
    // Not on Windows, where it would let another program share the port.
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpStream;

    use super::*;

    async fn connects(address: SocketAddr) -> bool {
        TcpStream::connect(address).await.is_ok()
    }

    #[tokio::test]
    async fn the_wildcard_ipv6_address_accepts_both_versions() {
        let bound = bind("[::]:0".parse().unwrap()).unwrap();
        if bound.families != Families::Both {
            eprintln!("skipped: this host has no dual-stack IPv6");
            return;
        }
        let port = bound.listener.local_addr().unwrap().port();
        assert!(connects(SocketAddr::from(([127, 0, 0, 1], port))).await);
        assert!(connects(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))).await);
    }

    #[tokio::test]
    async fn an_ipv4_address_accepts_only_ipv4() {
        let bound = bind("0.0.0.0:0".parse().unwrap()).unwrap();
        assert_eq!(bound.families, Families::V4);
        let port = bound.listener.local_addr().unwrap().port();
        assert!(connects(SocketAddr::from(([127, 0, 0, 1], port))).await);
        assert!(!connects(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))).await);
    }

    #[tokio::test]
    async fn a_specific_ipv6_address_accepts_only_ipv6() {
        let Ok(bound) = bind("[::1]:0".parse().unwrap()) else {
            eprintln!("skipped: this host has no IPv6 loopback");
            return;
        };
        assert_eq!(bound.families, Families::V6);
        let port = bound.listener.local_addr().unwrap().port();
        assert!(connects(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))).await);
        assert!(!connects(SocketAddr::from(([127, 0, 0, 1], port))).await);
    }

    #[tokio::test]
    async fn a_port_in_use_is_an_error_not_a_fallback() {
        let first = bind("[::]:0".parse().unwrap()).unwrap();
        let port = first.listener.local_addr().unwrap().port();
        let error = bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port))).err().expect("the port is taken");
        assert!(format!("{error:#}").contains("Failed to listen"), "{error:#}");
    }
}
