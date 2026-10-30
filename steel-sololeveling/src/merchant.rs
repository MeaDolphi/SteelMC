//! The city merchant: serial hunter gear for Won.
//!
//! Two showcases sell transferable, ordinary-wear copies:
//!
//! - the **shared** showcase is the same for every hunter and changes once an
//!   hour. It always carries replacements of the starter kit and adds four
//!   rotating specialization pieces;
//! - the **personal** showcase changes every twenty minutes, offers three
//!   specialization pieces for the hunter's class at a discount, and each of
//!   its offers can be bought once per period.
//!
//! Both are derived from the wall-clock period (and the hunter for the
//! personal one), so reconnecting or restarting can never reroll them; only
//! the personal purchase marks are saved. A request names the period it was
//! made in and is refused without a debit once that period has passed.
//!
//! The System shop is a separate store and stays unchanged.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use crate::class::HunterClass;
use crate::item::artifact::{
    self, ArmorLine, ArtifactId, MAGE_SHOP, Rarity, SPECIALIZATION, STARTER,
};
use crate::item::binding;
use crate::item::material::{self, Material, Quality};
use crate::rank::HunterRank;

/// Seconds one shared showcase stays up.
pub const SHARED_PERIOD_SECONDS: u64 = 60 * 60;
/// Seconds one personal showcase stays up.
pub const PERSONAL_PERIOD_SECONDS: u64 = 20 * 60;
/// Rotating specialization pieces of the shared showcase.
pub const SHARED_ROTATING: usize = 4;
/// Offers of the personal showcase.
pub const PERSONAL_OFFERS: usize = 3;
/// Discount of the personal showcase, in percent.
pub const PERSONAL_DISCOUNT_PERCENT: u64 = 15;
/// Lowest rank that may buy specialization armor: classes open there.
pub const SPECIALIZATION_RANK: HunterRank = HunterRank::D;

/// Which showcase an offer comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showcase {
    /// The hourly showcase every hunter sees.
    Shared,
    /// The hunter's own twenty-minute showcase.
    Personal,
}

/// One merchant offer, as shown and as requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MerchantOffer {
    /// Showcase the offer belongs to.
    pub showcase: Showcase,
    /// Period index the offer was rolled for.
    pub period: u64,
    /// Position in its showcase.
    pub index: usize,
    /// The item sold.
    pub item: ArtifactId,
    /// Price in Won.
    pub price: u64,
}

/// Personal-showcase purchases of the current period, saved with the hunter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerchantRecord {
    /// Personal period the marks below belong to.
    #[serde(default)]
    pub period: u64,
    /// Personal offers already bought in that period.
    #[serde(default)]
    pub bought: [bool; PERSONAL_OFFERS],
}

impl MerchantRecord {
    /// Returns whether personal offer `index` was bought in `period`.
    #[must_use]
    pub fn bought(self, period: u64, index: usize) -> bool {
        self.period == period && self.bought.get(index).copied().unwrap_or(false)
    }

    /// Marks personal offer `index` of `period` as bought.
    pub fn mark(&mut self, period: u64, index: usize) {
        if self.period != period {
            *self = Self {
                period,
                bought: [false; PERSONAL_OFFERS],
            };
        }
        if let Some(bought) = self.bought.get_mut(index) {
            *bought = true;
        }
    }
}

/// Returns the base price of `item` in Won, or `None` if it is not sold.
#[must_use]
pub const fn base_price(item: ArtifactId) -> Option<u64> {
    let price = match item {
        ArtifactId::HunterSword
        | ArtifactId::HunterDagger
        | ArtifactId::HunterAxe
        | ArtifactId::HunterLeggings
        | ArtifactId::ApprenticeStaff => 300,
        ArtifactId::HunterShield
        | ArtifactId::HunterHelmet
        | ArtifactId::HunterBoots
        | ArtifactId::HunterPickaxe => 150,
        ArtifactId::HunterChestplate => 350,
        ArtifactId::LightHelmet
        | ArtifactId::LightBoots
        | ArtifactId::ReinforcedHelmet
        | ArtifactId::ReinforcedBoots
        | ArtifactId::HeavyHelmet
        | ArtifactId::HeavyBoots
        | ArtifactId::RobeHelmet
        | ArtifactId::RobeBoots => 250,
        ArtifactId::LightLeggings
        | ArtifactId::ReinforcedLeggings
        | ArtifactId::HeavyLeggings
        | ArtifactId::RobeLeggings => 450,
        ArtifactId::LightChestplate
        | ArtifactId::ReinforcedChestplate
        | ArtifactId::HeavyChestplate
        | ArtifactId::RobeChestplate => 550,
        _ => return None,
    };
    Some(price)
}

/// Returns the Won the merchant pays for `stack`, or `None` when he does not
/// buy it: personal items, crystals (the Organization buys those) and
/// anything that is neither a material nor an artifact.
#[must_use]
pub fn buyback_price(stack: &ItemStack) -> Option<u64> {
    if stack.is_empty() || binding::bound_owner(stack).is_some() {
        return None;
    }
    let count = u64::try_from(stack.count).ok()?;
    if let Some((material, quality)) = material::of(stack) {
        let unit = match quality {
            Quality::Common => 10,
            Quality::Reinforced => 30,
            Quality::Superior => 90,
        };
        let unit = if material == Material::StableEssence {
            unit * 3
        } else {
            unit
        };
        return Some(unit * count);
    }
    let id = artifact::of(stack)?;
    let each = match artifact::rarity_of(stack)? {
        Rarity::Common => base_price(id).map_or(30, |price| price * 30 / 100),
        Rarity::Rare => 150,
        Rarity::Epic => 600,
        Rarity::Legendary => 2_500,
    };
    Some(each * count)
}

