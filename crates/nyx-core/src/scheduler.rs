// scheduler.rs - weekly bandwidth schedule. Time blocks override the global rate caps.
// Pure logic: the caller passes in "now", so tests need no clock.

use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

/// One time block. `days` is a bitmask with Monday = 1, Tuesday = 2, ... Sunday = 64. A block
/// that ends before it starts runs overnight, and `days` names the day it starts on.
/// Limits are bytes per second, 0 = unlimited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub days: u8,
    pub start: (u8, u8),
    pub end: (u8, u8),
    pub down: u64,
    pub up: u64,
}

/// A weekly schedule: the first matching block wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub blocks: Vec<Block>,
}

impl Block {
    /// Is the block active at this weekday (0 = Monday) and minute of day?
    fn active(&self, weekday: u8, minute: u32) -> bool {
        let start = self.start.0 as u32 * 60 + self.start.1 as u32;
        let end = self.end.0 as u32 * 60 + self.end.1 as u32;
        let on = |d: u8| self.days & (1 << d) != 0;
        if start == end {
            false // empty block (the UI rejects this too)
        } else if start < end {
            on(weekday) && minute >= start && minute < end
        } else if minute >= start {
            on(weekday) // evening part, belongs to today
        } else {
            minute < end && on((weekday + 6) % 7) // after midnight, belongs to yesterday's block
        }
    }
}

impl Schedule {
    /// The (down, up) caps in force at `now`: the first block covering it, else `global`.
    pub fn limits_at(&self, now: NaiveDateTime, global: (u64, u64)) -> (u64, u64) {
        let weekday = now.weekday().num_days_from_monday() as u8;
        let minute = now.hour() * 60 + now.minute();
        self.blocks
            .iter()
            .find(|b| b.active(weekday, minute))
            .map(|b| (b.down, b.up))
            .unwrap_or(global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// 2026-10-26 is a Monday; `day` is days after it.
    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 26 + day).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn block(days: u8, start: (u8, u8), end: (u8, u8)) -> Block {
        Block { days, start, end, down: 100, up: 50 }
    }

    #[test]
    fn schedule_block_overrides_global_in_window() {
        let s = Schedule { blocks: vec![block(0x7f, (9, 0), (17, 0))] };
        assert_eq!(s.limits_at(at(0, 12, 0), (1000, 500)), (100, 50));
    }

    #[test]
    fn schedule_outside_window_returns_global() {
        let s = Schedule { blocks: vec![block(0x7f, (9, 0), (17, 0))] };
        assert_eq!(s.limits_at(at(0, 17, 0), (1000, 500)), (1000, 500));
        assert_eq!(s.limits_at(at(0, 8, 59), (1000, 500)), (1000, 500));
    }

    #[test]
    fn schedule_overnight_block_wraps_midnight() {
        let s = Schedule { blocks: vec![block(0x7f, (22, 0), (6, 0))] };
        assert_eq!(s.limits_at(at(1, 2, 0), (1000, 500)), (100, 50));
        assert_eq!(s.limits_at(at(0, 23, 0), (1000, 500)), (100, 50));
        assert_eq!(s.limits_at(at(1, 6, 0), (1000, 500)), (1000, 500));
    }

    #[test]
    fn schedule_day_mask_respected() {
        // Monday only (bit 0).
        let s = Schedule { blocks: vec![block(1, (9, 0), (17, 0))] };
        assert_eq!(s.limits_at(at(0, 12, 0), (1000, 500)), (100, 50));
        assert_eq!(s.limits_at(at(1, 12, 0), (1000, 500)), (1000, 500));
        // An overnight block that starts Monday also covers Tuesday 02:00, but not Monday 02:00.
        let n = Schedule { blocks: vec![block(1, (22, 0), (6, 0))] };
        assert_eq!(n.limits_at(at(1, 2, 0), (1000, 500)), (100, 50));
        assert_eq!(n.limits_at(at(0, 2, 0), (1000, 500)), (1000, 500));
    }
}
