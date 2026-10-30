//! Daily system quest progress and penalty-zone eligibility.
//!
//! The quest day is the Moscow day of [`crate::daily`], so the quest resets
//! together with the personal shop at 00:00 Moscow time.

use serde::{Deserialize, Serialize};

use crate::daily;
use crate::rank::HunterRank;

/// Lowest hunter rank a failed daily quest sends to the penalty zone; weaker
/// hunters are only told they failed.
pub const PENALTY_MIN_RANK: HunterRank = HunterRank::C;
/// Delay before a penalty owed from an earlier day starts after login.
pub const PENALTY_DELAY_SECONDS: u64 = 10 * 60;
/// Required jump count.
pub const REQUIRED_JUMPS: u16 = 100;
/// Required sneak-toggle count.
pub const REQUIRED_SNEAKS: u16 = 100;
/// Required horizontal distance in blocks.
pub const REQUIRED_DISTANCE_BLOCKS: f64 = 3_000.0;
/// Length of the future penalty-zone survival challenge.
pub const PENALTY_ZONE_DURATION_SECONDS: u64 = 10 * 60;
/// Number of mobs the penalty tower spawns ahead of the hunter every wave.
pub const PENALTY_ZONE_ZOMBIES_PER_WAVE: u8 = 5;
/// Interval between future penalty-zone zombie waves.
pub const PENALTY_ZONE_WAVE_INTERVAL_SECONDS: u64 = 10;

/// Persisted progress for one daily system quest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SystemQuest {
    /// Moscow day that owns this progress.
    pub window: u64,
    /// Distinct jump-input presses recorded in this window.
    pub jumps: u16,
    /// Distinct sneak-input presses recorded in this window.
    pub sneaks: u16,
    /// Horizontal distance travelled while the quest is active.
    pub distance_blocks: f64,
    /// Whether the reward has been granted for this window.
    pub completed: bool,
    /// Whether this player must enter the future penalty-zone instance.
    pub penalty_pending: bool,
}

impl SystemQuest {
    /// Creates an uninitialized quest for player data that has not yet been
    /// assigned its first live two-hour window.
    #[must_use]
    pub const fn uninitialized() -> Self {
        Self::new(u64::MAX)
    }

    /// Creates empty progress for `window`.
    #[must_use]
    pub const fn new(window: u64) -> Self {
        Self {
            window,
            jumps: 0,
            sneaks: 0,
            distance_blocks: 0.0,
            completed: false,
            penalty_pending: false,
        }
    }

    /// Returns the quest day containing `unix_seconds`.
    #[must_use]
    pub const fn window_at(unix_seconds: u64) -> u64 {
        daily::day_at(unix_seconds)
    }

    /// Rolls this quest into `window`, reporting whether an unfinished quest
    /// became a pending penalty-zone challenge.
    ///
    /// A penalty already owed stays owed: finishing a later quest does not
    /// clear it, and several missed days still owe a single penalty. A stored
    /// window from the future (the former two-hour counter, or no window at
    /// all) starts a fresh quest without a penalty.
    pub const fn advance_window(&mut self, window: u64) -> bool {
        if self.window == window {
            return false;
        }
        if self.window > window {
            let owed = self.penalty_pending;
            *self = Self::new(window);
            self.penalty_pending = owed;
            return false;
        }

        let missed = !self.completed;
        let owed = self.penalty_pending;
        *self = Self::new(window);
        self.penalty_pending = owed || missed;
        missed
    }

    /// Returns whether every task is complete.
    #[must_use]
    pub fn tasks_complete(&self) -> bool {
        self.jumps >= REQUIRED_JUMPS
            && self.sneaks >= REQUIRED_SNEAKS
            && self.distance_blocks >= REQUIRED_DISTANCE_BLOCKS
    }

    /// Records one jump press, capped at the required target.
    pub fn record_jump(&mut self) {
        self.jumps = self.jumps.saturating_add(1).min(REQUIRED_JUMPS);
    }

    /// Records one sneak press, capped at the required target.
    pub fn record_sneak(&mut self) {
        self.sneaks = self.sneaks.saturating_add(1).min(REQUIRED_SNEAKS);
    }

    /// Records horizontal travel, capped at the required target.
    pub fn record_distance(&mut self, distance_blocks: f64) {
        if distance_blocks.is_sign_negative() || !distance_blocks.is_finite() {
            return;
        }
        self.distance_blocks =
            (self.distance_blocks + distance_blocks).min(REQUIRED_DISTANCE_BLOCKS);
    }
}

impl Default for SystemQuest {
    fn default() -> Self {
        Self::uninitialized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quest_completes_only_when_all_confirmed_tasks_reach_their_targets() {
        let mut quest = SystemQuest::new(0);
        quest.jumps = REQUIRED_JUMPS;
        quest.sneaks = REQUIRED_SNEAKS;
        assert!(!quest.tasks_complete());

        quest.record_distance(REQUIRED_DISTANCE_BLOCKS);
        assert!(quest.tasks_complete());
    }

    #[test]
    fn unfinished_window_marks_a_penalty_but_completed_window_does_not() {
        let mut quest = SystemQuest::new(0);
        assert!(quest.advance_window(1));
        assert!(quest.penalty_pending);

        quest.completed = true;
        assert!(!quest.advance_window(2));
        assert!(
            quest.penalty_pending,
            "a completed day must not clear an owed penalty"
        );

        quest.penalty_pending = false;
        quest.completed = true;
        assert!(!quest.advance_window(3));
        assert!(!quest.penalty_pending);
    }

    #[test]
    fn a_legacy_two_hour_window_restarts_without_a_penalty() {
        let mut quest = SystemQuest::new(250_000);
        assert!(!quest.advance_window(20_000));
        assert!(!quest.penalty_pending);
        assert_eq!(quest.window, 20_000);
        assert!(!SystemQuest::uninitialized().advance_window(20_000));
    }
}
