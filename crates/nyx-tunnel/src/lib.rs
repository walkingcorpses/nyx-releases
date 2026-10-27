// nyx-tunnel: userspace WireGuard for Nyx so torrent traffic can be forced through a VPN
// without any system-wide driver. It holds the `.conf` parser and the SOCKS5 server.

pub mod conf;
pub mod socks;
