//! nyx-engine: the only crate that talks to libtorrent. Exposes a safe Rust API
//! over the C++ shim in `cpp/`; everything above this crate stays libtorrent-free.

mod ffi;
mod types;

#[cfg(feature = "test-support")]
pub mod testkit;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock, RwLockReadGuard};

use cxx::UniquePtr;
use nyx_core::{check_torrent_bytes, parse_magnet, safe_join};

pub use types::*;

/// Returns the linked libtorrent-rasterbar version, e.g. "2.0.11.0".
/// Used by the toolchain spike and for the About screen.
pub fn libtorrent_version() -> String {
    ffi::lt_version()
}

/// Starts a real libtorrent session (loopback only, no DHT/LSD/UPnP/NAT-PMP)
/// and shuts it down. Proves the full engine links and runs, not just headers.
pub fn session_smoke() -> bool {
    ffi::lt_session_smoke()
}

/// Parses a `.torrent` for the pre-download file tree without adding it.
/// Runs the untrusted-input checks first and rejects any file path that could escape a save folder.
pub fn inspect_torrent(bytes: &[u8]) -> Result<TorrentMeta, EngineError> {
    check_torrent_bytes(bytes)?;
    let m = ffi::inspect_torrent(bytes)?;
    // Probe against a fixed base: only the relative path matters, not where it lands.
    let base = Path::new("base");
    for f in &m.files {
        safe_join(base, &f.path)?;
    }
    Ok(TorrentMeta {
        id: m.id,
        name: m.name,
        files: m.files.into_iter().map(file_info).collect(),
    })
}

/// Per-torrent facts libtorrent does not keep for us: category, queue hold, and an
/// error to show when the save folder could not be prepared.
#[derive(Default)]
struct Meta {
    categories: HashMap<TorrentId, String>,
    held: HashSet<TorrentId>,
    /// id -> (save folder, message) for torrents whose folder could not be created.
    save_errors: HashMap<TorrentId, (PathBuf, String)>,
}

/// The torrent engine: one libtorrent session plus Nyx's per-torrent metadata.
pub struct Engine {
    /// The session. Every call takes the read side; `rotate_identity` takes the write side to swap
    /// in a new session, so no call ever sees one half-built or already destroyed.
    inner: RwLock<UniquePtr<ffi::Session>>,
    meta: Mutex<Meta>,
    state_dir: PathBuf,
}