/// Returns the current Unix time in seconds.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Returns the shared period running at `unix_seconds`.
#[must_use]
pub const fn shared_period(unix_seconds: u64) -> u64 {
    unix_seconds / SHARED_PERIOD_SECONDS
}

/// Returns the personal period running at `unix_seconds`.
#[must_use]
pub const fn personal_period(unix_seconds: u64) -> u64 {
    unix_seconds / PERSONAL_PERIOD_SECONDS
}

/// Returns the seconds left until the period of `length` ends.
#[must_use]
pub const fn seconds_left(unix_seconds: u64, length: u64) -> u64 {
    length - unix_seconds % length
}

/// Returns the shared showcase of `period`: the starter replacements first,
/// then the rotating specialization pieces.
#[must_use]
pub fn shared_showcase(period: u64) -> Vec<MerchantOffer> {
    let rotating = pick_distinct(&SPECIALIZATION, SHARED_ROTATING, period ^ 0x5348_4152_4544);
    STARTER
        .into_iter()
        .chain(MAGE_SHOP)
        .chain(rotating)
        .enumerate()
        .filter_map(|(index, item)| {
            Some(MerchantOffer {
                showcase: Showcase::Shared,
                period,
                index,
                item,
                price: base_price(item)?,
            })
        })
        .collect()
}

/// Returns the personal showcase of `hunter` for `period`.
///
/// A hunter with a class sees only the armor their class may wear; a hunter
/// without one sees every line.
#[must_use]
pub fn personal_showcase(
    hunter: Uuid,
    class: Option<HunterClass>,
    period: u64,
) -> Vec<MerchantOffer> {
    let pool: Vec<ArtifactId> = SPECIALIZATION
        .into_iter()
        .filter(|item| class.is_none_or(|class| item.line().is_some_and(|line| fits(line, class))))
        .collect();
    let (high, low) = hunter.as_u64_pair();
    let seed = period ^ high.rotate_left(17) ^ low;
    pick_distinct(&pool, PERSONAL_OFFERS, seed)
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let price = base_price(item)? * (100 - PERSONAL_DISCOUNT_PERCENT) / 100;
            Some(MerchantOffer {
                showcase: Showcase::Personal,
                period,
                index,
                item,
                price,
            })
        })
        .collect()
}

fn fits(line: ArmorLine, class: HunterClass) -> bool {
    line.classes().contains(&class)
}

/// Returns whether a hunter of `rank` may buy `item`.
#[must_use]
pub fn may_buy(item: ArtifactId, rank: HunterRank) -> bool {
    item.line().is_none() || rank >= SPECIALIZATION_RANK
}

/// Picks `count` distinct entries of `pool` from `seed`.
fn pick_distinct(pool: &[ArtifactId], count: usize, seed: u64) -> Vec<ArtifactId> {
    let mut remaining = pool.to_vec();
    let mut state = seed;
    let mut picked = Vec::with_capacity(count);
    while picked.len() < count && !remaining.is_empty() {
        state = splitmix64(state);
        let index = usize::try_from(state % remaining.len() as u64).unwrap_or(0);
        picked.push(remaining.swap_remove(index));
    }
    picked
}

const fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        MerchantRecord, PERSONAL_OFFERS, SHARED_ROTATING, Showcase, base_price, may_buy,
        personal_showcase, shared_showcase,
    };
    use crate::class::HunterClass;
    use crate::item::artifact::{ArtifactId, MAGE_SHOP, SPECIALIZATION, STARTER};
    use crate::rank::HunterRank;

    #[test]
    fn every_sold_item_has_a_price() {
        for item in STARTER.into_iter().chain(SPECIALIZATION) {
            assert!(base_price(item).is_some(), "{item:?}");
        }
        assert_eq!(base_price(ArtifactId::RedKnightsHelmet), None);
    }

    #[test]
    fn the_shared_showcase_always_has_the_starter_replacements() {
        for period in 0..50 {
            let offers = shared_showcase(period);
            assert_eq!(
                offers.len(),
                STARTER.len() + MAGE_SHOP.len() + SHARED_ROTATING
            );
            for item in STARTER {
                assert!(offers.iter().any(|offer| offer.item == item));
            }
            assert_eq!(shared_showcase(period), offers, "a period never rerolls");
        }
    }

    #[test]
    fn personal_offers_fit_the_class_and_are_discounted() {
        let hunter = Uuid::new_v4();
        for period in 0..50 {
            let offers = personal_showcase(hunter, Some(HunterClass::Mage), period);
            assert_eq!(offers.len(), PERSONAL_OFFERS);
            for offer in &offers {
                assert_eq!(offer.showcase, Showcase::Personal);
                assert!(
                    offer
                        .item
                        .allowed_classes()
                        .is_some_and(|classes| { classes.contains(&HunterClass::Mage) })
                );
                assert!(offer.price < base_price(offer.item).unwrap_or(0));
            }
            assert_eq!(
                personal_showcase(hunter, Some(HunterClass::Mage), period),
                offers
            );
        }
    }

    #[test]
    fn specialization_needs_rank_d() {
        assert!(may_buy(ArtifactId::HunterSword, HunterRank::E));
        assert!(!may_buy(ArtifactId::LightChestplate, HunterRank::E));
        assert!(may_buy(ArtifactId::LightChestplate, HunterRank::D));
    }

    #[test]
    fn personal_marks_reset_with_the_period() {
        let mut record = MerchantRecord::default();
        record.mark(7, 1);
        assert!(record.bought(7, 1));
        assert!(!record.bought(8, 1));
        record.mark(8, 0);
        assert!(record.bought(8, 0));
        assert!(!record.bought(8, 1));
    }
}
