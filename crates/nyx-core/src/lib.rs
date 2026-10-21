// nyx-core: pure logic shared by the engine and the app shell.
// Kept free of libtorrent so its tests build and run in seconds.

pub mod validate;

pub use validate::{check_torrent_bytes, parse_magnet, safe_join, Magnet, ParseError};

/// Torrent identifier: lowercase hex info hash (40 chars v1, 64 chars v2).
/// Later crates import this so every layer names torrents the same way.
pub type TorrentId = String;
