//! The city merchant: serial hunter gear for Won.
//!
//! Two showcases sell transferable, ordinary-wear copies:
//!
//! - the **shared** showcase is the same for every hunter and changes once an
//!   hour. It always carries replacements of the starter kit, the starter-line
//!   weapons (swords, axes, bows and crossbows) and the Mage staff, adds four
//!   rotating specialization pieces, and closes with three arrow packs;
//! - the **personal** showcase changes every twenty minutes, offers three
//!   specialization pieces for the hunter's class at a discount, and each of
//!   its offers can be bought once per period.
//!
//! The arrow packs are always the same: ordinary arrows for everyone,
//! spectral arrows from rank C and poison-tipped arrows from rank A.
//!
//! Both are derived from the wall-clock period (and the hunter for the
//! personal one), so reconnecting or restarting can never reroll them; only
//! the personal purchase marks are saved. A request names the period it was
//! made in and is refused without a debit once that period has passed.
//!
//! The System shop is a separate store and stays unchanged.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use steel_registry::data_components::PotionContents;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_items, vanilla_potions};
use uuid::Uuid;

use crate::class::HunterClass;
use crate::item::artifact::{self, ArmorLine, Rarity};
use crate::item::binding;
use crate::item::material::{self, Material, Quality};
use crate::item::serial::{self, MAGE_SHOP, SPECIALIZATION, STARTER, SerialId, WEAPON_SHOP};
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

/// A pack of arrows sold by the shared showcase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowPack {
    /// Ordinary arrows, on sale from the start.
    Arrow,
    /// Spectral arrows, on sale from rank C.
    Spectral,
    /// Poison-tipped arrows, on sale from rank A.
    Tipped,
}

/// Arrow packs always listed by the shared showcase, worst first.
pub const ARROW_PACKS: [ArrowPack; 3] = [ArrowPack::Arrow, ArrowPack::Spectral, ArrowPack::Tipped];

/// Ordinary arrows in one pack.
pub const ARROW_PACK_COUNT: i32 = 16;
/// Spectral arrows in one pack.
pub const SPECTRAL_PACK_COUNT: i32 = 8;
/// Tipped arrows in one pack.
pub const TIPPED_PACK_COUNT: i32 = 8;
/// Price of one ordinary arrow pack, in Won.
pub const ARROW_PACK_PRICE: u64 = 50;
/// Price of one spectral arrow pack, in Won.
pub const SPECTRAL_PACK_PRICE: u64 = 200;
/// Price of one tipped arrow pack, in Won.
pub const TIPPED_PACK_PRICE: u64 = 450;

impl ArrowPack {
    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Arrow => "Обычные стрелы",
            Self::Spectral => "Спектральные стрелы",
            Self::Tipped => "Отравленные стрелы",
        }
    }

    /// Returns how many arrows one pack holds.
    #[must_use]
    pub const fn count(self) -> i32 {
        match self {
            Self::Arrow => ARROW_PACK_COUNT,
            Self::Spectral => SPECTRAL_PACK_COUNT,
            Self::Tipped => TIPPED_PACK_COUNT,
        }
    }

    /// Returns the price of one pack in Won.
    #[must_use]
    pub const fn price(self) -> u64 {
        match self {
            Self::Arrow => ARROW_PACK_PRICE,
            Self::Spectral => SPECTRAL_PACK_PRICE,
            Self::Tipped => TIPPED_PACK_PRICE,
        }
    }

    /// Returns the lowest rank that may buy this pack.
    #[must_use]
    pub const fn rank(self) -> HunterRank {
        match self {
            Self::Arrow => HunterRank::E,
            Self::Spectral => HunterRank::C,
            Self::Tipped => HunterRank::A,
        }
    }

    /// Builds the stack one pack grants.
    #[must_use]
    pub fn stack(self) -> ItemStack {
        match self {
            Self::Arrow => ItemStack::with_count(&vanilla_items::ARROW, ARROW_PACK_COUNT),
            Self::Spectral => {
                ItemStack::with_count(&vanilla_items::SPECTRAL_ARROW, SPECTRAL_PACK_COUNT)
            }
            // Poison matches the vanilla default tipped arrow of this core.
            Self::Tipped => {
                let mut stack = PotionContents::create_item_stack(
                    &vanilla_items::TIPPED_ARROW,
                    &vanilla_potions::POISON,
                );
                stack.set_count(TIPPED_PACK_COUNT);
                stack
            }
        }
    }
}

