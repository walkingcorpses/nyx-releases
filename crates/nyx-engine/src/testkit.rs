//! Test helpers, compiled only with the `test-support` feature: torrent maker, peer
//! injection, plaintext-only override and polling. Never part of the shipped app.

pub mod socks5;

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{ffi, Engine, EngineError, TorrentId};

/// Piece size for generated torrents: small so sequential/priority tests have many pieces.
const PIECE_LEN: u32 = 16 * 1024;

/// Writes one deterministic payload file of `size_bytes` under `dir` and returns the .torrent bytes.
pub fn make_torrent(dir: &Path, size_bytes: u64) -> Vec<u8> {
    make_torrent_files(dir, &[size_bytes])
}

/// Like `make_torrent` but with several files (a multi-file torrent rooted at `payload/`).
pub fn make_torrent_files(dir: &Path, sizes: &[u64]) -> Vec<u8> {
    std::fs::create_dir_all(dir).unwrap();
    ffi::make_torrent(&dir.to_string_lossy(), sizes, PIECE_LEN, "").unwrap()
}

/// Asks the OS for a free loopback port, so each test engine listens somewhere unique.
pub fn free_loopback_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// Polls `cond` every 50 ms until it is true or `timeout` passes. Returns the final value.
pub fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Engine {
    /// Tells the torrent to connect to a specific peer (a local seeder in tests).
    pub fn add_peer(&self, id: &TorrentId, addr: SocketAddr) -> Result<(), EngineError> {
        Ok(self.inner.add_peer(id, &addr.ip().to_string(), addr.port())?)
    }

    /// Makes this engine refuse encrypted connections and speak plaintext only.
    pub fn set_plaintext_only(&self) -> Result<(), EngineError> {
        Ok(self.inner.set_encryption(2)?)
    }

    /// One bool per piece: do we have it yet?
    pub fn have_pieces(&self, id: &TorrentId) -> Result<Vec<bool>, EngineError> {
        Ok(self.inner.have_pieces(id)?.into_iter().map(|b| b != 0).collect())
    }
}
