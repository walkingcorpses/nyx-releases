// profiles.rs - WireGuard profile management on AppState: import a .conf (sealed at once, the
// endpoint resolved once), list, rename, delete. Keys never leave this module in the clear:
// the stored form is sealed (see secret.rs) and the UI only ever sees ProfileDto.

use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use nyx_core::Profile;
use nyx_tunnel::conf::parse_conf;
use rand::Rng;

use crate::dto::ProfileDto;
use crate::secret;
use crate::state::AppState;

/// Largest .conf we read (same cap as the parser, checked here first so a huge file is never loaded).
const MAX_CONF_BYTES: u64 = 64 * 1024;
/// Longest profile name or group label, in characters.
const MAX_LABEL_CHARS: usize = 64;

/// Resolves the VPN endpoint to one socket address. The only DNS lookup in the profile lifecycle:
/// it happens at import, so connecting later never asks DNS outside the tunnel. An IP literal is
/// used as is; for a name the first answer wins.
pub fn resolve_endpoint(host: &str, port: u16) -> Result<SocketAddr, String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("could not resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("could not resolve {host}: no addresses found"))
}

/// Reads a .conf as text, refusing anything over 64 KiB before it is loaded (and capping the
/// read in case the file grows between the size check and the read).
fn read_conf(path: &str) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let len = file.metadata().map_err(|e| format!("cannot read {path}: {e}"))?.len();
    if len > MAX_CONF_BYTES {
        return Err("that file is larger than 64 KiB, so it is not a WireGuard config".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONF_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("cannot read {path}: {e}"))?;
    if bytes.len() as u64 > MAX_CONF_BYTES {
        return Err("that file is larger than 64 KiB, so it is not a WireGuard config".into());
    }
    String::from_utf8(bytes).map_err(|_| "that file is not text".to_string())
}

/// Trims a name or group label and checks the length. `what` names the field in the error.
fn clean_label(raw: &str, what: &str, required: bool) -> Result<String, String> {
    let t = raw.trim();
    if required && t.is_empty() {
        return Err(format!("the {what} cannot be empty"));
    }
    if t.chars().count() > MAX_LABEL_CHARS {
        return Err(format!("the {what} is longer than {MAX_LABEL_CHARS} characters"));
    }
    Ok(t.to_string())
}

/// A fresh profile id: 16 random lowercase hex characters.
fn new_id() -> String {
    format!("{:016x}", rand::rng().random::<u64>())
}

impl AppState {
    /// Imports a WireGuard .conf: parses it, resolves its endpoint once, seals the full text and
    /// stores the profile. Nothing is stored if any step fails.
    pub fn import_profile(&self, path: &str, name: &str, group: &str) -> Result<ProfileDto, String> {
        let name = clean_label(name, "name", true)?;
        let group = clean_label(group, "group", false)?;
        let text = read_conf(path)?;
        let conf = parse_conf(&text).map_err(|e| e.to_string())?;
        let ep = &conf.endpoint;
        let addr = resolve_endpoint(&ep.host, ep.port)?;
        let sealed_conf = secret::seal(&text);
        if sealed_conf.is_empty() {
            return Err("could not encrypt the profile with Windows DPAPI".into());
        }
        // IPv6 literals need brackets to read as host:port.
        let endpoint = if ep.host.contains(':') { format!("[{}]:{}", ep.host, ep.port) } else { format!("{}:{}", ep.host, ep.port) };
        let profile = Profile { id: new_id(), name, group, sealed_conf, endpoint, endpoint_ip: addr.to_string() };
        let dto = ProfileDto { id: profile.id.clone(), name: profile.name.clone(), group: profile.group.clone(), endpoint: profile.endpoint.clone(), active: false };
        self.update_settings(|s| s.network.profiles.push(profile))?;
        Ok(dto)
    }

    /// Every saved profile, in stored order, without key material.
    pub fn list_profiles(&self) -> Vec<ProfileDto> {
        let n = self.get_settings().network;
        n.profiles
            .into_iter()
            .map(|p| ProfileDto { active: n.active_profile.as_deref() == Some(p.id.as_str()), id: p.id, name: p.name, group: p.group, endpoint: p.endpoint })
            .collect()
    }

    /// Changes a profile's name and group.
    pub fn rename_profile(&self, id: &str, name: &str, group: &str) -> Result<(), String> {
        let name = clean_label(name, "name", true)?;
        let group = clean_label(group, "group", false)?;
        if !self.get_settings().network.profiles.iter().any(|p| p.id == id) {
            return Err("no such profile".into());
        }
        self.update_settings(|s| {
            if let Some(p) = s.network.profiles.iter_mut().find(|p| p.id == id) {
                p.name = name;
                p.group = group;
            }
        })
    }

