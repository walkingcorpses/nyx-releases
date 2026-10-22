// state.rs - the app's brain, free of any Tauri types so it can be tested directly:
// owns the engine, the settings and the queue, and implements every UI command.
// commands.rs only forwards Tauri calls to the methods here.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Mutex;

use chrono::NaiveDateTime;
use nyx_core::{
    build_magnet, safe_join, Categories, EncryptionPref, Queue, QueueEntry, Settings, TorrentId,
};
use nyx_engine::{
    AddOptions, Engine, EngineConfig, EncryptionMode, ProxyConfig, TorrentState, TorrentStatus,
};

use crate::dto::{DetailDto, MetaDto, TorrentDto};
use crate::poller::{completions, diff, Update};

/// Largest .torrent file the app will even open (matches nyx-core's limit).
const MAX_TORRENT_FILE: u64 = 10 * 1024 * 1024;

/// What one poll tick produced: the UI delta, and names of downloads that just finished.
pub struct Tick {
    pub update: Update,
    pub completed: Vec<String>,
}

/// Bookkeeping the poller keeps between ticks.
#[derive(Default)]
struct PollState {
    prev: HashMap<TorrentId, TorrentState>,
    /// First-seen order of torrents: the queue position.
    order: Vec<TorrentId>,
    /// Caps last handed to the engine, so we only call it when they change.
    last_limits: Option<(u64, u64)>,
}

/// Everything the running app needs. Shared as `Arc<AppState>` between commands, the poller
/// thread, the tray and the instance handler.
pub struct AppState {
    pub engine: Engine,
    pub data_dir: PathBuf,
    settings: Mutex<Settings>,
    queue: Mutex<Queue>,
    poll: Mutex<PollState>,
    /// Magnet links and .torrent paths that arrived (argv, second launch) and the UI has not
    /// picked up yet.
    pending_args: Mutex<Vec<String>>,
}

