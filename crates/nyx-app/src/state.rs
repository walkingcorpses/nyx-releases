// state.rs - the app's brain, free of any Tauri types so it can be tested directly:
// owns the engine, the settings and the queue, and implements every UI command.
// commands.rs only forwards Tauri calls to the methods here.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::NaiveDateTime;
use nyx_core::{
    build_magnet, parse_blocklist, safe_join, Categories, EncryptionPref, IpRange, Queue, QueueEntry, Settings,
    TorrentId,
};
use nyx_engine::{
    AddOptions, Engine, EngineConfig, EncryptionMode, ProxyConfig, TorrentState, TorrentStatus,
};

use crate::dto::{AppUpdateDto, DetailDto, MetaDto, RssReportDto, SearchResultDto, StatusDto, TorrentDto};
use crate::feeds::{self, Item, Seen};
use crate::http;
use crate::poller::{completions, diff, Update};
use crate::update::{self, UpdateInfo};

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
    /// Queue order last sent to the UI, so it is only sent again when it changes.
    last_queue: Vec<TorrentId>,
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
    /// Result of the last proxy reachability probe (true when no proxy is set).
    proxy_ok: AtomicBool,
    /// The kill switch is holding everything because the proxy is unreachable.
    killed: AtomicBool,
    /// Torrents the kill switch paused, to be released when the proxy is back.
    kill_held: Mutex<HashSet<TorrentId>>,
    /// The update found by the last check; `install_update` installs exactly this (never a URL from the UI).
    pending_update: Mutex<Option<UpdateInfo>>,
    /// Startup problems worth showing (for example an unreadable blocklist).
    warnings: Mutex<Vec<String>>,
}

