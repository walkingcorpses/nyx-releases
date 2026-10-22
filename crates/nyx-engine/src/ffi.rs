//! cxx bridge declarations: the raw C++ functions exported by `cpp/shim.cpp`.
//! Kept private to the crate; `lib.rs` wraps each one in a safe public API.

#[cxx::bridge(namespace = "nyx")]
mod bridge {
    /// One torrent as libtorrent reports it. `state` is 0 metadata, 1 checking,
    /// 2 downloading, 3 seeding, 4 anything else; the Rust side maps it to a status.
    struct TorrentRow {
        id: String,
        name: String,
        state: u8,
        paused: bool,
        error: String,
        progress: f32,
        down_rate: u64,
        up_rate: u64,
        size: u64,
        peers: u32,
    }

    /// Result of parsing a .torrent without adding it.
    struct TorrentMeta {
        id: String,
        name: String,
        files: Vec<FileRow>,
    }

    /// One file inside a torrent, with progress and priority (0 = skip).
    struct FileRow {
        path: String,
        size: u64,
        progress: f32,
        priority: u8,
    }

    /// One connected peer.
    struct PeerRow {
        addr: String,
        client: String,
        down_rate: u64,
        up_rate: u64,
        flags: String,
        encrypted: bool,
    }

    /// One tracker and a short human status.
    struct TrackerRow {
        url: String,
        status: String,
    }

    /// Resume data for one torrent, ready to write to disk.
    struct ResumeBlob {
        id: String,
        data: Vec<u8>,
    }

    /// Which discovery features the session currently has on.
    struct DiscoveryFlags {
        dht: bool,
        lsd: bool,
        upnp: bool,
        natpmp: bool,
    }

    unsafe extern "C++" {
        include!("nyx-engine/cpp/shim.hpp");

        /// libtorrent version string from `lt::version()`.
        fn lt_version() -> String;

        /// Starts and stops a loopback-only `lt::session`; true on success.
        fn lt_session_smoke() -> bool;

        /// Parses .torrent bytes with limits, without adding them.
        fn inspect_torrent(bytes: &[u8]) -> Result<TorrentMeta>;

        /// Test helper: writes payload files under `dir`, returns the .torrent bytes.
        fn make_torrent(dir: &str, sizes: &[u64], piece_len: u32, tracker: &str) -> Result<Vec<u8>>;

        /// The opaque libtorrent session wrapper.
        type Session;

        fn new_session(listen: &str, dht_state: &[u8], discovery: bool) -> Result<UniquePtr<Session>>;

        fn add_magnet(self: &Session, uri: &str, save_path: &str, paused: bool, sequential: bool) -> Result<String>;
        fn add_torrent(self: &Session, bytes: &[u8], save_path: &str, paused: bool, sequential: bool) -> Result<String>;
        fn add_resume(self: &Session, data: &[u8]) -> Result<String>;

        fn snapshot(self: &Session) -> Result<Vec<TorrentRow>>;
        fn files(self: &Session, id: &str) -> Result<Vec<FileRow>>;
        fn peers(self: &Session, id: &str) -> Result<Vec<PeerRow>>;
        fn trackers(self: &Session, id: &str) -> Result<Vec<TrackerRow>>;
        fn have_pieces(self: &Session, id: &str) -> Result<Vec<u8>>;
        fn discovery_flags(self: &Session) -> DiscoveryFlags;

        fn pause(self: &Session, id: &str) -> Result<()>;
        fn resume(self: &Session, id: &str) -> Result<()>;
        fn remove(self: &Session, id: &str, delete_files: bool) -> Result<()>;
        fn recheck(self: &Session, id: &str) -> Result<()>;
        fn set_priorities(self: &Session, id: &str, prios: &[u8]) -> Result<()>;
        fn set_sequential(self: &Session, id: &str, on: bool) -> Result<()>;
        fn set_torrent_limits(self: &Session, id: &str, down: u64, up: u64) -> Result<()>;
        fn set_global_limits(self: &Session, down: u64, up: u64) -> Result<()>;
        fn add_peer(self: &Session, id: &str, ip: &str, port: u16) -> Result<()>;

        fn set_encryption(self: &Session, mode: i32) -> Result<()>;
        fn set_proxy(self: &Session, enabled: bool, host: &str, port: u16, user: &str, pass: &str) -> Result<()>;

        fn save_session_state(self: &Session) -> Vec<u8>;
        fn save_resume(self: &Session, timeout_ms: u32) -> Result<Vec<ResumeBlob>>;
    }
}

pub use bridge::*;
