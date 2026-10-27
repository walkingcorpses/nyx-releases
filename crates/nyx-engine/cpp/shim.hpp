// shim.hpp - C++ side of the cxx bridge between Rust and libtorrent-rasterbar.
// Keeps libtorrent types out of the Rust API: only cxx-friendly types cross here.
// `nyx::Session` is the one opaque object Rust holds; every method is const because
// cxx hands Rust-side callers a `const Session&` (lt::session itself is thread-safe).
#pragma once

#include "rust/cxx.h"

#include <libtorrent/session.hpp>

#include <cstdint>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <thread>
#include <vector>

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
struct IpSpan;
struct SessionInit;
struct LimitRow;

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
    // `dht_state` is the blob from save_session_state() (may be empty). `init` carries the listen
    // address and the privacy policy the session must start with (see SessionInit in ffi.rs).
    Session(rust::Slice<const std::uint8_t> dht_state, SessionInit const& init);
    // The one place a session is built (both constructors and respawn() end here): `params` already
    // carries every setting, so the policy is in force before the first socket opens. The rest is
    // the little state this wrapper keeps beside libtorrent.
    Session(lt::session_params params, bool discovery, std::string listen, std::string iface, bool proxied);
    ~Session();
    Session(Session const&) = delete;
    Session& operator=(Session const&) = delete;

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
    // Lets a parked magnet (paused at metadata, waiting for the file choice) download after a resume.
    void unpark(rust::Str id) const;
    LimitRow torrent_limits(rust::Str id) const;
    // Replaces the peer blocklist (empty clears it).
    void set_ip_filter(rust::Slice<const IpSpan> ranges) const;
    // Binds listening and outgoing connections to one network adapter (name or IP); empty unbinds.
    void set_interface(rust::Str name) const;
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

    // Identity rotation (Task 15): returns a NEW session with the same settings (proxy, adapter,
    // encryption, limits), blocklist, parked magnets and every torrent (from resume data), whose
    // torrents get a fresh tracker peer_id and key. This session is silenced and paused first and
    // stays that way; the caller swaps the new one in and destroys this one. On any error this
    // session is put back exactly as it was and the error is thrown (-> Rust Err).
    std::unique_ptr<Session> respawn(std::uint32_t timeout_ms) const;

private:
    mutable lt::session ses_;
    bool discovery_;  // configured value, restored when a proxy is removed
    std::string listen_;  // configured listen_interfaces, restored when a proxy is removed
    mutable std::string iface_;   // adapter binding, empty = none
    mutable bool proxied_ = false;
    // No earlier session of this process had this session's address (see first_use in shim.cpp).
    bool new_address_ = false;
    std::string listen_value() const;
    void schedule_reannounce() const;
    mutable std::vector<std::chrono::steady_clock::time_point> reannounce_;  // guarded by mu_  // listen_interfaces for the current proxy/adapter state

    // Alert pump (see Session::pump): one thread reads every libtorrent alert.
    void pump();
    mutable std::mutex mu_;                  // guards the three fields below
    mutable std::condition_variable cv_;     // signalled when resume data arrives
    mutable std::set<std::string> parked_;   // magnets to pause once their metadata arrives
    mutable std::map<std::string, std::vector<char>> resume_done_;
    mutable int resume_failed_ = 0;
    mutable std::mutex save_mu_;             // one save_resume() at a time
    std::atomic<bool> stop_{false};
    std::thread pump_;                       // declared last: started last, joined first
};

// cxx can only construct opaque C++ types through a free function returning unique_ptr.
std::unique_ptr<Session> new_session(rust::Slice<const std::uint8_t> dht_state, SessionInit const& init);

} // namespace nyx
