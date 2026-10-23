// settings.rs - the user's settings, saved as settings.json. Pure data plus load/save;
// the app applies them to the engine. A damaged file never blocks startup.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::categories::CategoryDef;
use crate::scheduler::Schedule;

/// File name inside the data folder.
const FILE: &str = "settings.json";

/// Peer encryption choice. Forced refuses plaintext peers; Prefer allows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionPref {
    Forced,
    Prefer,
}

/// SOCKS5 proxy. Present means "use it"; absent means direct connections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySettings {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// One RSS feed Nyx polls for new torrents. Items whose title matches `filter` (a regular
/// expression, case-insensitive; empty matches everything) are added automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub name: String,
    pub url: String,
    pub filter: String,
    pub category: Option<String>,
    pub enabled: bool,
}

/// RSS auto-download settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RssSettings {
    pub feeds: Vec<Feed>,
    /// Minutes between polls (at least 5).
    pub interval_minutes: u32,
}

impl Default for RssSettings {
    fn default() -> Self {
        RssSettings { feeds: Vec::new(), interval_minutes: 30 }
    }
}

/// Everything the settings page edits. `#[serde(default)]` lets older files that lack newer
/// fields still load. Rate limits are bytes per second, 0 = unlimited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub encryption: EncryptionPref,
    pub proxy: Option<ProxySettings>,
    pub max_active_downloads: usize,
    pub max_active_seeds: usize,
    pub down_limit: u64,
    pub up_limit: u64,
    pub schedule: Schedule,
    pub categories: Vec<CategoryDef>,
    pub default_save_dir: PathBuf,
    pub start_minimized: bool,
    pub minimize_to_tray: bool,
    /// While a proxy is set: pause everything when the proxy cannot be reached.
    pub kill_switch: bool,
    /// Network adapter (name or IP) to bind to, e.g. a VPN adapter. If it disappears, nothing connects.
    pub bind_interface: Option<String>,
    /// Path of an IP blocklist file (PeerGuardian, plain ranges or CIDR), applied to peers.
    pub blocklist: Option<PathBuf>,
    /// Ask GitHub for a newer Nyx at launch. Off by default: Nyx makes no other non-torrent connections.
    pub check_updates: bool,
    pub rss: RssSettings,
    /// Search provider: an http(s) URL containing `{query}` that returns an RSS/Torznab feed.
    pub search_url: String,
}

impl Default for Settings {
    /// Forced encryption, no proxy, three downloads and three seeds at once, save to the
    /// user's Downloads folder.
    fn default() -> Self {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        Settings {
            encryption: EncryptionPref::Forced,
            proxy: None,
            max_active_downloads: 3,
            max_active_seeds: 3,
            down_limit: 0,
            up_limit: 0,
            schedule: Schedule::default(),
            categories: Vec::new(),
            default_save_dir: home.join("Downloads"),
            start_minimized: false,
            minimize_to_tray: true,
            kill_switch: true,
            bind_interface: None,
            blocklist: None,
            check_updates: false,
            rss: RssSettings::default(),
            search_url: String::new(),
        }
    }
}

impl Settings {
    /// Reads `settings.json` from `dir`. A missing file gives defaults. A corrupt file gives
    /// defaults too, and is kept as `settings.json.bad` so the user can recover by hand.
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        let Ok(text) = std::fs::read_to_string(&path) else { return Settings::default() };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(_) => {
                let _ = std::fs::rename(&path, dir.join(format!("{FILE}.bad")));
                Settings::default()
            }
        }
    }

    /// Saves to `dir/settings.json` through a temp file and rename, so a crash mid-write
    /// leaves the old file intact.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }
}

/// Where Nyx keeps its data: beside the exe when a `nyx.portable` marker file sits there,
/// otherwise `%APPDATA%\Nyx`.
pub fn data_dir(exe_dir: &Path, appdata: &Path) -> PathBuf {
    if exe_dir.join("nyx.portable").exists() {
        exe_dir.to_path_buf()
    } else {
        appdata.join("Nyx")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.encryption = EncryptionPref::Prefer;
        s.max_active_downloads = 7;
        s.proxy = Some(ProxySettings { host: "h".into(), port: 1080, username: Some("u".into()), password: None });
        s.save(tmp.path()).unwrap();
        assert_eq!(Settings::load(tmp.path()), s);
    }

    #[test]
    fn missing_file_gives_defaults_and_defaults_are_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Settings::load(tmp.path());
        assert_eq!(s.encryption, EncryptionPref::Forced);
        assert_eq!((s.max_active_downloads, s.max_active_seeds), (3, 3));
        assert!(!tmp.path().join("settings.json.bad").exists());
    }

    #[test]
    fn corrupt_settings_fall_back_and_keep_bad_copy() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(Settings::load(tmp.path()), Settings::default());
        assert_eq!(std::fs::read_to_string(tmp.path().join("settings.json.bad")).unwrap(), "{ not json");
    }

    #[test]
    fn partial_file_fills_missing_fields_with_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), r#"{"max_active_seeds": 9}"#).unwrap();
        let s = Settings::load(tmp.path());
        assert_eq!(s.max_active_seeds, 9);
        assert_eq!(s.max_active_downloads, 3);
    }

    #[test]
    fn portable_marker_selects_exe_dir() {
        let exe = tempfile::tempdir().unwrap();
        let appdata = Path::new("C:/Users/x/AppData/Roaming");
        assert_eq!(data_dir(exe.path(), appdata), appdata.join("Nyx"));
        std::fs::write(exe.path().join("nyx.portable"), "").unwrap();
        assert_eq!(data_dir(exe.path(), appdata), exe.path());
    }
}
