//! Rank-scaled hunter rewards, independent of vanilla experience-orb drops.

use crate::rank::HunterRank;

/// Experience and personal System currency credited to one hunter.
#[derive(Debug, Clone, Copy)]
pub struct HunterReward {
    /// Hunter experience, not vanilla experience-orb points.
    pub experience: u64,
    /// Personal System currency.
    pub system_points: u64,
}

impl HunterReward {
    /// Uses the dungeon's effective rank, already promoted for red gates.
    #[must_use]
    pub const fn for_mob(rank: HunterRank) -> Self {
        let scale = match rank {
            HunterRank::E => 1,
            HunterRank::D => 2,
            HunterRank::C => 4,
            HunterRank::B => 8,
            HunterRank::A => 16,
            HunterRank::S => 32,
        };
        Self {
            experience: 10 * scale,
            system_points: 2 * scale,
        }
    }

    /// Divides the boss pool (twenty ordinary kills) without losing remainders.
    /// The caller supplies survivors in their original dungeon-entry order.
    #[must_use]
    pub fn boss_shares(rank: HunterRank, survivors: usize) -> Vec<Self> {
        if survivors == 0 {
            return Vec::new();
        }
        let ordinary = Self::for_mob(rank);
        let experience = ordinary.experience * 20;
        let points = ordinary.system_points * 20;
        let count = survivors as u64;
        (0..survivors)
            .map(|index| Self {
                experience: experience / count + u64::from((index as u64) < experience % count),
                system_points: points / count + u64::from((index as u64) < points % count),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_currency_distributes_its_own_remainder_in_entry_order() {
        let shares = HunterReward::boss_shares(HunterRank::C, 3);
        assert_eq!(
            shares
                .iter()
                .map(|reward| reward.experience)
                .collect::<Vec<_>>(),
            [267, 267, 266]
        );
        assert_eq!(
            shares
                .iter()
                .map(|reward| reward.system_points)
                .collect::<Vec<_>>(),
            [54, 53, 53]
        );
        assert_eq!(
            shares.iter().map(|reward| reward.experience).sum::<u64>(),
            800
        );
        assert_eq!(
            shares
                .iter()
                .map(|reward| reward.system_points)
                .sum::<u64>(),
            160
        );
        assert!(HunterReward::boss_shares(HunterRank::S, 0).is_empty());
    }
}