impl AppState {
    /// Loads settings from `data_dir`, starts the engine on `listen` and applies the settings.
    /// `discovery` switches DHT/LSD/UPnP/NAT-PMP (off in tests).
    pub fn new(data_dir: &Path, listen: SocketAddr, discovery: bool) -> Result<AppState, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let settings = Settings::load(data_dir);
        let cfg = EngineConfig { listen, state_dir: data_dir.join("engine"), discovery };
        let engine = Engine::new(cfg).map_err(|e| e.to_string())?;
        let queue = Queue::new(settings.max_active_downloads, settings.max_active_seeds);
        let state = AppState {
            engine,
            data_dir: data_dir.to_path_buf(),
            settings: Mutex::new(settings),
            queue: Mutex::new(queue),
            poll: Mutex::new(PollState::default()),
            pending_args: Mutex::new(Vec::new()),
        };
        state.apply_settings()?;
        Ok(state)
    }

    /// A copy of the current settings.
    pub fn get_settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Validates, saves and applies new settings. Nothing changes if validation fails.
    pub fn set_settings(&self, new: Settings) -> Result<(), String> {
        validate_settings(&new)?;
        new.save(&self.data_dir).map_err(|e| format!("could not save settings: {e}"))?;
        *self.settings.lock().unwrap() = new;
        self.apply_settings()
    }

    /// Pushes the current settings into the engine and queue: encryption, proxy, queue limits
    /// and the rate caps (schedule aware).
    fn apply_settings(&self) -> Result<(), String> {
        let s = self.get_settings();
        let mode = match s.encryption {
            EncryptionPref::Forced => EncryptionMode::Forced,
            EncryptionPref::Prefer => EncryptionMode::Prefer,
        };
        self.engine.set_encryption(mode).map_err(|e| e.to_string())?;
        let proxy = s.proxy.as_ref().map(|p| ProxyConfig {
            host: p.host.clone(),
            port: p.port,
            username: p.username.clone().filter(|u| !u.is_empty()),
            password: p.password.clone(),
        });
        self.engine.set_proxy(proxy).map_err(|e| e.to_string())?;
        self.queue.lock().unwrap().set_limits(s.max_active_downloads, s.max_active_seeds);
        self.poll.lock().unwrap().last_limits = None; // force the caps to be re-applied
        self.apply_limits(chrono::Local::now().naive_local());
        Ok(())
    }

    /// Hands the engine the rate caps in force at `now` (global caps, or a schedule block).
    fn apply_limits(&self, now: NaiveDateTime) {
        let s = self.get_settings();
        let want = s.schedule.limits_at(now, (s.down_limit, s.up_limit));
        let mut p = self.poll.lock().unwrap();
        if p.last_limits != Some(want) {
            let _ = self.engine.set_limits(None, want.0, want.1);
            p.last_limits = Some(want);
        }
    }

    /// One poll tick: snapshot, diff against the last snapshot, run the queue and the schedule.
    /// Called about once a second by the poller thread.
    pub fn tick(&self, now: NaiveDateTime) -> Tick {
        let snap = self.engine.snapshot();
        self.apply_limits(now);
        self.run_queue(&snap);
        let mut p = self.poll.lock().unwrap();
        let live: HashSet<&TorrentId> = snap.iter().map(|t| &t.id).collect();
        p.order.retain(|id| live.contains(id));
        let mut fresh: Vec<&TorrentState> = snap.iter().filter(|t| !p.order.contains(&t.id)).collect();
        fresh.sort_by(|a, b| a.id.cmp(&b.id));
        for t in fresh {
            p.order.push(t.id.clone());
        }
        let update = diff(&p.prev, &snap);
        let completed = completions(&p.prev, &snap);
        p.prev = snap.into_iter().map(|t| (t.id.clone(), t)).collect();
        Tick { update, completed }
    }

    /// Starts and holds torrents according to the queue limits. Torrents fetching metadata or
    /// checking files always run, and errored or user-paused ones are left alone.
    fn run_queue(&self, snap: &[TorrentState]) {
        let p = self.poll.lock().unwrap();
        let entries: Vec<QueueEntry> = snap
            .iter()
            .filter(|t| !matches!(t.status, TorrentStatus::Error(_) | TorrentStatus::Metadata | TorrentStatus::Checking))
            .map(|t| QueueEntry {
                id: t.id.clone(),
                position: p.order.iter().position(|o| o == &t.id).unwrap_or(usize::MAX >> 1) as u32,
                is_seeding: t.status == TorrentStatus::Seeding || t.progress >= 1.0,
                user_paused: t.status == TorrentStatus::Paused,
            })
            .collect();
        drop(p);
        let plan = self.queue.lock().unwrap().plan(&entries);
        for (id, action) in plan {
            let Some(t) = snap.iter().find(|t| t.id == id) else { continue };
            match (action, &t.status) {
                (nyx_core::Action::Start, TorrentStatus::Queued) => {
                    let _ = self.engine.set_queued(&id, false);
                }
                (nyx_core::Action::Hold, TorrentStatus::Downloading | TorrentStatus::Seeding) => {
                    let _ = self.engine.set_queued(&id, true);
                }
                _ => {}
            }
        }
    }

    /// Where a torrent should save. An explicit folder from the UI must be a safe absolute
    /// path; an empty one means "use the category's folder, else the default".
    pub fn resolve_save_path(&self, save_path: &str, category: Option<&str>) -> Result<PathBuf, String> {
        if save_path.trim().is_empty() {
            let s = self.get_settings();
            let cats = Categories::from_defs(s.categories.clone());
            return Ok(cats.save_dir_for(category, &s.default_save_dir));
        }
        checked_save_dir(save_path)
    }

    /// Every torrent right now, as UI rows.
    pub fn list_torrents(&self) -> Vec<TorrentDto> {
        self.engine.snapshot().iter().map(TorrentDto::from).collect()
    }

    /// Adds a magnet link; returns the torrent id.
    pub fn add_magnet(&self, uri: &str, save_path: &str, category: Option<String>, paused: bool) -> Result<TorrentId, String> {
        let mut opts = AddOptions::new(self.resolve_save_path(save_path, category.as_deref())?);
        opts.paused = paused;
        opts.category = category;
        self.engine.add_magnet(uri, opts).map_err(|e| e.to_string())
    }

    /// Adds a .torrent file from disk; returns the torrent id.
    pub fn add_torrent(&self, path: &str, save_path: &str, category: Option<String>, paused: bool) -> Result<TorrentId, String> {
        let bytes = read_torrent_file(path)?;
        let mut opts = AddOptions::new(self.resolve_save_path(save_path, category.as_deref())?);
        opts.paused = paused;
        opts.category = category;
        self.engine.add_torrent_file(&bytes, opts).map_err(|e| e.to_string())
    }

    /// Reads a .torrent and lists its files for the pre-download tree, adding nothing.
    pub fn inspect_torrent(&self, path: &str) -> Result<MetaDto, String> {
        let bytes = read_torrent_file(path)?;
        nyx_engine::inspect_torrent(&bytes).map(Into::into).map_err(|e| e.to_string())
    }

    /// Runs a per-torrent action chosen from the UI.
    pub fn torrent_action(&self, id: &TorrentId, action: &str) -> Result<(), String> {
        let e = &self.engine;
        let r = match action {
            "pause" => e.pause(id),
            "resume" => e.resume(id),
            "recheck" => e.recheck(id),
            "sequential_on" => e.set_sequential(id, true),
            "sequential_off" => e.set_sequential(id, false),
            "remove" | "remove_delete" => {
                let r = e.remove(id, action == "remove_delete");
                self.queue.lock().unwrap().forget(id);
                r
            }
            "open_folder" => return self.open_folder(id),
            other => return Err(format!("unknown action: {other}")),
        };
        r.map_err(|e| e.to_string())
    }

    /// Opens the torrent's save folder in Explorer.
    fn open_folder(&self, id: &TorrentId) -> Result<(), String> {
        let t = self.engine.snapshot().into_iter().find(|t| &t.id == id).ok_or("no such torrent")?;
        // explorer.exe exits non-zero even on success, so only a failed spawn is an error.
        std::process::Command::new("explorer").arg(&t.save_path).spawn().map(|_| ()).map_err(|e| e.to_string())
    }

    /// Per-file priorities (0 = skip).
    pub fn set_priorities(&self, id: &TorrentId, prios: Vec<u8>) -> Result<(), String> {
        self.engine.set_file_priorities(id, &prios).map_err(|e| e.to_string())
    }

    /// Sets the save folder (checked like any other) and category of an existing torrent. Used by
    /// the add dialog for magnets, which are added before the user has chosen a folder.
    pub fn set_torrent_options(&self, id: &TorrentId, save_path: &str, category: Option<String>) -> Result<(), String> {
        let dir = self.resolve_save_path(save_path, category.as_deref())?;
        self.engine.set_category(id, category);
        self.engine.set_save_path(id, &dir).map_err(|e| e.to_string())
    }

    /// Files, peers and trackers for the drawer.
    pub fn get_detail(&self, id: &TorrentId) -> Result<DetailDto, String> {
        self.engine.torrent_detail(id).map(Into::into).map_err(|e| e.to_string())
    }

    /// A shareable magnet link for a torrent.
    pub fn get_magnet(&self, id: &TorrentId) -> Result<String, String> {
        let name = self.engine.snapshot().into_iter().find(|t| &t.id == id).map(|t| t.name).ok_or("no such torrent")?;
        let trackers = self.engine.torrent_detail(id).map_err(|e| e.to_string())?.trackers;
        let urls: Vec<String> = trackers.into_iter().map(|t| t.url).collect();
        Ok(build_magnet(id, &name, &urls))
    }

    /// Moves a torrent in the queue so it sits at `position`.
    pub fn move_in_queue(&self, id: &TorrentId, position: u32) {
        self.queue.lock().unwrap().move_to(id, position);
    }

    /// Stores magnets/paths that arrived while the UI may not be listening yet.
    pub fn push_pending(&self, args: Vec<String>) {
        self.pending_args.lock().unwrap().extend(args);
    }

    /// Hands the UI everything waiting, and clears it.
    pub fn take_pending(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_args.lock().unwrap())
    }

    /// Pause every running torrent (tray menu).
    pub fn pause_all(&self) {
        for t in self.engine.snapshot() {
            if !matches!(t.status, TorrentStatus::Paused | TorrentStatus::Error(_)) {
                let _ = self.engine.pause(&t.id);
            }
        }
    }

    /// Resume every user-paused torrent (tray menu).
    pub fn resume_all(&self) {
        for t in self.engine.snapshot() {
            if t.status == TorrentStatus::Paused {
                let _ = self.engine.resume(&t.id);
            }
        }
    }
}

