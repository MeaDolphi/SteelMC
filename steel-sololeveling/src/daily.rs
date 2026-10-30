//! The daily cycle of daily quests.
//!
//! One rule decides the boundary for every daily counter: the Moscow day. The
//! zone has kept a fixed UTC+3 offset all year since 2014, so no calendar table
//! or DST branch is needed and the reset is a single integer division.

use std::time::{SystemTime, UNIX_EPOCH};

/// Fixed offset of Moscow time from UTC, in seconds.
pub const MOSCOW_OFFSET_SECONDS: u64 = 3 * 60 * 60;
/// Seconds in one day.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Returns the Moscow day index containing `unix_seconds`.
///
/// Counters compare this value instead of instants, so a counter stored
/// yesterday is already stale at 00:00 Moscow regardless of when the server ran.
#[must_use]
pub const fn day_at(unix_seconds: u64) -> u64 {
    (unix_seconds + MOSCOW_OFFSET_SECONDS) / SECONDS_PER_DAY
}

/// Returns the Moscow day index of the current instant.
#[must_use]
pub fn today() -> u64 {
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    day_at(unix_seconds)
}

#[cfg(test)]
mod tests {
    use super::day_at;

    #[test]
    fn the_day_starts_at_midnight_moscow_not_utc() {
        // Midnight in Moscow is 21:00 UTC of the previous UTC day.
        assert_eq!(day_at(0), 0);
        assert_eq!(day_at(75_599), 0);
        assert_eq!(day_at(75_600), 1);
    }

    #[test]
    fn every_instant_of_one_moscow_day_shares_one_index() {
        let start = 86_400 * 3 + 75_600;
        for offset in [0, 1, 3_600, 43_199, 86_399] {
            assert_eq!(day_at(start + offset), 4, "offset {offset}");
        }
        assert_eq!(day_at(start + 86_400), 5);
    }
}