// SAFETY: lt::session is documented thread-safe, every Session method is const and goes
// through its API, and our own state sits behind a Mutex.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    /// Starts a session with forced encryption and restores saved torrents from `state_dir`.
    pub fn new(cfg: EngineConfig) -> Result<Engine, EngineError> {
        let io_err = |e: io::Error| EngineError::Backend(e.to_string());
        fs::create_dir_all(cfg.state_dir.join("resume")).map_err(io_err)?;
        let dht = fs::read(cfg.state_dir.join("session.dat")).unwrap_or_default();
        let p = cfg.proxy.as_ref();
        let init = ffi::SessionInit {
            listen: cfg.listen.to_string(),
            discovery: cfg.discovery,
            encryption: match cfg.encryption { EncryptionMode::Forced => 0, EncryptionMode::Prefer => 1 },
            proxy_enabled: p.is_some(),
            proxy_host: p.map(|c| c.host.clone()).unwrap_or_default(),
            proxy_port: p.map(|c| c.port).unwrap_or(0),
            proxy_user: p.and_then(|c| c.username.clone()).unwrap_or_default(),
            proxy_pass: p.and_then(|c| c.password.clone()).unwrap_or_default(),
            interface: cfg.interface.clone().unwrap_or_default(),
        };
        let inner = ffi::new_session(&dht, &init)?;
        let engine = Engine { inner: RwLock::new(inner), meta: Mutex::new(Meta::default()), state_dir: cfg.state_dir };
        // Blocklist before any torrent exists, so a restored torrent never talks to a blocked address.
        engine.set_ip_filter(&cfg.blocklist)?;
        engine.restore();
        Ok(engine)
    }

    /// The current session, for one call (read lock held while the guard lives).
    pub(crate) fn ses(&self) -> RwLockReadGuard<'_, UniquePtr<ffi::Session>> {
        self.inner.read().unwrap()
    }

    /// A new identity for trackers and peers (spec 7.3 check 3): every torrent gets a fresh
    /// tracker peer_id and key, as if a different client had started. Called after each VPN
    /// rotation, once the engine already points at the new tunnel. libtorrent fixes both for a
    /// torrent's lifetime, so this builds a new session from the live one's settings (same proxy,
    /// adapter, encryption and limits from its first socket) and its torrents' resume data
    /// (progress, paused flags, limits, priorities); categories and queue holds live here and stay.
    /// The old session is silenced before anything is saved (its stopped announces cannot reach
    /// a tracker) and torn down after the swap, outside the lock. Blocks other engine calls for
    /// the save and the rebuild (normally well under a second, at most the 5 s resume wait). On
    /// error nothing changes: the old session is restored as it was, still on its proxy.
    pub fn rotate_identity(&self) -> Result<(), EngineError> {
        let old = {
            let mut ses = self.inner.write().unwrap();
            let fresh = ses.respawn(5000)?;
            std::mem::replace(&mut *ses, fresh)
        };
        drop(old);
        Ok(())
    }

    /// Adds a magnet link. The torrent fetches metadata but downloads nothing until the caller
    /// sets file priorities and calls `resume` (it is paused as soon as its metadata arrives).
    /// Adding the same torrent again returns the same id.
    pub fn add_magnet(&self, uri: &str, opts: AddOptions) -> Result<TorrentId, EngineError> {
        parse_magnet(uri)?;
        let existing = self.ids();
        let save_err = prepare_save_dir(&opts.save_path);
        let paused = opts.paused || save_err.is_some();
        let id = self.ses().add_magnet(uri, &path_str(&opts.save_path), paused, opts.sequential)?;
        self.after_add(&id, &opts, save_err, &existing);
        Ok(id)
    }

    /// Adds a `.torrent` file's bytes. Adding the same torrent again returns the same id.
    pub fn add_torrent_file(&self, bytes: &[u8], opts: AddOptions) -> Result<TorrentId, EngineError> {
        inspect_torrent(bytes)?;
        let existing = self.ids();
        let save_err = prepare_save_dir(&opts.save_path);
        let paused = opts.paused || save_err.is_some();
        let id = self.ses().add_torrent(bytes, &path_str(&opts.save_path), paused, opts.sequential)?;
        self.after_add(&id, &opts, save_err, &existing);
        Ok(id)
    }

    /// Records category and any save-folder error for a freshly added torrent.
    /// A repeat add of an existing torrent leaves its state alone.
    fn after_add(&self, id: &TorrentId, opts: &AddOptions, save_err: Option<String>, existing: &HashSet<TorrentId>) {
        if existing.contains(id) {
            return;
        }
        let mut m = self.meta.lock().unwrap();
        if let Some(c) = &opts.category {
            m.categories.insert(id.clone(), c.clone());
        }
        if let Some(msg) = save_err {
            m.save_errors.insert(id.clone(), (opts.save_path.clone(), msg));
        }
    }

    /// All torrent ids currently in the session.
    fn ids(&self) -> HashSet<TorrentId> {
        self.ses().snapshot().map(|v| v.into_iter().map(|r| r.id).collect()).unwrap_or_default()
    }

    /// State of every torrent, for the list view. Never panics on libtorrent errors:
    /// a torrent with a problem shows `Error(message)`.
    pub fn snapshot(&self) -> Vec<TorrentState> {
        let rows = self.ses().snapshot().unwrap_or_default();
        let m = self.meta.lock().unwrap();
        rows.into_iter()
            .map(|r| {
                let status = if let Some((_, msg)) = m.save_errors.get(&r.id) {
                    TorrentStatus::Error(msg.clone())
                } else if !r.error.is_empty() {
                    TorrentStatus::Error(r.error.clone())
                } else if r.paused {
                    if m.held.contains(&r.id) { TorrentStatus::Queued } else { TorrentStatus::Paused }
                } else {
                    match r.state {
                        0 => TorrentStatus::Metadata,
                        1 => TorrentStatus::Checking,
                        3 => TorrentStatus::Seeding,
                        _ => TorrentStatus::Downloading,
                    }
                };
                TorrentState {
                    category: m.categories.get(&r.id).cloned(),
                    id: r.id,
                    name: r.name,
                    status,
                    progress: r.progress,
                    down_rate: r.down_rate,
                    up_rate: r.up_rate,
                    size: r.size,
                    peers: r.peers,
                    save_path: PathBuf::from(r.save_path),
                }
            })
            .collect()
    }

    /// User pause. Clears any queue hold so the torrent reads as `Paused`, not `Queued`.
    pub fn pause(&self, id: &TorrentId) -> Result<(), EngineError> {
        self.meta.lock().unwrap().held.remove(id);
        Ok(self.ses().pause(id)?)
    }

    /// User resume. If the save folder was the problem, retries creating it first.
    pub fn resume(&self, id: &TorrentId) -> Result<(), EngineError> {
        let retry = self.meta.lock().unwrap().save_errors.get(id).map(|(p, _)| p.clone());
        if let Some(path) = retry {
            match prepare_save_dir(&path) {
                Some(msg) => {
                    self.meta.lock().unwrap().save_errors.insert(id.clone(), (path, msg));
                    return Ok(());
                }
                None => {
                    self.meta.lock().unwrap().save_errors.remove(id);
                }
            }
        }
        self.meta.lock().unwrap().held.remove(id);
        self.ses().unpark(id)?;
        Ok(self.ses().resume(id)?)
    }

    /// Queue hold: pauses (`on`) or releases (`!on`) a torrent without it counting as a user pause.
    pub fn set_queued(&self, id: &TorrentId, on: bool) -> Result<(), EngineError> {
        {
            let mut m = self.meta.lock().unwrap();
            if on { m.held.insert(id.clone()); } else { m.held.remove(id); }
        }
        if on { Ok(self.ses().pause(id)?) } else { Ok(self.ses().resume(id)?) }
    }

    /// Removes a torrent, optionally deleting its downloaded files, and forgets its saved state.
    pub fn remove(&self, id: &TorrentId, delete_files: bool) -> Result<(), EngineError> {
        self.ses().remove(id, delete_files)?;
        let mut m = self.meta.lock().unwrap();
        m.categories.remove(id);
        m.held.remove(id);
        m.save_errors.remove(id);
        let _ = fs::remove_file(self.resume_path(id));
        Ok(())
    }

    /// Re-hashes the torrent's files from scratch.
    pub fn recheck(&self, id: &TorrentId) -> Result<(), EngineError> {
        Ok(self.ses().recheck(id)?)
    }

    /// Per-file priorities, in file order: 0 skips the file, 1 to 7 downloads it (higher first).
    pub fn set_file_priorities(&self, id: &TorrentId, priorities: &[u8]) -> Result<(), EngineError> {
        Ok(self.ses().set_priorities(id, priorities)?)
    }

    /// Sequential download on or off (pieces in order, for streaming).
    pub fn set_sequential(&self, id: &TorrentId, on: bool) -> Result<(), EngineError> {
        Ok(self.ses().set_sequential(id, on)?)
    }

    /// Replaces the peer blocklist: connections to and from these ranges are refused. An empty
    /// list clears it.
    pub fn set_ip_filter(&self, ranges: &[nyx_core::IpRange]) -> Result<(), EngineError> {
        let spans: Vec<ffi::IpSpan> =
            ranges.iter().map(|r| ffi::IpSpan { first: r.first.to_string(), last: r.last.to_string() }).collect();
        Ok(self.ses().set_ip_filter(&spans)?)
    }

    /// Binds to one network adapter (name or IP), or unbinds with `None`.
    pub fn set_interface(&self, name: Option<&str>) -> Result<(), EngineError> {
        Ok(self.ses().set_interface(name.unwrap_or(""))?)
    }

    /// Changes the save folder of a torrent (created first). For a magnet still fetching metadata
    /// this just sets the path. Errors creating the folder come back as `Backend`.
    pub fn set_save_path(&self, id: &TorrentId, path: &Path) -> Result<(), EngineError> {
        if let Some(msg) = prepare_save_dir(path) {
            return Err(EngineError::Backend(msg));
        }
        self.meta.lock().unwrap().save_errors.remove(id);
        Ok(self.ses().move_storage(id, &path_str(path))?)
    }

    /// Sets or clears the torrent's category label (the folder is chosen separately).
    pub fn set_category(&self, id: &TorrentId, category: Option<String>) {
        let mut m = self.meta.lock().unwrap();
        match category {
            Some(c) => m.categories.insert(id.clone(), c),
            None => m.categories.remove(id),
        };
    }

    /// Rate caps in bytes per second, 0 = unlimited. `None` sets the global caps.
    pub fn set_limits(&self, id: Option<&TorrentId>, down: u64, up: u64) -> Result<(), EngineError> {
        match id {
            Some(id) => Ok(self.ses().set_torrent_limits(id, down, up)?),
            None => Ok(self.ses().set_global_limits(down, up)?),
        }
    }

    /// Files, peers and trackers of one torrent, for the detail drawer.
    pub fn torrent_detail(&self, id: &TorrentId) -> Result<TorrentDetail, EngineError> {
        let files = self.ses().files(id)?.into_iter().map(file_info).collect();
        let peers = self
            .ses()
            .peers(id)?
            .into_iter()
            .filter_map(|p| {
                Some(PeerInfo {
                    addr: p.addr.parse::<SocketAddr>().ok()?,
                    client: p.client,
                    down_rate: p.down_rate,
                    up_rate: p.up_rate,
                    flags: p.flags,
                    encrypted: p.encrypted,
                })
            })
            .collect();
        let trackers = self
            .ses()
            .trackers(id)?
            .into_iter()
            .map(|t| TrackerInfo { url: t.url, status: t.status })
            .collect();
        let lim = self.ses().torrent_limits(id)?;
        Ok(TorrentDetail { files, peers, trackers, down_limit: lim.down, up_limit: lim.up })
    }

    /// Sets peer encryption: `Forced` refuses plaintext peers, `Prefer` accepts them.
    pub fn set_encryption(&self, mode: EncryptionMode) -> Result<(), EngineError> {
        let m = match mode { EncryptionMode::Forced => 0, EncryptionMode::Prefer => 1 };
        Ok(self.ses().set_encryption(m)?)
    }

    /// Routes peers, trackers and hostname lookups through a SOCKS5 proxy and turns off
    /// LSD, UPnP and NAT-PMP. `None` goes back to direct connections.
    pub fn set_proxy(&self, cfg: Option<ProxyConfig>) -> Result<(), EngineError> {
        match cfg {
            Some(c) => Ok(self.ses().set_proxy(
                true,
                &c.host,
                c.port,
                c.username.as_deref().unwrap_or(""),
                c.password.as_deref().unwrap_or(""),
            )?),
            None => Ok(self.ses().set_proxy(false, "", 0, "", "")?),
        }
    }

    /// The encryption policy libtorrent currently has (read back, not remembered).
    /// Anything other than the two user modes (test-only plaintext) reads as `Prefer`.
    pub fn encryption_mode(&self) -> EncryptionMode {
        if self.ses().encryption_mode() == 0 { EncryptionMode::Forced } else { EncryptionMode::Prefer }
    }

    /// DHT, LSD, UPnP and NAT-PMP as libtorrent currently has them.
    pub fn discovery_flags(&self) -> DiscoveryFlags {
        let f = self.ses().discovery_flags();
        DiscoveryFlags { dht: f.dht, lsd: f.lsd, upnp: f.upnp, natpmp: f.natpmp }
    }

    /// Writes resume data, DHT state and per-torrent metadata to `state_dir` so the next
    /// `Engine::new` on the same folder picks up where this one stopped.
    pub fn save_state(&self) -> Result<(), EngineError> {
        let io_err = |e: io::Error| EngineError::Backend(e.to_string());
        let dir = self.state_dir.join("resume");
        fs::create_dir_all(&dir).map_err(io_err)?;
        for blob in self.ses().save_resume(5000)? {
            write_atomic(&self.resume_path(&blob.id), &blob.data).map_err(io_err)?;
        }
        write_atomic(&self.state_dir.join("session.dat"), &self.ses().save_session_state()).map_err(io_err)?;
        let m = self.meta.lock().unwrap();
        let mut ids: Vec<&TorrentId> = m.categories.keys().chain(m.held.iter()).collect();
        ids.sort();
        ids.dedup();
        let tsv: String = ids
            .iter()
            .map(|id| {
                let cat = m.categories.get(*id).map(String::as_str).unwrap_or("");
                format!("{id}\t{cat}\t{}\n", u8::from(m.held.contains(*id)))
            })
            .collect();
        write_atomic(&self.state_dir.join("torrents.tsv"), tsv.as_bytes()).map_err(io_err)?;
        Ok(())
    }

    /// Re-adds every torrent with saved resume data and reloads categories and holds.
    /// A bad resume file is skipped, never fatal.
    fn restore(&self) {
        if let Ok(text) = fs::read_to_string(self.state_dir.join("torrents.tsv")) {
            let mut m = self.meta.lock().unwrap();
            for line in text.lines() {
                let mut p = line.split('\t');
                let (Some(id), Some(cat), Some(held)) = (p.next(), p.next(), p.next()) else { continue };
                if !cat.is_empty() {
                    m.categories.insert(id.to_string(), cat.to_string());
                }
                if held == "1" {
                    m.held.insert(id.to_string());
                }
            }
        }
        let Ok(entries) = fs::read_dir(self.state_dir.join("resume")) else { return };
        for e in entries.flatten() {
            if let Ok(bytes) = fs::read(e.path()) {
                let _ = self.ses().add_resume(&bytes);
            }
        }
    }

    /// Path of one torrent's resume file.
    fn resume_path(&self, id: &str) -> PathBuf {
        self.state_dir.join("resume").join(format!("{id}.resume"))
    }
}

/// Converts the bridge's file row to the public type.
fn file_info(f: ffi::FileRow) -> FileInfo {
    FileInfo { path: f.path, size: f.size, progress: f.progress, priority: f.priority }
}

/// Path as a UTF-8 string for libtorrent (lossy only for non-Unicode paths, which Windows rarely has).
fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Makes sure the save folder exists. Returns an error message when it cannot be created
/// (path is a file, no permission, bad drive), so the torrent can show it instead of crashing.
fn prepare_save_dir(p: &Path) -> Option<String> {
    match fs::create_dir_all(p) {
        Ok(()) if p.is_dir() => None,
        Ok(()) => Some(format!("Save location is not a folder: {}", p.display())),
        Err(e) => Some(format!("Cannot use save folder {}: {e}", p.display())),
    }
}

/// Writes `data` to a temp file beside `path`, then renames it over `path`, so a crash
/// mid-write never leaves a half-written file.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}
