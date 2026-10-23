// queue.rs - download/seed queue planning. Pure logic: given the torrents and the limits,
// say which should run and which should wait. The app applies the answer each tick.

use crate::TorrentId;

/// What the queue wants done with a torrent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Hold,
}

/// One torrent as the queue sees it. `position` is its place in the list (0 = first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: TorrentId,
    pub position: u32,
    pub is_seeding: bool,
    pub user_paused: bool,
}

/// Queue limits plus the user's ordering. `order` is the queue: index 0 starts first. Torrents
/// the queue has not seen yet sort after the known ones, by their list position.
#[derive(Debug, Clone)]
pub struct Queue {
    max_active_downloads: usize,
    max_active_seeds: usize,
    order: Vec<TorrentId>,
}

impl Queue {
    /// Queue allowing this many running downloads and running seeds at once.
    pub fn new(max_active_downloads: usize, max_active_seeds: usize) -> Queue {
        Queue { max_active_downloads, max_active_seeds, order: Vec::new() }
    }

    /// Changes the limits (settings page) without losing manual ordering.
    pub fn set_limits(&mut self, max_active_downloads: usize, max_active_seeds: usize) {
        self.max_active_downloads = max_active_downloads;
        self.max_active_seeds = max_active_seeds;
    }

    /// Brings the order in line with the torrents that exist: ids that are gone drop out, new ids
    /// join at the back (in the order given). Returns true when the order changed.
    pub fn sync(&mut self, ids: &[TorrentId]) -> bool {
        let before = self.order.len();
        self.order.retain(|id| ids.contains(id));
        let mut changed = self.order.len() != before;
        for id in ids {
            if !self.order.contains(id) {
                self.order.push(id.clone());
                changed = true;
            }
        }
        changed
    }

    /// The queue order, first to start first.
    pub fn order(&self) -> &[TorrentId] {
        &self.order
    }

    /// Decides Start or Hold for every torrent the user has not paused. Downloads and seeds
    /// are counted against their own limits, and earlier in the queue wins. User-paused torrents
    /// are left out of the answer entirely: the queue never overrides a user's pause.
    pub fn plan(&self, torrents: &[QueueEntry]) -> Vec<(TorrentId, Action)> {
        let mut order: Vec<&QueueEntry> = torrents.iter().filter(|t| !t.user_paused).collect();
        order.sort_by(|a, b| self.key(a).cmp(&self.key(b)).then_with(|| a.id.cmp(&b.id)));
        let (mut downloads, mut seeds) = (0usize, 0usize);
        order
            .into_iter()
            .map(|t| {
                let (used, max) = if t.is_seeding {
                    (&mut seeds, self.max_active_seeds)
                } else {
                    (&mut downloads, self.max_active_downloads)
                };
                let action = if *used < max {
                    *used += 1;
                    Action::Start
                } else {
                    Action::Hold
                };
                (t.id.clone(), action)
            })
            .collect()
    }

    /// Moves a torrent to `index` in the queue (clamped to the end). An unknown id is added first.
    pub fn move_to(&mut self, id: &TorrentId, index: usize) {
        self.order.retain(|x| x != id);
        let at = index.min(self.order.len());
        self.order.insert(at, id.clone());
    }

    /// Forgets a removed torrent.
    pub fn forget(&mut self, id: &TorrentId) {
        self.order.retain(|x| x != id);
    }

    /// Sort key: its place in the queue if known, otherwise after all known ones by list position.
    fn key(&self, e: &QueueEntry) -> usize {
        self.order.iter().position(|x| x == &e.id).unwrap_or(self.order.len() + e.position as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entry helper: id, position, seeding, paused.
    fn e(id: &str, pos: u32, seeding: bool, paused: bool) -> QueueEntry {
        QueueEntry { id: id.into(), position: pos, is_seeding: seeding, user_paused: paused }
    }

    /// Ids planned as Start, in plan order.
    fn started(plan: &[(TorrentId, Action)]) -> Vec<&str> {
        plan.iter().filter(|(_, a)| *a == Action::Start).map(|(i, _)| i.as_str()).collect()
    }

    #[test]
    fn queue_starts_only_max_downloads() {
        let q = Queue::new(2, 5);
        let plan = q.plan(&[e("a", 0, false, false), e("b", 1, false, false), e("c", 2, false, false)]);
        assert_eq!(started(&plan), ["a", "b"]);
        assert_eq!(plan[2], ("c".to_string(), Action::Hold));
    }

    #[test]
    fn queue_skips_user_paused() {
        let q = Queue::new(1, 1);
        let plan = q.plan(&[e("a", 0, false, true), e("b", 1, false, false)]);
        assert_eq!(plan, vec![("b".to_string(), Action::Start)]);
    }

    #[test]
    fn queue_seeds_counted_separately() {
        let q = Queue::new(1, 1);
        let plan = q.plan(&[e("d1", 0, false, false), e("s1", 1, true, false), e("d2", 2, false, false), e("s2", 3, true, false)]);
        assert_eq!(started(&plan), ["d1", "s1"]);
    }

    #[test]
    fn queue_reorder_changes_start_order() {
        let mut q = Queue::new(1, 1);
        let list = [e("a", 0, false, false), e("b", 1, false, false), e("c", 2, false, false)];
        assert_eq!(started(&q.plan(&list)), ["a"]);
        q.sync(&["a".to_string(), "b".to_string(), "c".to_string()]);
        q.move_to(&"c".to_string(), 0);
        assert_eq!(q.order(), ["c", "a", "b"]);
        assert_eq!(started(&q.plan(&list)), ["c"]);
        q.move_to(&"c".to_string(), 99);
        assert_eq!(q.order(), ["a", "b", "c"]);
    }

    #[test]
    fn queue_sync_drops_gone_and_appends_new() {
        let mut q = Queue::new(1, 1);
        assert!(q.sync(&["a".to_string(), "b".to_string()]));
        assert!(!q.sync(&["a".to_string(), "b".to_string()]));
        assert!(q.sync(&["b".to_string(), "c".to_string()]));
        assert_eq!(q.order(), ["b", "c"]);
    }
}
