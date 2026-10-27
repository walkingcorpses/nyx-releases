// shim.cpp - implementation of the cxx bridge functions declared in shim.hpp.
// Every libtorrent call Nyx makes goes through this file.
#include "nyx-engine/cpp/shim.hpp"
#include "nyx-engine/src/ffi.rs.h"

#include <libtorrent/add_torrent_params.hpp>
#include <libtorrent/alert_types.hpp>
#include <libtorrent/announce_entry.hpp>
#include <libtorrent/create_torrent.hpp>
#include <libtorrent/error_code.hpp>
#include <libtorrent/file_storage.hpp>
#include <libtorrent/ip_filter.hpp>
#include <libtorrent/load_torrent.hpp>
#include <libtorrent/magnet_uri.hpp>
#include <libtorrent/peer_info.hpp>
#include <libtorrent/read_resume_data.hpp>
#include <libtorrent/session_params.hpp>
#include <libtorrent/settings_pack.hpp>
#include <libtorrent/torrent_flags.hpp>
#include <libtorrent/torrent_handle.hpp>
#include <libtorrent/torrent_info.hpp>
#include <libtorrent/torrent_status.hpp>
#include <libtorrent/version.hpp>
#include <libtorrent/write_resume_data.hpp>

#include <chrono>
#include <condition_variable>
#include <mutex>
#include <thread>
#include <exception>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace nyx {

