// nyx-tunnel: userspace WireGuard for Nyx so torrent traffic can be forced through a VPN
// without any system-wide driver. It holds the `.conf` parser, the SOCKS5 server and the tunnel.

pub mod conf;
pub mod socks;
pub mod tunnel;

#[cfg(feature = "test-support")]
pub mod testpeer;
