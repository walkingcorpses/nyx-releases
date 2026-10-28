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

/// The network lock's thresholds from `NYX_LAB_STALE_AFTER_SECS`. LAB-ONLY override: the leak
/// lab sets it (20 s) so its kill-timing check does not wait the real 180 s. Unset keeps the
/// production thresholds; only `stale_after` changes (start grace stays default). Zero or a
/// non-number is an error, never a silent fallback. Only the daemon reads this variable. Takes
/// the raw `var_os` value so a non-UTF-8 value is an error too, not read as unset.
pub fn lab_timing(var: Option<&std::ffi::OsStr>) -> Result<nyx_app::netlock::NetTiming, String> {
    let mut t = nyx_app::netlock::NetTiming::default();
    if let Some(d) = positive_secs("NYX_LAB_STALE_AFTER_SECS", var)? {
        t.stale_after = d;
    }
    Ok(t)
}

/// The fixed rotation interval from `NYX_LAB_ROTATE_SECS`. LAB-ONLY override: the leak lab sets
/// it so its rotation scenario sees a timer-driven swap within seconds instead of the settings'
/// whole minutes (AppState::set_rotation_interval). Unset = no override; zero, a non-number or
/// non-UTF-8 is an error. Only the daemon reads this variable.
pub fn lab_rotate_every(var: Option<&std::ffi::OsStr>) -> Result<Option<std::time::Duration>, String> {
    positive_secs("NYX_LAB_ROTATE_SECS", var)
}

/// A lab env value (`name` for the message) as a positive whole number of seconds; `None` when
/// unset. Anything else is an error, never a silent fallback.
fn positive_secs(name: &str, var: Option<&std::ffi::OsStr>) -> Result<Option<std::time::Duration>, String> {
    let Some(v) = var else { return Ok(None) };
    let v = v.to_str().ok_or_else(|| format!("{name}={v:?} is not valid UTF-8"))?;
    let secs: u64 = v.parse().ok().filter(|s| *s > 0).ok_or_else(|| format!("{name}={v:?} is not a positive number of seconds"))?;
    Ok(Some(std::time::Duration::from_secs(secs)))
}