namespace {

// Limits applied to every .torrent that reaches libtorrent (spec: untrusted input).
constexpr int kMaxTorrentBytes = 10 * 1024 * 1024;
constexpr int kMaxBencodeDepth = 64;
// Bit for libtorrent's global peer class (id 0) in an ip_filter used as a peer-class filter.
constexpr std::uint32_t kGlobalClassBit = 1u;

// Hex of an info-hash, lowercase. v1 hash when the torrent has one (also for hybrids),
// otherwise the 64-char v2 hash. Matches nyx_core's TorrentId.
std::string id_of(lt::info_hash_t const& ih) {
    std::ostringstream os;
    if (ih.has_v1()) os << ih.v1; else os << ih.v2;
    return os.str();
}

// Decodes a lowercase/uppercase hex id into the 20-byte hash libtorrent indexes by.
// A 64-char v2 id is truncated to its first 20 bytes, which is how lt::find_torrent
// matches v2 torrents.
lt::sha1_hash hash_of(std::string const& id) {
    if (id.size() != 40 && id.size() != 64) throw std::invalid_argument("bad torrent id");
    auto nib = [](char c) -> int {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        throw std::invalid_argument("bad torrent id");
    };
    lt::sha1_hash h;
    char* out = h.data();
    for (int i = 0; i < 20; ++i) out[i] = char(nib(id[2 * i]) << 4 | nib(id[2 * i + 1]));
    return h;
}

// Looks a torrent up by id; throws (-> Rust Err) when it is not in the session.
lt::torrent_handle find(lt::session& ses, rust::Str id) {
    lt::torrent_handle h = ses.find_torrent(hash_of(std::string(id)));
    if (!h.is_valid()) throw std::runtime_error("no such torrent");
    return h;
}

// .torrent limits shared by add and inspect.
lt::load_torrent_limits load_limits() {
    lt::load_torrent_limits lim;
    lim.max_buffer_size = kMaxTorrentBytes;
    lim.max_decode_depth = kMaxBencodeDepth;
    return lim;
}

// Common add flags: Nyx does its own queueing (so no auto-manage) and repeat adds are
// not errors (the caller merges instead).
void base_flags(lt::add_torrent_params& atp, bool paused, bool sequential) {
    atp.flags &= ~lt::torrent_flags::auto_managed;
    atp.flags &= ~lt::torrent_flags::duplicate_is_error;
    // libtorrent's default flags include `paused` (auto-manage normally starts it); with
    // auto-manage off we must clear it ourselves or every torrent would sit paused.
    atp.flags &= ~lt::torrent_flags::paused;
    if (paused) atp.flags |= lt::torrent_flags::paused;
    if (sequential) atp.flags |= lt::torrent_flags::sequential_download;
}

// If `atp` names a torrent already in the session: merge trackers, fill in metadata
// when the existing one is a bare magnet, and return its id. Otherwise adds it.
rust::String add_or_merge(lt::session& ses, lt::add_torrent_params atp) {
    lt::info_hash_t const ih = atp.ti ? atp.ti->info_hashes() : atp.info_hashes;
    lt::torrent_handle existing = ses.find_torrent(ih.has_v1() ? ih.v1 : lt::sha1_hash(ih.v2.data()));
    if (existing.is_valid()) {
        for (auto const& url : atp.trackers) existing.add_tracker(lt::announce_entry(url));
        if (atp.ti && !existing.torrent_file()) {
            auto const info = atp.ti->info_section();
            existing.set_metadata(info);
        }
        return rust::String(id_of(existing.info_hashes()));
    }
    lt::torrent_handle h = ses.add_torrent(std::move(atp));
    return rust::String(id_of(h.info_hashes()));
}

// Deterministic filler bytes so test payloads hash the same every run.
void write_payload(std::filesystem::path const& p, std::uint64_t size, std::uint32_t seed) {
    std::filesystem::create_directories(p.parent_path());
    std::ofstream f(p, std::ios::binary | std::ios::trunc);
    std::uint32_t x = seed * 2654435761u + 12345u;
    std::vector<char> buf(64 * 1024);
    while (size > 0) {
        std::size_t n = std::size_t(std::min<std::uint64_t>(size, buf.size()));
        for (std::size_t i = 0; i < n; ++i) {
            x = x * 1664525u + 1013904223u;
            buf[i] = char(x >> 24);
        }
        f.write(buf.data(), std::streamsize(n));
        size -= n;
    }
}

// Maps libtorrent's torrent state to the small code Rust understands.
std::uint8_t state_code(lt::torrent_status const& st) {
    switch (st.state) {
        case lt::torrent_status::downloading_metadata: return 0;
        case lt::torrent_status::checking_files:
        case lt::torrent_status::checking_resume_data: return 1;
        case lt::torrent_status::downloading: return 2;
        case lt::torrent_status::finished:
        case lt::torrent_status::seeding: return 3;
        default: return 4;
    }
}

// Encryption policy: 0 = forced RC4 only, 1 = prefer encrypted, 2 = plaintext only (tests).
void fill_encryption(lt::settings_pack& p, std::int32_t mode) {
    using sp = lt::settings_pack;
    if (mode == 0) {
        p.set_int(sp::out_enc_policy, sp::pe_forced);
        p.set_int(sp::in_enc_policy, sp::pe_forced);
        p.set_int(sp::allowed_enc_level, sp::pe_rc4);
        p.set_bool(sp::prefer_rc4, true);
    } else if (mode == 1) {
        p.set_int(sp::out_enc_policy, sp::pe_enabled);
        p.set_int(sp::in_enc_policy, sp::pe_enabled);
        p.set_int(sp::allowed_enc_level, sp::pe_both);
    } else {
        p.set_int(sp::out_enc_policy, sp::pe_disabled);
        p.set_int(sp::in_enc_policy, sp::pe_disabled);
        p.set_int(sp::allowed_enc_level, sp::pe_plaintext);
    }
}

// With a proxy: everything (peers, trackers, hostnames) goes through it and the features that
// talk to the network directly are switched off to avoid leaks: LSD, UPnP, NAT-PMP, DHT (UDP to
// arbitrary nodes) and uTP (UDP peer traffic must not bypass the SOCKS5 CONNECT path).
// Without one, everything returns to the configured `discovery` state.
void fill_proxy(lt::settings_pack& p, bool enabled, std::string const& host, std::uint16_t port,
                std::string const& user, std::string const& pass, bool discovery) {
    using sp = lt::settings_pack;
    if (enabled) {
        p.set_int(sp::proxy_type, user.empty() ? sp::socks5 : sp::socks5_pw);
        p.set_str(sp::proxy_hostname, host);
        p.set_int(sp::proxy_port, port);
        p.set_str(sp::proxy_username, user);
        p.set_str(sp::proxy_password, pass);
        p.set_bool(sp::proxy_hostnames, true);
        p.set_bool(sp::proxy_peer_connections, true);
        p.set_bool(sp::proxy_tracker_connections, true);
        p.set_bool(sp::enable_dht, false);
        p.set_bool(sp::enable_lsd, false);
        p.set_bool(sp::enable_upnp, false);
        p.set_bool(sp::enable_natpmp, false);
        p.set_bool(sp::enable_outgoing_utp, false);
        p.set_bool(sp::enable_incoming_utp, false);
    } else {
        p.set_int(sp::proxy_type, sp::none);
        p.set_bool(sp::enable_dht, discovery);
        p.set_bool(sp::enable_lsd, discovery);
        p.set_bool(sp::enable_upnp, discovery);
        p.set_bool(sp::enable_natpmp, discovery);
        p.set_bool(sp::enable_outgoing_utp, true);
        p.set_bool(sp::enable_incoming_utp, true);
    }
}

// listen_interfaces for a proxy/adapter state: loopback only while proxied (nothing inbound from
// outside, since answering would reveal our real IP), else the adapter binding if there is one,
// else the configured address.
std::string listen_for(std::string const& listen, std::string const& iface, bool proxied) {
    if (proxied) return "127.0.0.1:0";
    if (iface.empty()) return listen;
    std::size_t const colon = listen.rfind(':');
    return iface + (colon == std::string::npos ? ":6881" : listen.substr(colon));
}

} // namespace

