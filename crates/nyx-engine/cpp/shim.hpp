// shim.hpp - C++ side of the cxx bridge between Rust and libtorrent-rasterbar.
// Keeps libtorrent types out of the Rust API: only cxx-friendly types cross here.
// `nyx::Session` is the one opaque object Rust holds; every method is const because
// cxx hands Rust-side callers a `const Session&` (lt::session itself is thread-safe).
#pragma once

#include "rust/cxx.h"

#include <libtorrent/session.hpp>

#include <cstdint>
#include <memory>
#include <string>

namespace nyx {

// Shared structs are defined by the cxx-generated header (ffi.rs.h); forward-declared
// here so this header can mention them without a circular include.
struct TorrentRow;
struct TorrentMeta;
struct PeerRow;
struct FileRow;
struct TrackerRow;
struct ResumeBlob;
struct DiscoveryFlags;

// Returns the linked libtorrent version string (lt::version()), e.g. "2.0.11.0".
rust::String lt_version();

// Starts an lt::session on loopback with DHT/LSD/UPnP/NAT-PMP off, reads its
// settings back, then destroys it. True when the round trip works.
bool lt_session_smoke();

// Parses .torrent bytes (with size/depth limits) without adding them anywhere:
// feeds the pre-download file tree. Throws on malformed input.
TorrentMeta inspect_torrent(rust::Slice<const std::uint8_t> bytes);

// Test helper: writes deterministic payload files under `dir` and returns the
// bencoded .torrent for them (one file -> "payload.bin", several -> "payload/fN.bin").
rust::Vec<std::uint8_t> make_torrent(rust::Str dir, rust::Slice<const std::uint64_t> sizes,
                                     std::uint32_t piece_len, rust::Str tracker);

// One libtorrent session plus the little state Nyx keeps beside it.
class Session {
public:
    // `listen` is a libtorrent listen_interfaces string; `dht_state` is the blob from
    // save_session_state() (may be empty); `discovery` toggles DHT/LSD/UPnP/NAT-PMP.
    Session(rust::Str listen, rust::Slice<const std::uint8_t> dht_state, bool discovery);

    // Adding. Both return the torrent id (lowercase hex info hash) and treat a repeat
    // add as "same torrent": no error, trackers merged, metadata filled in if missing.
    rust::String add_magnet(rust::Str uri, rust::Str save_path, bool paused, bool sequential) const;
    rust::String add_torrent(rust::Slice<const std::uint8_t> bytes, rust::Str save_path,
                             bool paused, bool sequential) const;
    // Restores a torrent from write_resume_data bytes; returns its id.
    rust::String add_resume(rust::Slice<const std::uint8_t> data) const;

    // Reading state.
    rust::Vec<TorrentRow> snapshot() const;
    rust::Vec<FileRow> files(rust::Str id) const;
    rust::Vec<PeerRow> peers(rust::Str id) const;
    rust::Vec<TrackerRow> trackers(rust::Str id) const;
    // One byte per piece, 1 = we have it (tests: sequential download, resume).
    rust::Vec<std::uint8_t> have_pieces(rust::Str id) const;
    DiscoveryFlags discovery_flags() const;
    // Current encryption policy read back from libtorrent: 0 forced, 1 prefer, 2 plaintext only.
    std::int32_t encryption_mode() const;

    // Controls.
    void pause(rust::Str id) const;
    void resume(rust::Str id) const;
    void remove(rust::Str id, bool delete_files) const;
    void recheck(rust::Str id) const;
    void set_priorities(rust::Str id, rust::Slice<const std::uint8_t> prios) const;
    void set_sequential(rust::Str id, bool on) const;
    // Changes where the torrent saves; for a magnet without metadata it just updates the path.
    void move_storage(rust::Str id, rust::Str path) const;
    void set_torrent_limits(rust::Str id, std::uint64_t down, std::uint64_t up) const;
    void set_global_limits(std::uint64_t down, std::uint64_t up) const;
    void add_peer(rust::Str id, rust::Str ip, std::uint16_t port) const;

    // Security. mode: 0 = forced RC4 only, 1 = prefer encrypted, 2 = plaintext only (tests).
    void set_encryption(std::int32_t mode) const;
    // enabled=false restores direct connections and the configured discovery flags.
    void set_proxy(bool enabled, rust::Str host, std::uint16_t port, rust::Str user,
                   rust::Str pass) const;

    // Persistence. Session state is the DHT blob; resume data is one blob per torrent.
    rust::Vec<std::uint8_t> save_session_state() const;
    rust::Vec<ResumeBlob> save_resume(std::uint32_t timeout_ms) const;

private:
    mutable lt::session ses_;
    bool discovery_;  // configured value, restored when a proxy is removed
    std::string listen_;  // configured listen_interfaces, restored when a proxy is removed
};

// cxx can only construct opaque C++ types through a free function returning unique_ptr.
std::unique_ptr<Session> new_session(rust::Str listen, rust::Slice<const std::uint8_t> dht_state,
                                     bool discovery);

} // namespace nyx
