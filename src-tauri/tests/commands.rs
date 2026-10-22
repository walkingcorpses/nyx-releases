//! Task 9 tests: the command layer (AppState) applies settings to a real engine and
//! refuses unsafe save paths.

use nyx_core::{EncryptionPref, ProxySettings, Settings};
use nyx_engine::testkit::free_loopback_addr;
use nyx_engine::EncryptionMode;
use nyx_lib::state::AppState;

/// An app state on loopback with no discovery, storing everything in `dir`.
fn app(dir: &std::path::Path) -> AppState {
    AppState::new(dir, free_loopback_addr(), false).unwrap()
}

/// set_settings reaches the engine: encryption policy changes and a proxy turns off LSD/UPnP/NAT-PMP
/// (we start with discovery on so there is something to turn off).
#[test]
fn settings_command_applies_encryption_and_proxy_to_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let a = AppState::new(tmp.path(), free_loopback_addr(), true).unwrap();
    assert_eq!(a.engine.encryption_mode(), EncryptionMode::Forced);
    assert!(a.engine.discovery_flags().lsd);

    let mut s = a.get_settings();
    s.encryption = EncryptionPref::Prefer;
    s.proxy = Some(ProxySettings { host: "127.0.0.1".into(), port: 1080, username: None, password: None });
    a.set_settings(s).unwrap();
    assert_eq!(a.engine.encryption_mode(), EncryptionMode::Prefer);
    let f = a.engine.discovery_flags();
    assert!(!f.lsd && !f.upnp && !f.natpmp);

    // The change was saved to disk too.
    assert_eq!(Settings::load(tmp.path()).encryption, EncryptionPref::Prefer);

    let mut s = a.get_settings();
    s.proxy = None;
    s.encryption = EncryptionPref::Forced;
    a.set_settings(s).unwrap();
    assert_eq!(a.engine.encryption_mode(), EncryptionMode::Forced);
    assert!(a.engine.discovery_flags().lsd);
}

/// Bad settings are rejected and change nothing.
#[test]
fn invalid_settings_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(tmp.path());
    let mut s = a.get_settings();
    s.proxy = Some(ProxySettings { host: "".into(), port: 0, username: None, password: None });
    assert!(a.set_settings(s).is_err());
    let mut s = a.get_settings();
    s.max_active_downloads = 0;
    assert!(a.set_settings(s).is_err());
    // Nothing was saved by the rejected calls.
    assert!(!tmp.path().join("settings.json").exists());
}

/// A save path that climbs out, names a reserved device, or is not a full local path is refused
/// before anything reaches the engine.
#[test]
fn command_rejects_unsafe_save_path() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(tmp.path());
    let magnet = format!("magnet:?xt=urn:btih:{}", "ab".repeat(20));
    for bad in ["D:\\dl\\..\\..\\Windows", "D:\\CON", "relative\\dir", "\\\\server\\share\\x"] {
        let err = a.add_magnet(&magnet, bad, None, false).unwrap_err();
        assert!(!err.is_empty(), "{bad}");
        assert!(a.list_torrents().is_empty(), "{bad} reached the engine");
    }
    // A fine path works.
    let ok = tmp.path().join("dl").to_string_lossy().into_owned();
    a.add_magnet(&magnet, &ok, None, false).unwrap();
    assert_eq!(a.list_torrents().len(), 1);
}