    /// Deletes a profile, unless it is the active one or sits in the rotation pool (the user must
    /// change those first, so the connection never points at a profile that is gone).
    pub fn delete_profile(&self, id: &str) -> Result<(), String> {
        let n = self.get_settings().network;
        if !n.profiles.iter().any(|p| p.id == id) {
            return Err("no such profile".into());
        }
        if n.active_profile.as_deref() == Some(id) {
            return Err("this is the active profile; choose another one first".into());
        }
        if n.rotation.pool.iter().any(|p| p == id) {
            return Err("this profile is in the rotation pool; remove it from the pool first".into());
        }
        self.update_settings(|s| s.network.profiles.retain(|p| p.id != id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_engine::testkit::free_loopback_addr;

    const PRIV: &str = "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PUB: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    /// A valid full-tunnel conf pointing at `endpoint`.
    fn conf(endpoint: &str) -> String {
        format!(
            "[Interface]\nPrivateKey = {PRIV}\nAddress = 10.2.0.2/32\nDNS = 10.64.0.1\n\n\
             [Peer]\nPublicKey = {PUB}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = {endpoint}\n"
        )
    }

    /// Writes `text` to `wg.conf` in `dir` and returns its path.
    fn write_conf(dir: &std::path::Path, text: &str) -> String {
        let p = dir.join("wg.conf");
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// A state in a temp dir plus the dir (kept alive by the caller).
    fn app() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(&dir.path().join("data"), free_loopback_addr(), false).unwrap();
        (dir, st)
    }

    #[test]
    fn import_stores_sealed_conf_and_resolves_endpoint_once() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let dto = st.import_profile(&path, "  Home  ", "NL").unwrap();
        assert_eq!((dto.name.as_str(), dto.group.as_str(), dto.endpoint.as_str(), dto.active), ("Home", "NL", "127.0.0.1:51820", false));
        assert_eq!(dto.id.len(), 16);
        assert!(dto.id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        let stored = st.get_settings().network.profiles;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].endpoint_ip, "127.0.0.1:51820");
        assert_eq!(stored[0].endpoint, "127.0.0.1:51820");
        assert!(!stored[0].sealed_conf.contains(PRIV));
        assert_eq!(secret::open(&stored[0].sealed_conf).unwrap(), conf("127.0.0.1:51820"));
        assert_eq!(st.list_profiles(), vec![dto]);
    }

    #[test]
    fn import_resolves_a_hostname_to_an_ip() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("localhost:51820"));
        st.import_profile(&path, "Local", "").unwrap();
        let p = &st.get_settings().network.profiles[0];
        assert_eq!(p.endpoint, "localhost:51820");
        let ip: SocketAddr = p.endpoint_ip.parse().unwrap();
        assert!(ip.ip().is_loopback() && ip.port() == 51820, "{ip}");
    }

    #[test]
    fn import_rejects_unresolvable_hostname_with_message() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("invalid-host.invalid:51820"));
        let e = st.import_profile(&path, "Bad", "").unwrap_err();
        assert!(e.contains("could not resolve invalid-host.invalid"), "{e}");
        assert!(st.list_profiles().is_empty());
    }

    #[test]
    fn import_rejects_file_over_64_kib_before_reading_it_all() {
        let (dir, st) = app();
        let big = format!("{}\n# {}", conf("127.0.0.1:51820"), "x".repeat(70 * 1024));
        let path = write_conf(dir.path(), &big);
        let e = st.import_profile(&path, "Big", "").unwrap_err();
        assert!(e.contains("64 KiB"), "{e}");
        assert!(st.list_profiles().is_empty());
    }

    #[test]
    fn import_rejects_bad_names_and_bad_confs_without_leaking_the_key() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        assert!(st.import_profile(&path, "   ", "").is_err());
        assert!(st.import_profile(&path, &"n".repeat(65), "").is_err());
        let bad = write_conf(dir.path(), &format!("[Interface]\nPrivateKey = {PRIV}\nbogus line\n"));
        let e = st.import_profile(&bad, "Bad", "").unwrap_err();
        assert!(!e.contains(PRIV), "{e}");
        assert!(st.import_profile("Z:\\no\\such\\file.conf", "Gone", "").is_err());
    }

    #[test]
    fn profile_dto_json_never_contains_the_private_key() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        st.import_profile(&path, "Home", "").unwrap();
        let json = serde_json::to_string(&st.list_profiles()).unwrap();
        assert!(!json.contains(PRIV) && !json.contains("sealed") && !json.contains("PrivateKey"), "{json}");
    }

    #[test]
    fn settings_json_on_disk_does_not_contain_the_private_key() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        st.import_profile(&path, "Home", "").unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join("data").join("settings.json")).unwrap();
        assert!(on_disk.contains("sealed_conf") && !on_disk.contains(PRIV), "{on_disk}");
    }

    #[test]
    fn settings_for_ui_has_no_sealed_conf() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let dto = st.import_profile(&path, "Home", "").unwrap();
        let ui = st.settings_for_ui();
        assert_eq!(ui.network.profiles.len(), 1);
        assert_eq!(ui.network.profiles[0].id, dto.id);
        assert!(ui.network.profiles[0].sealed_conf.is_empty());
        assert!(!st.get_settings().network.profiles[0].sealed_conf.is_empty());
    }

    #[test]
    fn ui_set_settings_cannot_change_or_wipe_profiles() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let dto = st.import_profile(&path, "Home", "").unwrap();
        let before = st.get_settings().network.profiles;
        // What a UI round-trip sends back: sealed_conf emptied.
        let mut roundtrip = st.settings_for_ui();
        roundtrip.max_active_downloads = 7;
        st.set_settings(roundtrip).unwrap();
        assert_eq!(st.get_settings().network.profiles, before);
        assert_eq!(st.get_settings().max_active_downloads, 7);
        // An empty list does not wipe them.
        let mut s = st.settings_for_ui();
        s.network.profiles.clear();
        st.set_settings(s).unwrap();
        assert_eq!(st.get_settings().network.profiles, before);
        // A forged profile is ignored.
        let mut s = st.settings_for_ui();
        s.network.profiles = vec![Profile { id: dto.id.clone(), sealed_conf: "plain:AAAA".into(), endpoint_ip: "6.6.6.6:1".into(), ..Default::default() }];
        st.set_settings(s).unwrap();
        assert_eq!(st.get_settings().network.profiles, before);
    }

    #[test]
    fn delete_refuses_active_profile() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let a = st.import_profile(&path, "A", "").unwrap();
        let b = st.import_profile(&path, "B", "").unwrap();
        let mut s = st.settings_for_ui();
        s.network.active_profile = Some(a.id.clone());
        st.set_settings(s).unwrap();
        assert!(st.list_profiles().iter().find(|p| p.id == a.id).unwrap().active);
        let e = st.delete_profile(&a.id).unwrap_err();
        assert!(e.contains("active"), "{e}");
        st.delete_profile(&b.id).unwrap();
        assert_eq!(st.list_profiles().len(), 1);
    }

    #[test]
    fn delete_refuses_profile_in_rotation_pool() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let a = st.import_profile(&path, "A", "").unwrap();
        let b = st.import_profile(&path, "B", "").unwrap();
        let mut s = st.settings_for_ui();
        s.network.rotation.pool = vec![a.id.clone(), b.id.clone()];
        st.set_settings(s).unwrap();
        let e = st.delete_profile(&b.id).unwrap_err();
        assert!(e.contains("rotation"), "{e}");
        assert_eq!(st.list_profiles().len(), 2);
    }

    #[test]
    fn rename_updates_name_and_group_only() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let a = st.import_profile(&path, "A", "x").unwrap();
        st.rename_profile(&a.id, " Work ", "SE").unwrap();
        let p = &st.list_profiles()[0];
        assert_eq!((p.name.as_str(), p.group.as_str(), p.endpoint.as_str()), ("Work", "SE", "127.0.0.1:51820"));
        assert!(st.rename_profile(&a.id, "", "").is_err());
        assert!(st.rename_profile("nope", "N", "").is_err());
    }

    #[test]
    fn validation_checks_active_pool_and_rotation_rules() {
        let (dir, st) = app();
        let path = write_conf(dir.path(), &conf("127.0.0.1:51820"));
        let a = st.import_profile(&path, "A", "").unwrap();
        let b = st.import_profile(&path, "B", "").unwrap();
        let try_set = |f: &dyn Fn(&mut nyx_core::Settings)| {
            let mut s = st.settings_for_ui();
            f(&mut s);
            st.set_settings(s)
        };
        assert!(try_set(&|s| s.network.active_profile = Some("ghost".into())).is_err());
        assert!(try_set(&|s| s.network.rotation.pool = vec!["ghost".into()]).is_err());
        assert!(try_set(&|s| s.network.rotation.min_minutes = 0).is_err());
        assert!(try_set(&|s| {
            s.network.rotation.min_minutes = 9;
            s.network.rotation.max_minutes = 3;
        })
        .is_err());
        // Enabled rotation needs two pool entries.
        assert!(try_set(&|s| {
            s.network.rotation.enabled = true;
            s.network.rotation.pool = vec![a.id.clone()];
        })
        .is_err());
        assert!(try_set(&|s| {
            s.network.rotation.enabled = true;
            s.network.rotation.pool = vec![a.id.clone(), b.id.clone()];
        })
        .is_ok());
    }
}
