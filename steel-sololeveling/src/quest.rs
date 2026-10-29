//! Timed system quest progress and penalty-zone eligibility.

use serde::{Deserialize, Serialize};

/// Duration of one recurring system-quest window.
pub const QUEST_WINDOW_SECONDS: u64 = 2 * 60 * 60;
/// Required jump count.
pub const REQUIRED_JUMPS: u16 = 100;
/// Required sneak-toggle count.
pub const REQUIRED_SNEAKS: u16 = 100;
/// Required horizontal distance in blocks.
pub const REQUIRED_DISTANCE_BLOCKS: f64 = 3_000.0;
/// Unspent stat points granted for finishing a system quest.
pub const QUEST_REWARD_STAT_POINTS: u32 = 2;
/// Length of the future penalty-zone survival challenge.
pub const PENALTY_ZONE_DURATION_SECONDS: u64 = 10 * 60;
/// Number of zombies the future penalty-zone instance spawns every wave.
pub const PENALTY_ZONE_ZOMBIES_PER_WAVE: u8 = 20;
/// Interval between future penalty-zone zombie waves.
pub const PENALTY_ZONE_WAVE_INTERVAL_SECONDS: u64 = 10;

/// Persisted progress for one two-hour system-quest window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SystemQuest {
    /// Unix two-hour window that owns this progress.
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

    /// Returns the globally aligned two-hour window for Unix seconds.
    #[must_use]
    pub const fn window_at(unix_seconds: u64) -> u64 {
        unix_seconds / QUEST_WINDOW_SECONDS
    }

    /// Rolls this quest into `window`, reporting whether an unfinished quest
    /// became a pending penalty-zone challenge.
    pub const fn advance_window(&mut self, window: u64) -> bool {
        if self.window == window {
            return false;
        }

        let penalty_pending = !self.completed;
        *self = Self::new(window);
        self.penalty_pending = penalty_pending;
        penalty_pending
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
        assert!(!quest.penalty_pending);
    }
}
