//! Hunter rank derived from level.

/// A hunter's automatic rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HunterRank {
    /// Levels 1 through 9.
    E,
    /// Levels 10 through 24.
    D,
    /// Levels 25 through 49.
    C,
    /// Levels 50 through 74.
    B,
    /// Levels 75 through 99.
    A,
    /// Level 100 and above.
    S,
}

impl HunterRank {
    /// Calculates the rank assigned to `level`.
    #[must_use]
    pub const fn from_level(level: u32) -> Self {
        match level {
            ..=9 => Self::E,
            10..=24 => Self::D,
            25..=49 => Self::C,
            50..=74 => Self::B,
            75..=99 => Self::A,
            100.. => Self::S,
        }
    }

    /// Returns the next higher rank, capped at [`HunterRank::S`].
    #[must_use]
    pub const fn promoted(self) -> Self {
        match self {
            Self::E => Self::D,
            Self::D => Self::C,
            Self::C => Self::B,
            Self::B => Self::A,
            Self::A | Self::S => Self::S,
        }
    }

    /// Returns the one-letter rank displayed to players.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::E => "E",
            Self::D => "D",
            Self::C => "C",
            Self::B => "B",
            Self::A => "A",
            Self::S => "S",
        }
    }

    /// Parses a one-letter rank stored in item data or quest records.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "E" => Some(Self::E),
            "D" => Some(Self::D),
            "C" => Some(Self::C),
            "B" => Some(Self::B),
            "A" => Some(Self::A),
            "S" => Some(Self::S),
            _ => None,
        }
    }

    /// Returns the payment multiplier of a job of this rank.
    #[must_use]
    pub const fn payment_multiplier(self) -> u64 {
        match self {
            Self::E => 1,
            Self::D => 2,
            Self::C => 4,
            Self::B => 8,
            Self::A => 16,
            Self::S => 32,
        }
    }

    /// Crystal veins generated in a dungeon of this rank.
    #[must_use]
    pub const fn crystal_vein_count(self) -> usize {
        match self {
            Self::E => 0,
            Self::D => 1,
            Self::C => 3,
            Self::B => 5,
            Self::A => 7,
            Self::S => 9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HunterRank;

    #[test]
    fn promotion_stops_at_s_rank() {
        assert_eq!(HunterRank::E.promoted(), HunterRank::D);
        assert_eq!(HunterRank::A.promoted(), HunterRank::S);
        assert_eq!(HunterRank::S.promoted(), HunterRank::S);
    }

    #[test]
    fn crystal_veins_follow_the_confirmed_odd_count_progression() {
        assert_eq!(
            [
                HunterRank::E,
                HunterRank::D,
                HunterRank::C,
                HunterRank::B,
                HunterRank::A,
                HunterRank::S,
            ]
            .map(HunterRank::crystal_vein_count),
            [0, 1, 3, 5, 7, 9]
        );
    }

    #[test]
    fn level_boundaries_map_to_the_confirmed_rank_ranges() {
        assert_eq!(HunterRank::from_level(1), HunterRank::E);
        assert_eq!(HunterRank::from_level(9), HunterRank::E);
        assert_eq!(HunterRank::from_level(10), HunterRank::D);
        assert_eq!(HunterRank::from_level(24), HunterRank::D);
        assert_eq!(HunterRank::from_level(25), HunterRank::C);
        assert_eq!(HunterRank::from_level(49), HunterRank::C);
        assert_eq!(HunterRank::from_level(50), HunterRank::B);
        assert_eq!(HunterRank::from_level(74), HunterRank::B);
        assert_eq!(HunterRank::from_level(75), HunterRank::A);
        assert_eq!(HunterRank::from_level(99), HunterRank::A);
        assert_eq!(HunterRank::from_level(100), HunterRank::S);
    }
}