/// Rejects settings that would leave the app unusable.
fn validate_settings(s: &Settings) -> Result<(), String> {
    if s.max_active_downloads == 0 || s.max_active_seeds == 0 {
        return Err("queue limits must be at least 1".into());
    }
    if let Some(p) = &s.proxy {
        if p.host.trim().is_empty() || p.port == 0 {
            return Err("proxy needs a host and a port".into());
        }
    }
    for b in &s.schedule.blocks {
        if b.start == b.end {
            return Err("a schedule block cannot start and end at the same time".into());
        }
        if b.start.0 > 23 || b.end.0 > 23 || b.start.1 > 59 || b.end.1 > 59 {
            return Err("schedule time out of range".into());
        }
    }
    Ok(())
}

/// Reads a .torrent file, refusing anything over the size limit before loading it.
fn read_torrent_file(path: &str) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("cannot open {path}: {e}"))?.len();
    if len > MAX_TORRENT_FILE {
        return Err("torrent file is too large".into());
    }
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// A user-chosen save folder must be a full drive path with no `..`, reserved names,
/// trailing dots or over-long parts. The drive root is the base and the rest goes through
/// nyx-core's `safe_join`, the same check used for torrent file paths.
pub fn checked_save_dir(raw: &str) -> Result<PathBuf, String> {
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err("save folder must be a full path, like D:\\Downloads".into());
    }
    let mut comps = p.components();
    let mut base = PathBuf::new();
    match comps.next() {
        Some(Component::Prefix(pre)) if matches!(pre.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {
            base.push(pre.as_os_str());
        }
        _ => return Err("save folder must be on a local drive (network paths are not supported)".into()),
    }
    match comps.next() {
        Some(Component::RootDir) => base.push(Component::RootDir),
        _ => return Err("save folder must be a full path, like D:\\Downloads".into()),
    }
    let rel: Vec<String> = comps
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rel.is_empty() {
        return Ok(base);
    }
    safe_join(&base, &rel.join("/")).map_err(|e| format!("unsafe save folder: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_save_dir_accepts_normal_paths() {
        assert_eq!(checked_save_dir("D:\\Downloads\\Films").unwrap(), PathBuf::from("D:\\Downloads\\Films"));
        assert_eq!(checked_save_dir("D:\\").unwrap(), PathBuf::from("D:\\"));
    }

    #[test]
    fn checked_save_dir_rejects_unsafe_paths() {
        for bad in ["downloads", "D:downloads", "D:\\a\\..\\..\\Windows", "D:\\CON", "\\\\srv\\share\\x", "D:\\x.", "/abs"] {
            assert!(checked_save_dir(bad).is_err(), "should reject {bad}");
        }
    }
}
