// poller.rs - pure helpers for the once-a-second poll: what changed since last tick, and
// which downloads just finished. The thread that calls them lives in lib.rs.

use std::collections::HashMap;

use nyx_core::TorrentId;
use nyx_engine::{TorrentState, TorrentStatus};

/// What changed between two snapshots: rows that are new or different, and ids that vanished.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    pub changed: Vec<TorrentState>,
    pub removed: Vec<TorrentId>,
}

impl Update {
    /// True when nothing changed, so no event needs to be sent.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// Compares the previous snapshot with the next one. Only changed or new torrents are
/// returned, so the UI event stays small with hundreds of torrents.
pub fn diff(prev: &HashMap<TorrentId, TorrentState>, next: &[TorrentState]) -> Update {
    let changed = next.iter().filter(|t| prev.get(&t.id) != Some(t)).cloned().collect();
    let mut removed: Vec<TorrentId> =
        prev.keys().filter(|id| !next.iter().any(|t| &t.id == *id)).cloned().collect();
    removed.sort();
    Update { changed, removed }
}

/// Names of torrents that went from downloading to seeding between two snapshots.
/// Torrents first seen already complete (restored at startup) do not count.
pub fn completions(prev: &HashMap<TorrentId, TorrentState>, next: &[TorrentState]) -> Vec<String> {
    next.iter()
        .filter(|t| t.status == TorrentStatus::Seeding)
        .filter(|t| prev.get(&t.id).is_some_and(|p| p.status != TorrentStatus::Seeding && p.progress < 1.0))
        .map(|t| t.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A torrent row with just the fields the tests vary.
    fn t(id: &str, status: TorrentStatus, progress: f32) -> TorrentState {
        TorrentState {
            id: id.into(),
            name: format!("name-{id}"),
            status,
            progress,
            down_rate: 0,
            up_rate: 0,
            size: 100,
            peers: 0,
            category: None,
            save_path: Default::default(),
        }
    }

    /// Snapshot map helper.
    fn map(v: &[TorrentState]) -> HashMap<TorrentId, TorrentState> {
        v.iter().map(|t| (t.id.clone(), t.clone())).collect()
    }

    #[test]
    fn diff_reports_only_changed_torrents() {
        let a = t("a", TorrentStatus::Downloading, 0.1);
        let b = t("b", TorrentStatus::Downloading, 0.5);
        let prev = map(&[a.clone(), b.clone()]);
        let b2 = t("b", TorrentStatus::Downloading, 0.6);
        let c = t("c", TorrentStatus::Metadata, 0.0);
        let u = diff(&prev, &[a, b2.clone(), c.clone()]);
        assert_eq!(u.changed, vec![b2, c]);
        assert!(u.removed.is_empty());
    }

    #[test]
    fn diff_reports_removed_ids() {
        let a = t("a", TorrentStatus::Paused, 0.0);
        let b = t("b", TorrentStatus::Paused, 0.0);
        let prev = map(&[a.clone(), b]);
        let u = diff(&prev, &[a]);
        assert!(u.changed.is_empty());
        assert_eq!(u.removed, vec!["b".to_string()]);
    }

    #[test]
    fn diff_empty_when_nothing_changed() {
        let a = t("a", TorrentStatus::Seeding, 1.0);
        let prev = map(&[a.clone()]);
        assert!(diff(&prev, &[a]).is_empty());
    }

    #[test]
    fn completions_only_for_downloads_that_finish() {
        let d = t("a", TorrentStatus::Downloading, 0.9);
        let s = t("a", TorrentStatus::Seeding, 1.0);
        assert_eq!(completions(&map(&[d]), &[s.clone()]), vec!["name-a".to_string()]);
        // already seeding, or never seen before: no notification
        assert!(completions(&map(&[s.clone()]), &[s.clone()]).is_empty());
        assert!(completions(&HashMap::new(), &[s]).is_empty());
    }
}
