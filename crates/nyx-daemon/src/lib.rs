//! nyx-daemon library half: the control protocol lives in `ctl` so tests can call it without sockets.
//! The binary (`main.rs`) adds the process wiring: env, tick loop, TCP listener, `ctl` client.

pub mod ctl;