// Real session start/stop: links the session, networking and crypto code paths,
// and checks a setting survives the round trip. Exceptions map to false so
// nothing unwinds across the cxx boundary.
bool lt_session_smoke() {
    try {
        lt::settings_pack pack;
        pack.set_str(lt::settings_pack::listen_interfaces, "127.0.0.1:0");
        pack.set_bool(lt::settings_pack::enable_dht, false);
        pack.set_bool(lt::settings_pack::enable_lsd, false);
        pack.set_bool(lt::settings_pack::enable_upnp, false);
        pack.set_bool(lt::settings_pack::enable_natpmp, false);
        lt::session ses(std::move(pack));
        lt::settings_pack back = ses.get_settings();
        return back.get_str(lt::settings_pack::listen_interfaces) == "127.0.0.1:0"
            && !back.get_bool(lt::settings_pack::enable_dht);
    } catch (std::exception const&) {
        return false;
    }
}

// Calls lt::version() (a compiled function, not a header macro) so the
// spike proves the static library actually links, not just that headers exist.
rust::String lt_version() {
    return rust::String(lt::version());
}

// The settings a new session starts with: every privacy setting already in place (encryption,
// proxy, adapter binding, identity), so not even the first packet can go out under the wrong policy.
static lt::session_params initial_params(rust::Slice<const std::uint8_t> dht_state, SessionInit const& init) {
    std::string const iface(init.interface);
    lt::settings_pack pack;
    pack.set_str(lt::settings_pack::listen_interfaces,
                 listen_for(std::string(init.listen), iface, init.proxy_enabled));
    pack.set_str(lt::settings_pack::outgoing_interfaces, iface);
    // Identity (lab findings F7, F8): nothing a tracker or peer sees names Nyx or libtorrent.
    // An empty fingerprint makes all 20 peer_id bytes random (libtorrent's default prefix
    // "-LT20B0-" names the engine and its version). anonymous_mode leaves the client version `v`
    // out of the extension handshake, sends trackers libtorrent's generic User-Agent
    // ("curl/7.81.0") and never adds ip=; it does not touch connectivity (checked in the
    // 2.0.11 source: those three places are all it changes). The user agent and handshake
    // version are blanked as well, for the private-torrent case where anonymous_mode still
    // sends the user agent.
    pack.set_str(lt::settings_pack::peer_fingerprint, "");
    pack.set_str(lt::settings_pack::user_agent, "");
    pack.set_str(lt::settings_pack::handshake_client_version, "");
    pack.set_bool(lt::settings_pack::anonymous_mode, true);
    pack.set_bool(lt::settings_pack::allow_multiple_connections_per_ip, true);
    // Default is 60 s before libtorrent redials a peer it was connected to. Pausing and
    // resuming (and the parked-magnet flow) should pick peers back up within seconds.
    // Failing peers still back off: the delay is multiplied by their fail count.
    pack.set_int(lt::settings_pack::min_reconnect_time, 5);
    pack.set_int(lt::settings_pack::alert_mask, lt::alert_category::error | lt::alert_category::status);
    fill_encryption(pack, init.encryption);
    fill_proxy(pack, init.proxy_enabled, std::string(init.proxy_host), init.proxy_port,
               std::string(init.proxy_user), std::string(init.proxy_pass), init.discovery);
    lt::session_params sp(std::move(pack));
    if (init.discovery && !init.proxy_enabled && !dht_state.empty()) {
        lt::session_params old = lt::read_session_params(lt::span<char const>(
            reinterpret_cast<char const*>(dht_state.data()), dht_state.size()));
        sp.dht_state = std::move(old.dht_state);
    }
    return sp;
}

// Records the address of `s`'s session_impl among every one this process has built; true when it
// is new. The address is an input of every tracker key (torrent::tracker_key hashes the torrent's,
// its storage slot's and the session's addresses), and the allocator readily hands a new session
// the block of one freed earlier: the identity test saw a torrent get its key from two rotations
// back. respawn() only accepts a session at an address never used before.
static bool first_use(lt::session const& s) {
    static std::mutex mu;
    static std::set<std::uintptr_t> used;
    std::lock_guard<std::mutex> lk(mu);
    return used.insert(reinterpret_cast<std::uintptr_t>(s.native_handle().get())).second;
}

