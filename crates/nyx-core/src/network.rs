// network.rs - settings types for WireGuard VPN profiles, the network lock and rotation.
// Pure data only; the app/engine layers do the connecting. Part of Settings (settings.json).

use serde::{Deserialize, Serialize};

/// One saved WireGuard profile. The config itself is stored encrypted so keys never sit
/// in settings.json in plain text.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Stable identifier, referenced by `active_profile` and the rotation pool.
    pub id: String,
    /// Name shown in the UI.
    pub name: String,
    /// Free-form group label (e.g. a country) for sorting the list.
    pub group: String,
    /// Opaque encrypted WireGuard config.
    pub sealed_conf: String,
    /// Endpoint as host:port, for display.
    pub endpoint: String,
    /// Resolved endpoint as IP:port, used at runtime so no DNS lookup happens outside the tunnel.
    pub endpoint_ip: String,
}

/// Debug output hides `sealed_conf`: on non-Windows builds (the lab) it is the plain key in base64,
/// and a failing `assert_eq!` or a stray `{:?}` must never print it.
impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("group", &self.group)
            .field("sealed_conf", &"<hidden>")
            .field("endpoint", &self.endpoint)
            .field("endpoint_ip", &self.endpoint_ip)
            .finish()
    }
}

/// Automatic switching between profiles at random intervals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RotationSettings {
    pub enabled: bool,
    /// Profile ids to rotate through.
    pub pool: Vec<String>,
    /// Shortest time on one profile, in minutes.
    pub min_minutes: u32,
    /// Longest time on one profile, in minutes.
    pub max_minutes: u32,
}

impl Default for RotationSettings {
    /// Off, empty pool, switch every 5 to 10 minutes once enabled.
    fn default() -> Self {
        RotationSettings { enabled: false, pool: Vec::new(), min_minutes: 5, max_minutes: 10 }
    }
}

/// The network section of Settings: saved profiles, which one is in use, and rotation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    pub profiles: Vec<Profile>,
    /// Id of the profile to connect with, if any.
    pub active_profile: Option<String>,
    pub rotation: RotationSettings,
}

#[cfg(test)]
mod tests {
    /// `{:?}` of a profile must never print the sealed config (the plain key on non-Windows).
    #[test]
    fn profile_debug_hides_the_sealed_config() {
        let p = super::Profile { id: "p1".into(), sealed_conf: "plain:SECRETKEY".into(), ..Default::default() };
        let shown = format!("{p:?}");
        assert!(!shown.contains("SECRETKEY"), "{shown}");
        assert!(shown.contains("p1"));
    }
}
