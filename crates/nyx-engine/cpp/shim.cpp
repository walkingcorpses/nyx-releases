// shim.cpp - implementation of the cxx bridge functions declared in shim.hpp.
// Every libtorrent call Nyx makes goes through this file.
#include "nyx-engine/cpp/shim.hpp"

#include <libtorrent/session.hpp>
#include <libtorrent/settings_pack.hpp>
#include <libtorrent/version.hpp>

#include <exception>

namespace nyx {

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

} // namespace nyx