// A session from the engine's start-up config (see initial_params).
Session::Session(rust::Slice<const std::uint8_t> dht_state, SessionInit const& init)
    : Session(initial_params(dht_state, init), init.discovery, std::string(init.listen),
              std::string(init.interface), init.proxy_enabled) {}

// Builds the session from complete params (start-up config or a respawn's copy of the live
// settings), then sets up what every session needs: the peer-class filter and the alert pump.
Session::Session(lt::session_params params, bool discovery, std::string listen, std::string iface, bool proxied)
    : ses_(std::move(params)),
      discovery_(discovery),
      listen_(std::move(listen)),
      iface_(std::move(iface)),
      proxied_(proxied),
      new_address_(first_use(ses_)) {
    // Rate limits must also bind LAN/loopback peers (libtorrent exempts them by default):
    // put every address, v4 and v6, in the global peer class and nothing in the local one.
    lt::ip_filter classes;
    classes.add_rule(lt::make_address_v4("0.0.0.0"), lt::make_address_v4("255.255.255.255"),
                     kGlobalClassBit);
    classes.add_rule(lt::make_address_v6("::"), lt::make_address_v6("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
                     kGlobalClassBit);
    ses_.set_peer_class_filter(classes);
    if (proxied_) schedule_reannounce();
    pump_ = std::thread([this] { pump(); });
}

// A UDP tracker announce sent before the proxy's UDP association is up fails and would only be
// retried after a long backoff. Re-announce shortly after a proxy comes into effect.
void Session::schedule_reannounce() const {
    auto const now = std::chrono::steady_clock::now();
    std::lock_guard<std::mutex> lk(mu_);
    reannounce_.push_back(now + std::chrono::milliseconds(1500));
    reannounce_.push_back(now + std::chrono::milliseconds(6000));
}

// Stops the alert thread before the session goes away.
Session::~Session() {
    stop_ = true;
    if (pump_.joinable()) pump_.join();
}

// The one place alerts are read. Handles two things:
//  - a parked magnet's metadata arrived -> pause it at once, so nothing is downloaded before
//    the user has picked files (resume() starts it);
//  - resume data answers, which save_resume() waits for.
// Everything else is dropped so libtorrent's alert queue never fills.
void Session::pump() {
    while (!stop_) {
        ses_.wait_for_alert(std::chrono::milliseconds(100));
        bool reannounce = false;
        {
            std::lock_guard<std::mutex> lk(mu_);
            auto const now = std::chrono::steady_clock::now();
            for (auto it = reannounce_.begin(); it != reannounce_.end();) {
                if (*it <= now) { reannounce = true; it = reannounce_.erase(it); } else { ++it; }
            }
        }
        if (reannounce) {
            for (lt::torrent_handle const& h : ses_.get_torrents()) h.force_reannounce();
        }
        std::vector<lt::alert*> alerts;
        ses_.pop_alerts(&alerts);
        for (lt::alert* a : alerts) {
            if (auto* md = lt::alert_cast<lt::metadata_received_alert>(a)) {
                std::string const id = id_of(md->handle.info_hashes());
                bool parked;
                {
                    std::lock_guard<std::mutex> lk(mu_);
                    parked = parked_.erase(id) > 0;
                }
                if (parked) md->handle.pause();
            } else if (auto* ok = lt::alert_cast<lt::save_resume_data_alert>(a)) {
                std::lock_guard<std::mutex> lk(mu_);
                resume_done_[id_of(ok->params.info_hashes)] = lt::write_resume_data_buf(ok->params);
                cv_.notify_all();
            } else if (lt::alert_cast<lt::save_resume_data_failed_alert>(a)) {
                std::lock_guard<std::mutex> lk(mu_);
                ++resume_failed_;
                cv_.notify_all();
            }
        }
    }
}

std::unique_ptr<Session> new_session(rust::Slice<const std::uint8_t> dht_state, SessionInit const& init) {
    return std::make_unique<Session>(dht_state, init);
}

// Magnets are parked: the torrent runs just long enough to fetch metadata, then pump() pauses it
// until the UI has set file priorities and calls resume(). (Tried and rejected: default_dont_download
// and upload_mode both make the torrent upload-only, so libtorrent drops seed peers and did not
// reliably reconnect them; a 1 B/s download limit also throttles the metadata exchange itself.)
rust::String Session::add_magnet(rust::Str uri, rust::Str save_path, bool paused, bool sequential) const {
    lt::error_code ec;
    lt::add_torrent_params atp = lt::parse_magnet_uri(std::string(uri), ec);
    if (ec) throw std::runtime_error(ec.message());
    atp.save_path = std::string(save_path);
    base_flags(atp, paused, sequential);
    rust::String id = add_or_merge(ses_, std::move(atp));
    lt::torrent_handle h = find(ses_, rust::Str(std::string(id)));
    if (!h.torrent_file()) {  // still needs metadata: park it once that arrives
        std::lock_guard<std::mutex> lk(mu_);
        parked_.insert(std::string(id));
    }
    return id;
}

rust::String Session::add_torrent(rust::Slice<const std::uint8_t> bytes, rust::Str save_path,
                                  bool paused, bool sequential) const {
    lt::add_torrent_params atp = lt::load_torrent_buffer(
        lt::span<char const>(reinterpret_cast<char const*>(bytes.data()), bytes.size()),
        load_limits());
    atp.save_path = std::string(save_path);
    base_flags(atp, paused, sequential);
    return add_or_merge(ses_, std::move(atp));
}

rust::String Session::add_resume(rust::Slice<const std::uint8_t> data) const {
    lt::add_torrent_params atp = lt::read_resume_data(lt::span<char const>(
        reinterpret_cast<char const*>(data.data()), data.size()));
    // Resume data carries its own paused flag; keep it, only drop auto-manage.
    atp.flags &= ~lt::torrent_flags::auto_managed;
    lt::torrent_handle h = ses_.add_torrent(std::move(atp));
    return rust::String(id_of(h.info_hashes()));
}

// Reads every torrent's status in one pass. Error text comes from libtorrent's errc
// (storage errors, disk full); pause state from the flags.
rust::Vec<TorrentRow> Session::snapshot() const {
    // Drain alerts so libtorrent's queue never fills (errors are read from status instead).
    std::vector<lt::alert*> drain;
    ses_.pop_alerts(&drain);
    for (auto* a : drain) fprintf(stderr, "ALERT %s\n", a->message().c_str());

    rust::Vec<TorrentRow> out;
    for (lt::torrent_handle const& h : ses_.get_torrents()) {
        lt::torrent_status st = h.status();
        TorrentRow row;
        row.id = id_of(st.info_hashes);
        row.name = st.name;
        row.state = state_code(st);
        row.paused = bool(st.flags & lt::torrent_flags::paused);
        row.error = st.errc ? rust::String(st.errc.message()) : rust::String();
        row.progress = st.progress;
        row.down_rate = std::uint64_t(st.download_payload_rate);
        row.up_rate = std::uint64_t(st.upload_payload_rate);
        auto ti = st.torrent_file.lock();
        row.size = ti ? std::uint64_t(ti->total_size()) : 0;
        row.peers = std::uint32_t(st.num_peers);
        row.save_path = st.save_path;
        out.push_back(std::move(row));
    }
    return out;
}

rust::Vec<FileRow> Session::files(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    rust::Vec<FileRow> out;
    auto ti = h.torrent_file();
    if (!ti) return out;
    lt::file_storage const& fs = ti->files();
    std::vector<std::int64_t> prog = h.file_progress();
    std::vector<lt::download_priority_t> prio = h.get_file_priorities();
    for (lt::file_index_t i{0}; i < lt::file_index_t(fs.num_files()); ++i) {
        FileRow f;
        f.path = fs.file_path(i);
        f.size = std::uint64_t(fs.file_size(i));
        std::size_t const k = std::size_t(static_cast<int>(i));
        f.progress = f.size == 0 ? 1.0f : float(double(prog[k]) / double(f.size));
        f.priority = k < prio.size() ? std::uint8_t(static_cast<std::uint8_t>(prio[k])) : 4;
        out.push_back(std::move(f));
    }
    return out;
}

// Flags string is a compact set of letters; `encrypted` counts only real RC4 payload
// encryption, not a plaintext payload behind an encrypted handshake.
rust::Vec<PeerRow> Session::peers(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    std::vector<lt::peer_info> infos;
    h.get_peer_info(infos);
    rust::Vec<PeerRow> out;
    for (auto const& p : infos) {
        PeerRow r;
        r.addr = p.ip.address().to_string() + ":" + std::to_string(p.ip.port());
        r.client = p.client;
        r.down_rate = std::uint64_t(p.payload_down_speed);
        r.up_rate = std::uint64_t(p.payload_up_speed);
        std::string fl;
        if (p.flags & lt::peer_info::interesting) fl += 'I';
        if (p.flags & lt::peer_info::choked) fl += 'c';
        if (p.flags & lt::peer_info::remote_interested) fl += 'i';
        if (p.flags & lt::peer_info::remote_choked) fl += 'C';
        fl += (p.flags & lt::peer_info::local_connection) ? 'L' : 'R';
        if (p.flags & lt::peer_info::rc4_encrypted) fl += 'E';
        r.flags = fl;
        r.encrypted = bool(p.flags & lt::peer_info::rc4_encrypted);
        out.push_back(std::move(r));
    }
    return out;
}

rust::Vec<TrackerRow> Session::trackers(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    rust::Vec<TrackerRow> out;
    for (auto const& t : h.trackers()) {
        TrackerRow r;
        r.url = t.url;
        std::string status = "Not contacted";
        for (auto const& ep : t.endpoints) {
            for (auto const& info : ep.info_hashes) {
                if (info.fails > 0) status = "Error: " + info.last_error.message();
                else if (info.updating) status = "Updating";
                else if (info.start_sent) status = "Working";
            }
        }
        r.status = status;
        out.push_back(std::move(r));
    }
    return out;
}

rust::Vec<std::uint8_t> Session::have_pieces(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    rust::Vec<std::uint8_t> out;
    auto ti = h.torrent_file();
    if (!ti) return out;
    for (lt::piece_index_t p{0}; p < lt::piece_index_t(ti->num_pieces()); ++p)
        out.push_back(h.have_piece(p) ? 1 : 0);
    return out;
}

DiscoveryFlags Session::discovery_flags() const {
    lt::settings_pack s = ses_.get_settings();
    return DiscoveryFlags{s.get_bool(lt::settings_pack::enable_dht), s.get_bool(lt::settings_pack::enable_lsd),
                          s.get_bool(lt::settings_pack::enable_upnp), s.get_bool(lt::settings_pack::enable_natpmp)};
}

std::int32_t Session::encryption_mode() const {
    lt::settings_pack s = ses_.get_settings();
    int const out = s.get_int(lt::settings_pack::out_enc_policy);
    if (out == lt::settings_pack::pe_forced) return 0;
    if (out == lt::settings_pack::pe_enabled) return 1;
    return 2;
}

void Session::pause(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    h.unset_flags(lt::torrent_flags::auto_managed);
    h.pause();
}

// The user has decided about a magnet: do not pause it when its metadata arrives.
void Session::unpark(rust::Str id) const {
    std::lock_guard<std::mutex> lk(mu_);
    parked_.erase(std::string(id));
}

LimitRow Session::torrent_limits(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    int const d = h.download_limit(), u = h.upload_limit();
    return LimitRow{d > 0 ? std::uint64_t(d) : 0, u > 0 ? std::uint64_t(u) : 0};
}

void Session::set_ip_filter(rust::Slice<const IpSpan> ranges) const {
    lt::ip_filter f;
    for (IpSpan const& r : ranges) {
        lt::error_code a, b;
        lt::address first = lt::make_address(std::string(r.first), a);
        lt::address last = lt::make_address(std::string(r.last), b);
        if (!a && !b) f.add_rule(first, last, lt::ip_filter::blocked);
    }
    ses_.set_ip_filter(f);
}

std::string Session::listen_value() const { return listen_for(listen_, iface_, proxied_); }

// Binding to an adapter: both listening and outgoing connections use it, so if the adapter (a
// VPN, say) goes away nothing connects: a built-in kill switch.
void Session::set_interface(rust::Str name) const {
    iface_ = std::string(name);
    lt::settings_pack p;
    p.set_str(lt::settings_pack::outgoing_interfaces, iface_);
    p.set_str(lt::settings_pack::listen_interfaces, listen_value());
    ses_.apply_settings(std::move(p));
}

void Session::resume(rust::Str id) const {
    lt::torrent_handle h = find(ses_, id);
    h.unset_flags(lt::torrent_flags::auto_managed);
    h.resume();
}

void Session::remove(rust::Str id, bool delete_files) const {
    lt::torrent_handle h = find(ses_, id);
    {
        std::lock_guard<std::mutex> lk(mu_);
        parked_.erase(std::string(id));
    }
    ses_.remove_torrent(h, delete_files ? lt::session::delete_files : lt::remove_flags_t{});
}

void Session::recheck(rust::Str id) const { find(ses_, id).force_recheck(); }

void Session::set_priorities(rust::Str id, rust::Slice<const std::uint8_t> prios) const {
    std::vector<lt::download_priority_t> v;
    for (std::uint8_t p : prios) v.push_back(lt::download_priority_t{p > 7 ? std::uint8_t(7) : p});
    find(ses_, id).prioritize_files(v);
}

void Session::set_sequential(rust::Str id, bool on) const {
    lt::torrent_handle h = find(ses_, id);
    if (on) h.set_flags(lt::torrent_flags::sequential_download);
    else h.unset_flags(lt::torrent_flags::sequential_download);
}

// Existing files at the destination are kept (never overwritten): a move must not destroy data.
void Session::move_storage(rust::Str id, rust::Str path) const {
    find(ses_, id).move_storage(std::string(path), lt::move_flags_t::dont_replace);
}

// 0 means unlimited in the Rust API; libtorrent uses -1 for that on per-torrent limits.
void Session::set_torrent_limits(rust::Str id, std::uint64_t down, std::uint64_t up) const {
    lt::torrent_handle h = find(ses_, id);
    h.set_download_limit(down == 0 ? -1 : int(down));
    h.set_upload_limit(up == 0 ? -1 : int(up));
}

void Session::set_global_limits(std::uint64_t down, std::uint64_t up) const {
    lt::settings_pack p;
    p.set_int(lt::settings_pack::download_rate_limit, int(down));
    p.set_int(lt::settings_pack::upload_rate_limit, int(up));
    ses_.apply_settings(std::move(p));
}

void Session::add_peer(rust::Str id, rust::Str ip, std::uint16_t port) const {
    lt::error_code ec;
    lt::address a = lt::make_address(std::string(ip), ec);
    if (ec) throw std::runtime_error(ec.message());
    find(ses_, id).connect_peer(lt::tcp::endpoint(a, port));
}

// Forced = both directions pe_forced, RC4 only: plaintext peers are refused.
void Session::set_encryption(std::int32_t mode) const {
    lt::settings_pack p;
    fill_encryption(p, mode);
    ses_.apply_settings(std::move(p));
}

void Session::set_proxy(bool enabled, rust::Str host, std::uint16_t port, rust::Str user,
                        rust::Str pass) const {
    proxied_ = enabled;
    lt::settings_pack p;
    fill_proxy(p, enabled, std::string(host), port, std::string(user), std::string(pass), discovery_);
    p.set_str(lt::settings_pack::listen_interfaces, listen_value());
    ses_.apply_settings(std::move(p));
    if (enabled) schedule_reannounce();
}

rust::Vec<std::uint8_t> Session::save_session_state() const {
    std::vector<char> buf = lt::write_session_params_buf(ses_.session_state(lt::session::save_dht_state));
    rust::Vec<std::uint8_t> out;
    for (char c : buf) out.push_back(std::uint8_t(c));
    return out;
}

// Asks every torrent for resume data and waits (up to timeout) for the pump thread to collect the
// answers. Failures (e.g. nothing changed, no metadata) are skipped; their old file stays valid.
rust::Vec<ResumeBlob> Session::save_resume(std::uint32_t timeout_ms) const {
    std::lock_guard<std::mutex> one_at_a_time(save_mu_);
    {
        std::lock_guard<std::mutex> lk(mu_);
        resume_done_.clear();
        resume_failed_ = 0;
    }
    int pending = 0;
    for (lt::torrent_handle const& h : ses_.get_torrents()) {
        if (!h.is_valid()) continue;
        h.save_resume_data(lt::torrent_handle::save_info_dict);
        ++pending;
    }
    std::unique_lock<std::mutex> lk(mu_);
    cv_.wait_for(lk, std::chrono::milliseconds(timeout_ms),
                 [&] { return int(resume_done_.size()) + resume_failed_ >= pending; });
    rust::Vec<ResumeBlob> out;
    for (auto const& [id, buf] : resume_done_) {
        ResumeBlob b;
        b.id = id;
        for (char c : buf) b.data.push_back(std::uint8_t(c));
        out.push_back(std::move(b));
    }
    return out;
}

// Lab findings F5, F6. libtorrent keeps a torrent's tracker peer_id (made when the torrent object
// is built) and its tracker key (a hash of the torrent's and the session's addresses) for the torrent's whole life;
// a new peer_fingerprint only reaches peer connections opened later. So a new identity needs new
// torrent objects in a new session. Order, so nothing links or leaks:
//  1. Read the live settings (they already hold the new tunnel's proxy), blocklist and torrent count.
//  2. Silence this session: proxy to 127.0.0.1:0 (every connect fails on the spot, so the stopped
//     announces of step 3 and of the later teardown never reach a tracker: the old peer_id and key
//     must not show up behind the new exit) and loopback listening (frees a configured port for
//     the new session); then pause it (disconnects every peer). Session pause does not touch the
//     torrents' own paused flags, so the resume data keeps them.
//  3. Resume data for every torrent; any missing -> undo step 2 and fail (no torrent may be lost).
//  4. The new session at an address no earlier session of this process had (see first_use), the
//     step-1 settings, the blocklist before any torrent, parked magnets, then every torrent from
//     its resume data (save path, progress, paused flag, limits, file priorities, sequential,
//     trackers).
// This session stays alive (inert) until the caller drops it after the swap.
std::unique_ptr<Session> Session::respawn(std::uint32_t timeout_ms) const {
    lt::settings_pack const keep = ses_.get_settings();
    lt::ip_filter const blocklist = ses_.get_ip_filter();
    std::size_t const count = ses_.get_torrents().size();
    // The live settings, silenced: nothing can connect, loopback listening only.
    lt::settings_pack quiet = keep;
    fill_proxy(quiet, true, "127.0.0.1", 0, "", "", false);
    quiet.set_str(lt::settings_pack::listen_interfaces, "127.0.0.1:0");
    ses_.apply_settings(quiet);
    ses_.pause();
    try {
        rust::Vec<ResumeBlob> blobs = save_resume(timeout_ms);
        if (blobs.size() != count)
            throw std::runtime_error("identity rotation: resume data for " + std::to_string(blobs.size()) + " of " +
                                     std::to_string(count) + " torrents");
        std::set<std::string> parked;
        {
            std::lock_guard<std::mutex> lk(mu_);
            parked = parked_;
        }
        // Candidates start silenced too and get the live settings only once accepted, before any
        // torrent exists: no socket ever opens under a looser policy. One at a reused address is
        // kept alive while the next is built, so the allocator must place that one elsewhere.
        std::vector<std::unique_ptr<Session>> reused;
        auto fresh = std::make_unique<Session>(lt::session_params(quiet), discovery_, listen_, iface_, proxied_);
        while (!fresh->new_address_ && reused.size() < 8) {
            reused.push_back(std::move(fresh));
            fresh = std::make_unique<Session>(lt::session_params(quiet), discovery_, listen_, iface_, proxied_);
        }
        if (!fresh->new_address_) throw std::runtime_error("identity rotation: no session at a fresh address");
        reused.clear();
        fresh->ses_.apply_settings(keep);
        fresh->ses_.set_ip_filter(blocklist);
        {
            std::lock_guard<std::mutex> lk(fresh->mu_);
            fresh->parked_ = std::move(parked);
        }
        for (ResumeBlob const& b : blobs)
            fresh->add_resume(rust::Slice<const std::uint8_t>(b.data.data(), b.data.size()));
        return fresh;
    } catch (...) {
        ses_.apply_settings(keep);
        ses_.resume();
        throw;
    }
}

TorrentMeta inspect_torrent(rust::Slice<const std::uint8_t> bytes) {
    lt::add_torrent_params atp = lt::load_torrent_buffer(
        lt::span<char const>(reinterpret_cast<char const*>(bytes.data()), bytes.size()),
        load_limits());
    TorrentMeta m;
    m.id = id_of(atp.ti->info_hashes());
    m.name = atp.ti->name();
    lt::file_storage const& fs = atp.ti->files();
    for (lt::file_index_t i{0}; i < lt::file_index_t(fs.num_files()); ++i) {
        FileRow f;
        f.path = fs.file_path(i);
        f.size = std::uint64_t(fs.file_size(i));
        f.progress = 0.0f;
        f.priority = 4;
        m.files.push_back(std::move(f));
    }
    return m;
}

rust::Vec<std::uint8_t> make_torrent(rust::Str dir, rust::Slice<const std::uint64_t> sizes,
                                     std::uint32_t piece_len, rust::Str tracker) {
    namespace fsys = std::filesystem;
    fsys::path const base{std::string(dir)};
    lt::file_storage fs;
    if (sizes.size() == 1) {
        write_payload(base / "payload.bin", sizes[0], 0);
        fs.add_file("payload.bin", std::int64_t(sizes[0]));
    } else {
        for (std::size_t i = 0; i < sizes.size(); ++i) {
            std::string const rel = "payload/f" + std::to_string(i) + ".bin";
            write_payload(base / rel, sizes[i], std::uint32_t(i));
            fs.add_file(rel, std::int64_t(sizes[i]));
        }
    }
    lt::create_torrent ct(fs, int(piece_len));
    if (!tracker.empty()) ct.add_tracker(std::string(tracker));
    lt::error_code ec;
    lt::set_piece_hashes(ct, base.string(), ec);
    if (ec) throw std::runtime_error(ec.message());
    std::vector<char> buf = ct.generate_buf();
    rust::Vec<std::uint8_t> out;
    for (char c : buf) out.push_back(std::uint8_t(c));
    return out;
}

} // namespace nyx