impl AppState {
    /// Loads settings from `data_dir`, starts the engine on `listen` and applies the settings.
    /// `discovery` switches DHT/LSD/UPnP/NAT-PMP (off in tests).
    pub fn new(data_dir: &Path, listen: SocketAddr, discovery: bool) -> Result<AppState, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let settings = Settings::load(data_dir);
        let mut warnings = Vec::new();
        // The whole privacy policy goes into the engine config, so it is in force before the
        // session opens a socket or restores a torrent.
        let blocklist = match &settings.blocklist {
            Some(path) => load_blocklist(path).unwrap_or_else(|e| {
                warnings.push(format!("Blocklist not loaded: {e}"));
                Vec::new()
            }),
            None => Vec::new(),
        };
        let cfg = EngineConfig {
            listen,
            state_dir: data_dir.join("engine"),
            discovery,
            encryption: encryption_mode(&settings),
            proxy: engine_proxy(&settings),
            interface: settings.bind_interface.clone().filter(|i| !i.trim().is_empty()),
            blocklist,
        };
        let engine = Engine::new(cfg).map_err(|e| e.to_string())?;
        let queue = Queue::new(settings.max_active_downloads, settings.max_active_seeds);
        if let Ok(exe) = std::env::current_exe() {
            update::cleanup_old(&exe); // the exe parked by the previous self-update
        }
        let _ = std::fs::remove_dir_all(data_dir.join("tmp-torrents"));
        let state = AppState {
            engine,
            data_dir: data_dir.to_path_buf(),
            settings: Mutex::new(settings),
            queue: Mutex::new(queue),
            poll: Mutex::new(PollState::default()),
            pending_args: Mutex::new(Vec::new()),
            proxy_ok: AtomicBool::new(true),
            killed: AtomicBool::new(false),
            kill_held: Mutex::new(HashSet::new()),
            pending_update: Mutex::new(None),
            warnings: Mutex::new(warnings),
        };
        state.apply_settings()?;
        state.probe_proxy();
        Ok(state)
    }

    /// A copy of the current settings.
    pub fn get_settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// The settings for the UI and the daemon's control port: the same as `get_settings`, but with
    /// every profile's sealed config emptied, so encrypted key material never leaves the app.
    pub fn settings_for_ui(&self) -> Settings {
        let mut s = self.get_settings();
        s.network.profiles.iter_mut().for_each(|p| p.sealed_conf.clear());
        s
    }

    /// Validates, saves and applies new settings from the UI. Nothing changes if validation fails.
    /// The profile list in `new` is ignored (the UI only ever sees it without configs, so echoing
    /// it back must not wipe or alter what is stored); profiles change only through
    /// import/rename/delete. `active_profile` and `rotation` are taken from `new`.
    pub fn set_settings(&self, mut new: Settings) -> Result<(), String> {
        new.network.profiles = self.get_settings().network.profiles;
        self.commit_settings(new)
    }

    /// Changes settings from inside the app (profile import/rename/delete): applies `f` to a copy
    /// of the current settings, then validates, saves and applies like `set_settings`, but
    /// without pinning the profile list.
    pub(crate) fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut new = self.get_settings();
        f(&mut new);
        self.commit_settings(new)
    }

    /// Validates `new`, writes it to disk, makes it current and applies it.
    fn commit_settings(&self, new: Settings) -> Result<(), String> {
        validate_settings(&new)?;
        new.save(&self.data_dir).map_err(|e| format!("could not save settings: {e}"))?;
        *self.settings.lock().unwrap() = new;
        self.apply_settings()
    }

    /// Pushes the current settings into the engine and queue: encryption, proxy, queue limits
    /// and the rate caps (schedule aware).
    fn apply_settings(&self) -> Result<(), String> {
        let s = self.get_settings();
        self.engine.set_encryption(encryption_mode(&s)).map_err(|e| e.to_string())?;
        self.engine.set_proxy(engine_proxy(&s)).map_err(|e| e.to_string())?;
        self.engine
            .set_interface(s.bind_interface.as_deref().map(str::trim).filter(|i| !i.is_empty()))
            .map_err(|e| e.to_string())?;
        // Saving settings already checked the file; at startup it may have vanished: warn, don't fail.
        let ranges = match &s.blocklist {
            Some(path) => load_blocklist(path).unwrap_or_else(|e| {
                let mut w = self.warnings.lock().unwrap();
                if !w.iter().any(|x| x.contains(&e)) {
                    w.push(format!("Blocklist not loaded: {e}"));
                }
                Vec::new()
            }),
            None => Vec::new(),
        };
        self.engine.set_ip_filter(&ranges).map_err(|e| e.to_string())?;
        self.queue.lock().unwrap().set_limits(s.max_active_downloads, s.max_active_seeds);
        self.poll.lock().unwrap().last_limits = None; // force the caps to be re-applied
        self.apply_limits(chrono::Local::now().naive_local());
        self.probe_proxy();
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

    /// One poll tick: snapshot, apply the kill switch, queue and schedule, then diff against the last
    /// snapshot. Called about once a second by the poller thread.
    pub fn tick(&self, now: NaiveDateTime) -> Tick {
        let snap = self.engine.snapshot();
        self.apply_limits(now);
        self.apply_kill_switch(&snap);
        // Queue order: new torrents join at the back (sorted by id so restores are stable).
        let mut ids: Vec<TorrentId> = snap.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        self.queue.lock().unwrap().sync(&ids);
        self.run_queue(&snap);
        let order = self.queue.lock().unwrap().order().to_vec();
        let mut p = self.poll.lock().unwrap();
        let mut update = diff(&p.prev, &snap);
        if order != p.last_queue {
            update.queue = Some(order.clone());
            p.last_queue = order;
        }
        let completed = completions(&p.prev, &snap);
        p.prev = snap.into_iter().map(|t| (t.id.clone(), t)).collect();
        Tick { update, completed }
    }

    /// Starts and holds torrents according to the queue limits. Torrents fetching metadata or
    /// checking files always run, and errored or user-paused ones are left alone. Nothing starts
    /// while the kill switch is holding.
    fn run_queue(&self, snap: &[TorrentState]) {
        if self.killed.load(Ordering::SeqCst) {
            return;
        }
        let entries: Vec<QueueEntry> = snap
            .iter()
            .filter(|t| !matches!(t.status, TorrentStatus::Error(_) | TorrentStatus::Metadata | TorrentStatus::Checking))
            .map(|t| QueueEntry {
                id: t.id.clone(),
                position: 0, // the queue knows every torrent's place after `sync`
                is_seeding: t.status == TorrentStatus::Seeding || t.progress >= 1.0,
                user_paused: t.status == TorrentStatus::Paused,
            })
            .collect();
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

    /// Checks whether the configured proxy accepts a TCP connection (nothing to check without one).
    /// The poller thread calls this every few seconds; the result feeds the kill switch.
    pub fn probe_proxy(&self) {
        use std::net::ToSocketAddrs;
        let ok = match self.get_settings().proxy {
            None => true,
            Some(p) => (p.host.as_str(), p.port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .map(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).is_ok())
                .unwrap_or(false),
        };
        self.proxy_ok.store(ok, Ordering::SeqCst);
    }

    /// Kill switch: with a proxy set and unreachable, pause every running torrent (as "queued",
    /// so it reads as held, not user-paused) and keep new ones paused; when the proxy is back,
    /// release exactly the ones it paused.
    fn apply_kill_switch(&self, snap: &[TorrentState]) {
        let s = self.get_settings();
        let want = s.kill_switch && s.proxy.is_some() && !self.proxy_ok.load(Ordering::SeqCst);
        let mut held = self.kill_held.lock().unwrap();
        if want {
            self.killed.store(true, Ordering::SeqCst);
            for t in snap {
                let running = matches!(
                    t.status,
                    TorrentStatus::Downloading | TorrentStatus::Seeding | TorrentStatus::Checking | TorrentStatus::Metadata
                );
                if running && !held.contains(&t.id) && self.engine.set_queued(&t.id, true).is_ok() {
                    held.insert(t.id.clone());
                }
            }
        } else if self.killed.swap(false, Ordering::SeqCst) {
            for id in held.drain() {
                let _ = self.engine.set_queued(&id, false);
            }
        }
    }

    /// What the UI banners need.
    pub fn status(&self) -> StatusDto {
        let s = self.get_settings();
        StatusDto {
            version: env!("CARGO_PKG_VERSION").to_string(),
            proxy_down: s.proxy.is_some() && !self.proxy_ok.load(Ordering::SeqCst),
            killed: self.killed.load(Ordering::SeqCst),
            update: self.pending_update.lock().unwrap().as_ref().map(|u| AppUpdateDto { version: u.version.clone(), notes: u.notes.clone() }),
            warnings: self.warnings.lock().unwrap().clone(),
        }
    }

    /// The queue order, first to start first.
    pub fn queue_order(&self) -> Vec<TorrentId> {
        self.queue.lock().unwrap().order().to_vec()
    }

    /// Moves a torrent to `index` in the queue.
    pub fn move_in_queue(&self, id: &TorrentId, index: u32) {
        self.queue.lock().unwrap().move_to(id, index as usize);
    }

    /// An HTTP agent that follows the proxy setting. Fails when a proxy is set but known to be
    /// down, rather than letting a request go out some other way.
    fn http_agent(&self) -> Result<ureq::Agent, String> {
        let s = self.get_settings();
        if s.proxy.is_some() && !self.proxy_ok.load(Ordering::SeqCst) {
            return Err("the proxy is not reachable".into());
        }
        http::agent(s.proxy.as_ref())
    }

    /// Asks GitHub for a newer Nyx and remembers it for `install_update`.
    pub fn check_update(&self) -> Result<Option<AppUpdateDto>, String> {
        let found = update::check(&self.http_agent()?, env!("CARGO_PKG_VERSION"))?;
        let dto = found.as_ref().map(|u| AppUpdateDto { version: u.version.clone(), notes: u.notes.clone() });
        *self.pending_update.lock().unwrap() = found;
        Ok(dto)
    }

    /// Downloads, verifies and swaps in the update found by the last check. The new exe runs after
    /// a restart. Returns the installed version.
    pub fn install_update(&self) -> Result<String, String> {
        let info = self.pending_update.lock().unwrap().clone().ok_or("no update has been found yet")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let tmp = self.data_dir.join("update").join("nyx-new.exe");
        update::download(&self.http_agent()?, &info, &tmp)?;
        let swapped = update::swap_exe(&tmp, &exe)
            .map_err(|e| format!("could not replace {} (is the folder writable?): {e}", exe.display()));
        let _ = std::fs::remove_file(&tmp);
        swapped?;
        *self.pending_update.lock().unwrap() = None;
        Ok(info.version)
    }

    /// Searches the configured provider (an RSS/Torznab URL with `{query}`).
    pub fn search(&self, query: &str) -> Result<Vec<SearchResultDto>, String> {
        let template = self.get_settings().search_url;
        let q = query.trim();
        if q.is_empty() {
            return Err("type something to search for".into());
        }
        if template.trim().is_empty() {
            return Err("no search provider is set (Settings > Automation)".into());
        }
        let url = template.replace("{query}", &urlencoding::encode(q));
        let xml = http::get_bytes(&self.http_agent()?, &url, feeds::MAX_FEED_BYTES)?;
        let mut items = feeds::parse_feed(&xml);
        items.sort_by(|a, b| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)).then_with(|| a.title.cmp(&b.title)));
        Ok(items
            .into_iter()
            .take(100)
            .map(|i| SearchResultDto { title: i.title, link: i.link, size: i.size, seeders: i.seeders })
            .collect())
    }

    /// Downloads a .torrent from an http(s) link (a search result) into a temporary file, checks
    /// that it really is a torrent, and returns its path for the add dialog.
    pub fn fetch_torrent_url(&self, url: &str) -> Result<String, String> {
        let bytes = http::get_bytes(&self.http_agent()?, url, MAX_TORRENT_FILE)?;
        let meta = nyx_engine::inspect_torrent(&bytes).map_err(|e| format!("not a valid torrent: {e}"))?;
        let dir = self.data_dir.join("tmp-torrents");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.torrent", &meta.id[..16]));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Polls every enabled RSS feed once and adds new items that match the feed's filter. A feed
    /// seen for the first time is only recorded (its current items are skipped), so adding a feed
    /// never floods the download list.
    pub fn rss_run(&self) -> RssReportDto {
        let mut report = RssReportDto::default();
        let s = self.get_settings();
        let agent = match self.http_agent() {
            Ok(a) => a,
            Err(e) => {
                report.errors.push(e);
                return report;
            }
        };
        let seen_path = self.data_dir.join("rss-seen.txt");
        let mut seen = Seen::load(&seen_path);
        for feed in s.rss.feeds.iter().filter(|f| f.enabled) {
            let xml = match http::get_bytes(&agent, &feed.url, feeds::MAX_FEED_BYTES) {
                Ok(x) => x,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", feed.name));
                    continue;
                }
            };
            let items = feeds::parse_feed(&xml);
            let baseline_key = format!("baseline:{}", feed.url);
            if !seen.contains(&baseline_key) {
                items.iter().for_each(|i| seen.insert(&i.id));
                seen.insert(&baseline_key);
                report.baselined += 1;
                continue;
            }
            let fresh: Vec<&Item> =
                items.iter().filter(|i| !seen.contains(&i.id) && feeds::matches(&feed.filter, &i.title)).collect();
            for item in fresh {
                match self.add_feed_item(&agent, item, feed.category.clone()) {
                    Ok(()) => {
                        seen.insert(&item.id);
                        report.added += 1;
                    }
                    Err(e) => report.errors.push(format!("{}: {}: {e}", feed.name, item.title)),
                }
            }
            // Items that did not match are remembered too, so a later filter edit does not pull old ones.
            items.iter().filter(|i| !feeds::matches(&feed.filter, &i.title)).for_each(|i| seen.insert(&i.id));
        }
        let _ = seen.save();
        report
    }

    /// Adds one feed item (magnet or .torrent link) straight into its category folder and starts it.
    fn add_feed_item(&self, agent: &ureq::Agent, item: &Item, category: Option<String>) -> Result<(), String> {
        let mut opts = AddOptions::new(self.resolve_save_path("", category.as_deref())?);
        opts.category = category;
        let id = if item.link.to_ascii_lowercase().starts_with("magnet:") {
            self.engine.add_magnet(&item.link, opts)
        } else {
            let bytes = http::get_bytes(agent, &item.link, MAX_TORRENT_FILE)?;
            self.engine.add_torrent_file(&bytes, opts)
        }
        .map_err(|e| e.to_string())?;
        // Automatic adds download everything: lift the "wait for the file choice" hold on magnets.
        self.engine.resume(&id).map_err(|e| e.to_string())
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

    /// Speed caps for one torrent, in bytes per second (0 = unlimited).
    pub fn set_torrent_limits(&self, id: &TorrentId, down: u64, up: u64) -> Result<(), String> {
        self.engine.set_limits(Some(id), down, up).map_err(|e| e.to_string())
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

/// Engine encryption mode for the saved preference.
fn encryption_mode(s: &Settings) -> EncryptionMode {
    match s.encryption {
        EncryptionPref::Forced => EncryptionMode::Forced,
        EncryptionPref::Prefer => EncryptionMode::Prefer,
    }
}

/// The proxy section of the settings in the form the engine takes.
fn engine_proxy(s: &Settings) -> Option<ProxyConfig> {
    s.proxy.as_ref().map(|p| ProxyConfig {
        host: p.host.clone(),
        port: p.port,
        username: p.username.clone().filter(|u| !u.is_empty()),
        password: p.password.clone(),
    })
}

/// Reads and parses a blocklist file, refusing one that is too big.
fn load_blocklist(path: &Path) -> Result<Vec<IpRange>, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?.len();
    if len > nyx_core::blocklist::MAX_BYTES {
        return Err(format!("{} is too large for a blocklist", path.display()));
    }
    // Lists are text, but be lenient about stray bytes.
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(parse_blocklist(&String::from_utf8_lossy(&bytes)))
}

/// Rejects settings that would leave the app unusable or unsafe.
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
    if let Some(path) = &s.blocklist {
        load_blocklist(path)?;
    }
    if s.rss.interval_minutes < 5 {
        return Err("RSS feeds can be polled at most every 5 minutes".into());
    }
    for f in &s.rss.feeds {
        if !http::is_http_url(&f.url) {
            return Err(format!("feed \"{}\": the address must start with http:// or https://", f.name));
        }
        if !feeds::filter_ok(&f.filter) {
            return Err(format!("feed \"{}\": the filter is not a valid regular expression", f.name));
        }
        if let Some(c) = &f.category {
            if !s.categories.iter().any(|x| &x.name == c) {
                return Err(format!("feed \"{}\": category \"{c}\" does not exist", f.name));
            }
        }
    }
    let n = &s.network;
    let known = |id: &str| n.profiles.iter().any(|p| p.id == id);
    if let Some(a) = &n.active_profile {
        if !known(a) {
            return Err("the active profile does not exist".into());
        }
    }
    if n.rotation.pool.iter().any(|id| !known(id)) {
        return Err("the rotation pool lists a profile that does not exist".into());
    }
    if n.rotation.min_minutes < 1 {
        return Err("rotation must stay on a profile for at least 1 minute".into());
    }
    if n.rotation.min_minutes > n.rotation.max_minutes {
        return Err("the rotation minimum time cannot be longer than the maximum".into());
    }
    if n.rotation.enabled && n.rotation.pool.len() < 2 {
        return Err("rotation needs at least 2 profiles in its pool".into());
    }
    let su = s.search_url.trim();
    if !su.is_empty() && !(http::is_http_url(su) && su.contains("{query}")) {
        return Err("the search address must be an http(s) URL containing {query}".into());
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
