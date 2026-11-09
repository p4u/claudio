//! The TCP peer of a request, from the connection (never a header).

use std::net::{IpAddr, SocketAddr};

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::net::TcpListener;

/// The peer address as `ConnectInfo<Peer>`: claudio-web's own type, since
/// the orphan rule forbids `Connected<IncomingStream<'_, TlsListener>>` for
/// `SocketAddr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Peer(pub SocketAddr);

/// What the login limits and the connection caps count by: every loopback
/// address is one peer, an IPv4 address its own, an IPv6 address its /64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PeerKey {
    Loopback,
    V4(u32),
    V6Net(u64),
    /// A request without a peer (never from a server: it always has one).
    Unknown,
}

impl PeerKey {
    /// `ip`'s key without the loopback rule: its IPv4 address or IPv6 /64.
    pub(crate) fn of(ip: IpAddr) -> PeerKey {
        match ip.to_canonical() {
            IpAddr::V4(v4) => PeerKey::V4(u32::from(v4)),
            IpAddr::V6(v6) => PeerKey::V6Net((u128::from(v6) >> 64) as u64),
        }
    }
}

impl Peer {
    /// The canonical address: `::ffff:a.b.c.d` is `a.b.c.d`.
    pub(crate) fn ip(&self) -> IpAddr {
        self.0.ip().to_canonical()
    }

    pub(crate) fn key(&self) -> PeerKey {
        match self.ip() {
            ip if ip.is_loopback() => PeerKey::Loopback,
            ip => PeerKey::of(ip),
        }
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Peer {
        Peer(*stream.remote_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(s: &str) -> Peer {
        Peer(s.parse().unwrap())
    }

    #[test]
    fn keys_fold_loopback_and_ipv6_nets() {
        for lo in ["[::ffff:127.0.0.1]:1", "127.0.0.9:1", "[::1]:1"] {
            assert_eq!(peer(lo).key(), PeerKey::Loopback, "{lo}");
        }
        assert_eq!(peer("[::ffff:10.0.0.1]:1").key(), PeerKey::V4(0x0a00_0001));
        assert_eq!(peer("[::ffff:10.0.0.1]:1").ip(), peer("10.0.0.1:2").ip());
        assert_eq!(
            peer("[2001:db8::1]:1").key(),
            peer("[2001:db8::ffff:1]:2").key()
        );
        assert_ne!(
            peer("[2001:db8::1]:1").key(),
            peer("[2001:db8:0:1::1]:1").key()
        );
    }
}
