//! nyx-daemon library half: the control protocol lives in `ctl` so tests can call it without sockets.
//! The binary (`main.rs`) adds the process wiring: env, tick loop, TCP listener, `ctl` client.

pub mod ctl;

use std::net::SocketAddr;

/// The listen address for torrent traffic from `NYX_LISTEN`: unset means 0.0.0.0:6881, anything
/// unparseable is an error (silently falling back would bind an address the operator did not ask for).
pub fn listen_addr(var: Option<&str>) -> Result<SocketAddr, String> {
    match var {
        None => Ok(SocketAddr::from(([0, 0, 0, 0], 6881))),
        Some(v) => v.parse().map_err(|_| format!("NYX_LISTEN={v:?} is not an IP:port address")),
    }
}
