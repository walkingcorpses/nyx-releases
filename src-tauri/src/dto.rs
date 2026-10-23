// dto.rs - the shapes the UI receives over Tauri commands and events. They are plain,
// serde-friendly copies of the engine types, so the engine crate stays free of serde and
// the UI contract is visible in one file.

use nyx_core::TorrentId;
use nyx_engine::{FileInfo, PeerInfo, TorrentDetail, TorrentMeta, TorrentState, TorrentStatus, TrackerInfo};
use serde::Serialize;

use crate::poller::Update;

/// One row of the torrent table. `status` is a lowercase word; `error` is set only when
/// status is "error".
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TorrentDto {
    pub id: TorrentId,
    pub name: String,
    pub status: &'static str,
    pub error: Option<String>,
    pub progress: f32,
    pub down_rate: u64,
    pub up_rate: u64,
    pub size: u64,
    pub peers: u32,
    pub category: Option<String>,
}

impl From<&TorrentState> for TorrentDto {
    /// Flattens the engine status enum into the word + optional message the UI wants.
    fn from(t: &TorrentState) -> Self {
        let (status, error) = match &t.status {
            TorrentStatus::Metadata => ("metadata", None),
            TorrentStatus::Queued => ("queued", None),
            TorrentStatus::Checking => ("checking", None),
            TorrentStatus::Downloading => ("downloading", None),
            TorrentStatus::Seeding => ("seeding", None),
            TorrentStatus::Paused => ("paused", None),
            TorrentStatus::Error(m) => ("error", Some(m.clone())),
        };
        TorrentDto {
            id: t.id.clone(),
            name: t.name.clone(),
            status,
            error,
            progress: t.progress,
            down_rate: t.down_rate,
            up_rate: t.up_rate,
            size: t.size,
            peers: t.peers,
            category: t.category.clone(),
        }
    }
}

/// Payload of the `torrents://update` event: changed rows and ids that disappeared.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateDto {
    pub changed: Vec<TorrentDto>,
    pub removed: Vec<TorrentId>,
    /// New queue order when it changed.
    pub queue: Option<Vec<TorrentId>>,
}

impl From<&Update> for UpdateDto {
    fn from(u: &Update) -> Self {
        UpdateDto { changed: u.changed.iter().map(TorrentDto::from).collect(), removed: u.removed.clone(), queue: u.queue.clone() }
    }
}

/// A file inside a torrent (priority 0 = skip).
#[derive(Debug, Clone, Serialize)]
pub struct FileDto {
    pub path: String,
    pub size: u64,
    pub progress: f32,
    pub priority: u8,
}

impl From<FileInfo> for FileDto {
    fn from(f: FileInfo) -> Self {
        FileDto { path: f.path, size: f.size, progress: f.progress, priority: f.priority }
    }
}

/// A connected peer for the drawer's Peers tab.
#[derive(Debug, Clone, Serialize)]
pub struct PeerDto {
    pub addr: String,
    pub client: String,
    pub down_rate: u64,
    pub up_rate: u64,
    pub flags: String,
    pub encrypted: bool,
}

impl From<PeerInfo> for PeerDto {
    fn from(p: PeerInfo) -> Self {
        PeerDto {
            addr: p.addr.to_string(),
            client: p.client,
            down_rate: p.down_rate,
            up_rate: p.up_rate,
            flags: p.flags,
            encrypted: p.encrypted,
        }
    }
}

/// A tracker for the drawer's Trackers tab.
#[derive(Debug, Clone, Serialize)]
pub struct TrackerDto {
    pub url: String,
    pub status: String,
}

impl From<TrackerInfo> for TrackerDto {
    fn from(t: TrackerInfo) -> Self {
        TrackerDto { url: t.url, status: t.status }
    }
}

/// Everything the detail drawer needs for one torrent.
#[derive(Debug, Clone, Serialize)]
pub struct DetailDto {
    pub files: Vec<FileDto>,
    pub peers: Vec<PeerDto>,
    pub trackers: Vec<TrackerDto>,
    /// This torrent's own speed caps in bytes per second (0 = unlimited).
    pub down_limit: u64,
    pub up_limit: u64,
}

impl From<TorrentDetail> for DetailDto {
    fn from(d: TorrentDetail) -> Self {
        DetailDto {
            files: d.files.into_iter().map(Into::into).collect(),
            peers: d.peers.into_iter().map(Into::into).collect(),
            trackers: d.trackers.into_iter().map(Into::into).collect(),
            down_limit: d.down_limit,
            up_limit: d.up_limit,
        }
    }
}

/// Result of inspecting a `.torrent` before adding it (feeds the file tree).
#[derive(Debug, Clone, Serialize)]
pub struct MetaDto {
    pub id: TorrentId,
    pub name: String,
    pub files: Vec<FileDto>,
}

impl From<TorrentMeta> for MetaDto {
    fn from(m: TorrentMeta) -> Self {
        MetaDto { id: m.id, name: m.name, files: m.files.into_iter().map(Into::into).collect() }
    }
}

/// A newer Nyx that can be installed.
#[derive(Debug, Clone, Serialize)]
pub struct AppUpdateDto {
    pub version: String,
    pub notes: String,
}

/// What the UI banners need: network guard state, update and startup warnings.
#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    /// This Nyx's version.
    pub version: String,
    /// A proxy is configured but cannot be reached.
    pub proxy_down: bool,
    /// The kill switch has paused everything because of that.
    pub killed: bool,
    pub update: Option<AppUpdateDto>,
    /// Things that went wrong at startup (for example an unreadable blocklist).
    pub warnings: Vec<String>,
}

/// One search result.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultDto {
    pub title: String,
    pub link: String,
    pub size: Option<u64>,
    pub seeders: Option<u32>,
}

/// What an RSS run did.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RssReportDto {
    pub added: u32,
    /// Feeds seen for the first time: their current items were recorded, not downloaded.
    pub baselined: u32,
    pub errors: Vec<String>,
}
