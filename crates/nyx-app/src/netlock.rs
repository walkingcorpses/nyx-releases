// netlock.rs - the network lock's view of the VPN tunnel: turns the tunnel's handshake age into a
// route state. AppState holds every torrent and refuses HTTP requests unless the route is `Up`
// (spec 4.2). Pure functions only, so the thresholds are tested without a tunnel.

use std::time::Duration;

use serde::Serialize;

/// A tunnel whose last handshake is older than this is stale (a healthy one re-keys every 120 s).
pub const STALE_AFTER: Duration = Duration::from_secs(180);
/// How long a new tunnel may take for its first handshake before it counts as down.
pub const START_GRACE: Duration = Duration::from_secs(30);

/// The state of the VPN route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// No handshake yet, still inside the start grace.
    Starting,
    /// A recent handshake: traffic may flow.
    Up,
    /// The last handshake is too old (VPN server gone, or the machine slept).
    Stale,
    /// No handshake at all after the start grace (or no usable profile).
    Down,
}

impl Route {
    /// The lowercase word the UI shows (same as the serde form).
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Starting => "starting",
            Route::Up => "up",
            Route::Stale => "stale",
            Route::Down => "down",
        }
    }
}

/// The lock's thresholds. `Default` is the real values; tests pass shorter ones (timing override
/// for tests) so nobody waits 180 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetTiming {
    pub stale_after: Duration,
    pub start_grace: Duration,
}

impl Default for NetTiming {
    /// `STALE_AFTER` and `START_GRACE`.
    fn default() -> Self {
        NetTiming { stale_after: STALE_AFTER, start_grace: START_GRACE }
    }
}

impl NetTiming {
    /// The route for a tunnel whose last handshake is `handshake_age` old (`None` = never) and
    /// that started `since_start` ago. An age of exactly `stale_after` is still up.
    pub fn evaluate(&self, handshake_age: Option<Duration>, since_start: Duration) -> Route {
        match handshake_age {
            Some(age) if age <= self.stale_after => Route::Up,
            Some(_) => Route::Stale,
            None if since_start < self.start_grace => Route::Starting,
            None => Route::Down,
        }
    }

    /// How often the tunnel re-keys: two thirds of the stale threshold (120 s for the real 180 s),
    /// so a healthy tunnel never looks stale and a dead one keeps asking for a fresh handshake.
    pub fn rekey_age(&self) -> Duration {
        self.stale_after * 2 / 3
    }
}

/// `NetTiming::evaluate` with the real thresholds.
pub fn evaluate(handshake_age: Option<Duration>, since_start: Duration) -> Route {
    NetTiming::default().evaluate(handshake_age, since_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn fresh_handshake_is_up() {
        assert_eq!(evaluate(Some(S(1)), S(5)), Route::Up);
        assert_eq!(evaluate(Some(S(0)), S(500)), Route::Up);
    }

    #[test]
    fn handshake_older_than_180s_is_stale() {
        assert_eq!(evaluate(Some(S(181)), S(1000)), Route::Stale);
        assert_eq!(evaluate(Some(Duration::from_millis(180_001)), S(1000)), Route::Stale);
    }

    #[test]
    fn exactly_180s_is_still_up() {
        assert_eq!(evaluate(Some(S(180)), S(1000)), Route::Up);
    }

    #[test]
    fn no_handshake_is_starting_then_down_after_grace() {
        assert_eq!(evaluate(None, S(0)), Route::Starting);
        assert_eq!(evaluate(None, S(29)), Route::Starting);
        assert_eq!(evaluate(None, S(30)), Route::Down);
        assert_eq!(evaluate(None, S(3600)), Route::Down);
    }

    #[test]
    fn rekey_is_two_thirds_of_stale_and_route_serialises_lowercase() {
        assert_eq!(NetTiming::default().rekey_age(), S(120));
        assert_eq!(serde_json::to_string(&Route::Starting).unwrap(), "\"starting\"");
        assert_eq!(Route::Stale.as_str(), "stale");
    }
}
