// nyx-core: pure logic shared by the engine and the app shell.
// Kept free of libtorrent so its tests build and run in seconds.

pub mod blocklist;
pub mod categories;
pub mod network;
pub mod queue;
pub mod scheduler;
pub mod settings;
pub mod validate;

pub use blocklist::{parse_blocklist, Range as IpRange};
pub use categories::{Categories, CategoryDef};
pub use network::{NetworkSettings, Profile, RotationSettings};
pub use queue::{Action, Queue, QueueEntry};
pub use scheduler::{Block, Schedule};
pub use settings::{data_dir, EncryptionPref, Feed, ProxySettings, RssSettings, Settings};
pub use validate::{build_magnet, check_torrent_bytes, parse_magnet, safe_join, Magnet, ParseError};

/// Torrent identifier: lowercase hex info hash (40 chars v1, 64 chars v2).
/// Later crates import this so every layer names torrents the same way.
pub type TorrentId = String;
