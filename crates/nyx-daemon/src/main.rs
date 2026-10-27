//! nyx-daemon: headless Nyx for the Docker leak lab.
//!   nyx-daemon run            start the engine; serve the control port on 127.0.0.1:7777
//!   nyx-daemon ctl '<json>'   send one request to a running daemon and print the reply
//!   nyx-daemon --version      print "nyx-daemon <version>"
//! Env for `run`: NYX_DATA_DIR (required), NYX_LISTEN (default 0.0.0.0:6881), NYX_NO_DISCOVERY (any value).

use nyx_app::state::AppState;
use nyx_daemon::ctl::{handle, is_shutdown};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Loopback only, so the port never shows on a container's network interface.
const CTL_ADDR: &str = "127.0.0.1:7777";
/// Save resume data this often (ticks of one second), same as the desktop app.
const SAVE_EVERY_TICKS: u32 = 60;

/// Dispatches the subcommand; any failure prints to stderr and exits 1.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--version") => {
            println!("nyx-daemon {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("run") => run(),
        Some("ctl") if args.len() == 2 => ctl_client(&args[1]),
        _ => Err("usage: nyx-daemon run | ctl '<json>' | --version".to_string()),
    };
    if let Err(e) = result {
        eprintln!("nyx-daemon: {e}");
        std::process::exit(1);
    }
}

/// Builds the app from env, starts the background threads and blocks serving the control port.
fn run() -> Result<(), String> {
    let data = std::env::var_os("NYX_DATA_DIR").ok_or("NYX_DATA_DIR is not set")?;
    let listen = std::env::var("NYX_LISTEN").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| "0.0.0.0:6881".parse().unwrap());
    let discovery = std::env::var_os("NYX_NO_DISCOVERY").is_none();
    let state = Arc::new(AppState::new(std::path::Path::new(&data), listen, discovery)?);
    spawn_poller(state.clone());
    spawn_background(state.clone());
    let listener = TcpListener::bind(CTL_ADDR).map_err(|e| format!("cannot bind {CTL_ADDR}: {e}"))?;
    for conn in listener.incoming().flatten() {
        let s = state.clone();
        std::thread::spawn(move || serve(&s, conn));
    }
    Ok(())
}

/// One control connection: answer each request line; after `shutdown` has been answered, exit the process.
fn serve(state: &AppState, conn: TcpStream) {
    let Ok(mut out) = conn.try_clone() else { return };
    for line in BufReader::new(conn).lines().map_while(Result::ok) {
        let reply = handle(state, &line);
        if writeln!(out, "{reply}").is_err() {
            return;
        }
        if is_shutdown(&line) {
            std::process::exit(0);
        }
    }
}

/// The `ctl` client: one request line out, one reply line printed. Exit 1 if the reply is an error.
fn ctl_client(json: &str) -> Result<(), String> {
    let mut conn = TcpStream::connect(CTL_ADDR).map_err(|e| format!("cannot reach daemon on {CTL_ADDR}: {e}"))?;
    writeln!(conn, "{}", json.replace('\n', " ")).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply).map_err(|e| e.to_string())?;
    print!("{reply}");
    if reply.starts_with("{\"err\"") {
        std::process::exit(1);
    }
    Ok(())
}

/// Once a second: poll the engine and every minute save resume data. No UI events or notifications here.
fn spawn_poller(state: Arc<AppState>) {
    std::thread::spawn(move || {
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let _ = state.tick(chrono::Local::now().naive_local());
            ticks += 1;
            if ticks % SAVE_EVERY_TICKS == 0 {
                let _ = state.engine.save_state();
            }
        }
    });
}

/// Slow jobs, same cadence as the desktop app: proxy probe every 5 s (feeds the kill switch) and RSS
/// polling on its interval. Waking every 30 s keeps the RSS check cheap.
fn spawn_background(state: Arc<AppState>) {
    let s = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        s.probe_proxy();
    });
    std::thread::spawn(move || {
        let mut last = std::time::Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let cfg = state.get_settings().rss;
            let due = last.elapsed() >= Duration::from_secs(u64::from(cfg.interval_minutes.max(5)) * 60);
            if due && cfg.feeds.iter().any(|f| f.enabled) {
                last = std::time::Instant::now();
                let _ = state.rss_run();
            }
        }
    });
}
