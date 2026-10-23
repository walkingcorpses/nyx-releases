//! Public data types of the engine API: torrent state, options, detail and config.
//! Plain structs with no libtorrent types in them, so the app and UI layers stay clean.

use std::net::SocketAddr;
use std::path::PathBuf;

pub use nyx_core::TorrentId;
use nyx_core::ParseError;

/// How a torrent is added: where it saves and its initial flags.
#[derive(Debug, Clone)]
pub struct AddOptions {
    pub save_path: PathBuf,
    pub paused: bool,
    pub sequential: bool,
    pub category: Option<String>,
}

impl AddOptions {
    /// Options saving to `save_path`, running, not sequential, no category.
    pub fn new(save_path: impl Into<PathBuf>) -> Self {
        AddOptions { save_path: save_path.into(), paused: false, sequential: false, category: None }
    }
}

/// What a torrent is doing right now. `Queued` means the app's queue is holding it;
/// `Paused` means the user paused it.
#[derive(Debug, Clone, PartialEq)]
pub enum TorrentStatus {
    Metadata,
    Queued,
    Checking,
    Downloading,
    Seeding,
    Paused,
    Error(String),
}

/// One row of the torrent list.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentState {
    pub id: TorrentId,
    pub name: String,
    pub status: TorrentStatus,
    pub progress: f32,
    pub down_rate: u64,
    pub up_rate: u64,
    pub size: u64,
    pub peers: u32,
    pub category: Option<String>,
    /// Folder the torrent saves into (for "open folder").
    pub save_path: PathBuf,
}

/// A connected peer, shown in the detail drawer.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerInfo {
    pub addr: SocketAddr,
    pub client: String,
    pub down_rate: u64,
    pub up_rate: u64,
    pub flags: String,
    pub encrypted: bool,
}

/// A file inside a torrent. `priority` 0 means skip, 1-7 download (higher first).
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub progress: f32,
    pub priority: u8,
}

/// A tracker with a short status string.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackerInfo {
    pub url: String,
    pub status: String,
}

/// Everything the detail drawer shows for one torrent.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentDetail {
    pub files: Vec<FileInfo>,
    pub peers: Vec<PeerInfo>,
    pub trackers: Vec<TrackerInfo>,
    /// This torrent's own speed caps in bytes per second (0 = unlimited).
    pub down_limit: u64,
    pub up_limit: u64,
}

/// Parsed `.torrent` contents, used by the pre-download file tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentMeta {
    pub id: TorrentId,
    pub name: String,
    pub files: Vec<FileInfo>,
}

/// Peer encryption policy. `Forced` refuses plaintext peers; `Prefer` accepts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMode {
    Forced,
    Prefer,
}

/// SOCKS5 proxy settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Which discovery features are on right now (read back from libtorrent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryFlags {
    pub dht: bool,
    pub lsd: bool,
    pub upnp: bool,
    pub natpmp: bool,
}

/// Engine startup settings. `discovery` switches DHT, LSD, UPnP and NAT-PMP together.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub listen: SocketAddr,
    pub state_dir: PathBuf,
    pub discovery: bool,
    /// Privacy policy the session starts with, applied before any torrent is restored.
    pub encryption: EncryptionMode,
    pub proxy: Option<ProxyConfig>,
    /// Network adapter (name or IP) to bind to.
    pub interface: Option<String>,
    /// Blocked address ranges, applied before any torrent is restored.
    pub blocklist: Vec<nyx_core::IpRange>,
}

impl EngineConfig {
    /// Config for tests: loopback only, no discovery features.
    pub fn for_test(listen: SocketAddr, state_dir: impl Into<PathBuf>) -> Self {
        EngineConfig {
            listen,
            state_dir: state_dir.into(),
            discovery: false,
            encryption: EncryptionMode::Forced,
            proxy: None,
            interface: None,
            blocklist: Vec::new(),
        }
    }
}

/// Everything that can go wrong in the engine API.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// Untrusted input was rejected before reaching libtorrent.
    Parse(ParseError),
    /// libtorrent or the OS reported a failure.
    Backend(String),
    /// No torrent with that id.
    NotFound,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Parse(e) => write!(f, "invalid input: {e:?}"),
            EngineError::Backend(m) => write!(f, "{m}"),
            EngineError::NotFound => write!(f, "no such torrent"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<ParseError> for EngineError {
    fn from(e: ParseError) -> Self {
        EngineError::Parse(e)
    }
}

impl From<cxx::Exception> for EngineError {
    /// libtorrent "no such torrent" becomes `NotFound`; everything else keeps its message.
    fn from(e: cxx::Exception) -> Self {
        let m = e.what().to_string();
        if m == "no such torrent" { EngineError::NotFound } else { EngineError::Backend(m) }
    }
}