/// What one merchant offer sells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goods {
    /// A transferable serial catalog item.
    Serial(SerialId),
    /// A pack of arrows.
    Arrows(ArrowPack),
}

impl Goods {
    /// Builds the stack the hunter receives.
    #[must_use]
    pub fn stack(self) -> ItemStack {
        match self {
            Self::Serial(id) => serial::stack(id, None),
            Self::Arrows(pack) => pack.stack(),
        }
    }

    /// Returns the Russian name used in purchase messages.
    #[must_use]
    pub fn display_name(self) -> String {
        match self {
            Self::Serial(id) => id.serial().name.to_owned(),
            Self::Arrows(pack) => {
                format!("{} ×{}", pack.display_name(), pack.count())
            }
        }
    }
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
    /// The goods sold.
    pub item: Goods,
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
pub const fn base_price(item: SerialId) -> Option<u64> {
    let price = match item {
        SerialId::HunterSword
        | SerialId::HunterDagger
        | SerialId::HunterAxe
        | SerialId::HunterLeggings
        | SerialId::ApprenticeStaff
        | SerialId::HunterBow => 300,
        SerialId::HunterShield
        | SerialId::HunterHelmet
        | SerialId::HunterBoots
        | SerialId::HunterPickaxe => 150,
        SerialId::HunterChestplate | SerialId::HunterCrossbow => 350,
        SerialId::LightHelmet
        | SerialId::LightBoots
        | SerialId::ReinforcedHelmet
        | SerialId::ReinforcedBoots
        | SerialId::HeavyHelmet
        | SerialId::HeavyBoots
        | SerialId::RobeHelmet
        | SerialId::RobeBoots => 250,
        SerialId::LightLeggings
        | SerialId::ReinforcedLeggings
        | SerialId::HeavyLeggings
        | SerialId::RobeLeggings => 450,
        SerialId::LightChestplate
        | SerialId::ReinforcedChestplate
        | SerialId::HeavyChestplate
        | SerialId::RobeChestplate => 550,
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
    if let Some(id) = serial::of(stack) {
        let each = base_price(id).map_or(30, |price| price * 30 / 100);
        return Some(each * count);
    }
    let _id = artifact::of(stack)?;
    let each = match artifact::rarity_of(stack)? {
        Rarity::Common => 30,
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
/// then the always-on-sale weapons and the rotating specialization pieces,
/// then the arrow packs.
#[must_use]
pub fn shared_showcase(period: u64) -> Vec<MerchantOffer> {
    let rotating = pick_distinct(&SPECIALIZATION, SHARED_ROTATING, period ^ 0x5348_4152_4544);
    STARTER
        .into_iter()
        .chain(MAGE_SHOP)
        .chain(WEAPON_SHOP)
        .chain(rotating)
        .map(Goods::Serial)
        .chain(ARROW_PACKS.into_iter().map(Goods::Arrows))
        .enumerate()
        .filter_map(|(index, item)| {
            let price = match item {
                Goods::Serial(id) => base_price(id)?,
                Goods::Arrows(pack) => pack.price(),
            };
            Some(MerchantOffer {
                showcase: Showcase::Shared,
                period,
                index,
                item,
                price,
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
    let pool: Vec<SerialId> = SPECIALIZATION
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
                item: Goods::Serial(item),
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
pub fn may_buy(item: Goods, rank: HunterRank) -> bool {
    match item {
        Goods::Serial(id) => id.line().is_none() || rank >= SPECIALIZATION_RANK,
        Goods::Arrows(pack) => rank >= pack.rank(),
    }
}

/// Picks `count` distinct entries of `pool` from `seed`.
fn pick_distinct(pool: &[SerialId], count: usize, seed: u64) -> Vec<SerialId> {
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
        ARROW_PACKS, Goods, MerchantRecord, PERSONAL_OFFERS, SHARED_ROTATING, Showcase, base_price,
        may_buy, personal_showcase, shared_showcase,
    };
    use crate::class::HunterClass;
    use crate::item::serial::{MAGE_SHOP, SPECIALIZATION, STARTER, SerialId, WEAPON_SHOP};
    use crate::rank::HunterRank;

    #[test]
    fn every_sold_item_has_a_price() {
        for item in STARTER.into_iter().chain(SPECIALIZATION).chain(WEAPON_SHOP) {
            assert!(base_price(item).is_some(), "{item:?}");
        }
        assert_eq!(base_price(SerialId::ImprovedHunterSword), None);
        assert_eq!(base_price(SerialId::HunterBow), Some(300));
        assert_eq!(base_price(SerialId::HunterCrossbow), Some(350));
    }

    #[test]
    fn the_shared_showcase_always_has_the_starter_replacements() {
        for period in 0..50 {
            let offers = shared_showcase(period);
            assert_eq!(
                offers.len(),
                STARTER.len()
                    + MAGE_SHOP.len()
                    + WEAPON_SHOP.len()
                    + SHARED_ROTATING
                    + ARROW_PACKS.len()
            );
            for item in STARTER.into_iter().chain(WEAPON_SHOP) {
                assert!(offers.iter().any(|offer| offer.item == Goods::Serial(item)));
            }
            assert_eq!(shared_showcase(period), offers, "a period never rerolls");
        }
    }

    #[test]
    fn arrow_packs_are_always_on_sale_and_rank_gated() {
        let offers = shared_showcase(7);
        for pack in ARROW_PACKS {
            let offer = offers
                .iter()
                .find(|offer| offer.item == Goods::Arrows(pack))
                .unwrap_or_else(|| panic!("{pack:?} must be on sale"));
            assert_eq!(offer.price, pack.price());
            assert_eq!(offer.showcase, Showcase::Shared);
            let stack = offer.item.stack();
            assert_eq!(stack.count, pack.count(), "{pack:?}");
        }
        assert!(may_buy(
            Goods::Arrows(super::ArrowPack::Arrow),
            HunterRank::E
        ));
        assert!(!may_buy(
            Goods::Arrows(super::ArrowPack::Spectral),
            HunterRank::D
        ));
        assert!(may_buy(
            Goods::Arrows(super::ArrowPack::Spectral),
            HunterRank::C
        ));
        assert!(!may_buy(
            Goods::Arrows(super::ArrowPack::Tipped),
            HunterRank::B
        ));
        assert!(may_buy(
            Goods::Arrows(super::ArrowPack::Tipped),
            HunterRank::A
        ));
    }

    #[test]
    fn personal_offers_fit_the_class_and_are_discounted() {
        let hunter = Uuid::new_v4();
        for period in 0..50 {
            let offers = personal_showcase(hunter, Some(HunterClass::Mage), period);
            assert_eq!(offers.len(), PERSONAL_OFFERS);
            for offer in &offers {
                assert_eq!(offer.showcase, Showcase::Personal);
                let Goods::Serial(id) = offer.item else {
                    panic!("personal offers are always serials");
                };
                assert!(
                    id.allowed_classes()
                        .is_some_and(|classes| { classes.contains(&HunterClass::Mage) })
                );
                assert!(offer.price < base_price(id).unwrap_or(0));
            }
            assert_eq!(
                personal_showcase(hunter, Some(HunterClass::Mage), period),
                offers
            );
        }
    }

    #[test]
    fn specialization_needs_rank_d() {
        assert!(may_buy(Goods::Serial(SerialId::HunterSword), HunterRank::E));
        assert!(may_buy(Goods::Serial(SerialId::HunterBow), HunterRank::E));
        assert!(!may_buy(
            Goods::Serial(SerialId::LightChestplate),
            HunterRank::E
        ));
        assert!(may_buy(
            Goods::Serial(SerialId::LightChestplate),
            HunterRank::D
        ));
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
