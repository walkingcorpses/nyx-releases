//! Task 13 tests: single instance, argument forwarding, tray/minimized behaviour. These start the
//! real release exe (build it first: `cargo build --release -p nyx --features custom-protocol`, or
//! point NYX_EXE at one). Every instance runs off-screen with its own data folder, profile and
//! window-title check, and only processes this test spawned are ever killed (by handle).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

/// Path of the exe under test.
fn exe() -> PathBuf {
    if let Some(p) = std::env::var_os("NYX_EXE") {
        return PathBuf::from(p);
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("target").join("release").join("nyx.exe");
    assert!(p.exists(), "build the release exe first (see the file header): {}", p.display());
    p
}

/// One test instance: a spawned Nyx with its own folder. Killed on drop (only this child).
struct Instance {
    child: Child,
    dir: tempfile::TempDir,
}

impl Instance {
    /// Starts Nyx off-screen with an isolated data folder; `settings_json` pre-seeds settings.json.
    fn start(settings_json: Option<&str>) -> Instance {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        if let Some(s) = settings_json {
            std::fs::write(data.join("settings.json"), s).unwrap();
        }
        let child = spawn(dir.path(), &[]);
        let inst = Instance { child, dir };
        // Ready once the engine has created its state folder (setup finished).
        assert!(wait(Duration::from_secs(30), || data.join("engine").join("resume").exists()), "instance did not start");
        std::thread::sleep(Duration::from_millis(500));
        inst
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("log.txt")).unwrap_or_default()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawns the exe with the isolating environment; `args` are its command-line arguments.
fn spawn(dir: &Path, args: &[&str]) -> Child {
    Command::new(exe())
        .args(args)
        .env("NYX_DATA_DIR", dir.join("data"))
        .env("NYX_LISTEN", "127.0.0.1:0")
        .env("NYX_NO_DISCOVERY", "1")
        .env("NYX_TEST_OFFSCREEN", "1")
        .env("NYX_TEST_LOG", dir.join("log.txt"))
        .env("WEBVIEW2_USER_DATA_FOLDER", dir.join("webview"))
        .env("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "--mute-audio")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not start nyx.exe")
}

/// Polls until `cond` or timeout.
fn wait(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cond()
}

/// (visible, hidden) counts of top-level windows titled "Nyx" owned by any of `pids`.
fn nyx_windows(pids: &[u32]) -> (usize, usize) {
    struct Ctx<'a> {
        pids: &'a [u32],
        seen: (usize, usize),
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        // SAFETY: lp is the &mut Ctx passed by nyx_windows below and outlives the EnumWindows call.
        let ctx = unsafe { &mut *(lp as *mut Ctx) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if ctx.pids.contains(&pid) {
            let mut buf = [0u16; 16];
            let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) } as usize;
            if String::from_utf16_lossy(&buf[..n]) == "Nyx" {
                if unsafe { IsWindowVisible(hwnd) } != 0 { ctx.seen.0 += 1 } else { ctx.seen.1 += 1 }
            }
        }
        1
    }
    let mut ctx = Ctx { pids, seen: (0, 0) };
    // SAFETY: `each` matches WNDENUMPROC and ctx lives for the whole synchronous call.
    unsafe { EnumWindows(Some(each), &mut ctx as *mut Ctx as LPARAM) };
    ctx.seen
}

const MAGNET: &str = "magnet:?xt=urn:btih:abababababababababababababababababababab&dn=Forwarded";

/// A second launch with a magnet hands it to the first instance, exits 0 quickly, and no
/// second window appears.
#[test]
fn second_launch_with_magnet_is_forwarded_and_exits() {
    let first = Instance::start(None);
    let mut second = spawn(first.dir.path(), &[MAGNET]);
    let spid = second.id();
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = second.try_wait().unwrap() {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "second launch did not exit within 5 s");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "second launch exited with {status}");
    assert!(wait(Duration::from_secs(5), || first.log().contains(&format!("forwarded: {MAGNET}"))), "first never saw the magnet");
    assert!(first.child.id() == first.pid());
    let (visible, hidden) = nyx_windows(&[first.pid(), spid]);
    assert_eq!(visible + hidden, 1, "expected exactly one Nyx window, found {visible} visible + {hidden} hidden");
}

/// A launch while the first instance sits hidden in the tray reaches it and brings the window up.
#[test]
fn launch_while_first_is_minimized_to_tray_forwards() {
    let first = Instance::start(Some(r#"{"start_minimized": true}"#));
    assert_eq!(nyx_windows(&[first.pid()]), (0, 1), "window should start hidden");
    let mut second = spawn(first.dir.path(), &[MAGNET]);
    assert!(second.wait().unwrap().success());
    assert!(wait(Duration::from_secs(5), || first.log().contains("forwarded: magnet:")));
    assert!(wait(Duration::from_secs(5), || nyx_windows(&[first.pid()]) == (1, 0)), "window was not shown");
}

/// With "start minimized" the window exists but is hidden; without it the window is shown.
#[test]
fn start_minimized_hides_window() {
    let hidden = Instance::start(Some(r#"{"start_minimized": true}"#));
    assert_eq!(nyx_windows(&[hidden.pid()]), (0, 1));
    let shown = Instance::start(None);
    assert_eq!(nyx_windows(&[shown.pid()]), (1, 0));
}
