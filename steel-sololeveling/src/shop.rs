//! The personal System shop: ten slots rolled per hunter per Moscow day.
//!
//! Each slot independently rolls a category from the shop table, then a
//! uniform item from that category's catalog. The roll is saved with the
//! hunter, so reconnecting or restarting cannot reroll the day's slots.
//!
//! Prices are paid in system points, which only personal mob kills, boss pools,
//! and dungeon rewards produce, so trading cannot fund the shop. Every purchase,
//! food and potions included, is bound to its buyer: the item can be used,
//! upgraded and dropped, but never handed to another hunter.

use std::array;

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use uuid::Uuid;

use crate::item::artifact::{self, ArtifactId, Rarity};
use crate::item::binding;
use crate::item::potion::{self, BuffPotion};

/// How many offers the shop has.
pub const SLOTS: usize = 10;
/// Items in one food offer.
pub const FOOD_COUNT: i32 = 8;

/// Food the shop sells in stacks of [`FOOD_COUNT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Food {
    /// Baked potatoes.
    BakedPotato,
    /// Cooked beef.
    CookedBeef,
}

/// The concrete item behind one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferItem {
    /// A stack of food.
    Food(Food),
    /// A themed buff potion.
    Buff(BuffPotion),
    /// An artifact that starts personal and stays personal.
    Artifact(ArtifactId),
}

/// One shop slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// Price in system points.
    pub price: u64,
    /// What the hunter receives.
    pub item: OfferItem,
}

impl Offer {
    /// Builds the item this offer grants, bound to its buyer.
    #[must_use]
    pub fn stack(self, owner: Uuid) -> ItemStack {
        match self.item {
            OfferItem::Food(Food::BakedPotato) => binding::bound(
                ItemStack::with_count(&vanilla_items::BAKED_POTATO, FOOD_COUNT),
                owner,
            ),
            OfferItem::Food(Food::CookedBeef) => binding::bound(
                ItemStack::with_count(&vanilla_items::COOKED_BEEF, FOOD_COUNT),
                owner,
            ),
            OfferItem::Buff(buff) => binding::bound(buff.stack(), owner),
            OfferItem::Artifact(id) => artifact::stack(id, Some(owner)),
        }
    }

    /// Returns the Russian name shown in the shop list.
    #[must_use]
    pub fn display_name(self) -> String {
        match self.item {
            OfferItem::Food(Food::BakedPotato) => format!("Печёный картофель ×{FOOD_COUNT}"),
            OfferItem::Food(Food::CookedBeef) => format!("Жареная говядина ×{FOOD_COUNT}"),
            OfferItem::Buff(buff) => buff.display_name().to_owned(),
            OfferItem::Artifact(id) => {
                let artifact = id.artifact();
                format!("{} ({})", artifact.name, artifact.rarity.display_name())
            }
        }
    }
}

/// One row of the shop table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Food(Food),
    Buff,
    Weapon(Rarity),
    Armor(Rarity),
}

/// The shop table: chance in per mille, category, and price in system points.
///
/// The chances add up to exactly 1000.
const TABLE: [(u32, Category, u64); 9] = [
    (200, Category::Food(Food::BakedPotato), 10),
    (200, Category::Food(Food::CookedBeef), 15),
    (350, Category::Buff, 40),
    (100, Category::Weapon(Rarity::Rare), 120),
    (100, Category::Armor(Rarity::Rare), 100),
    (20, Category::Weapon(Rarity::Epic), 600),
    (20, Category::Armor(Rarity::Epic), 500),
    (5, Category::Weapon(Rarity::Legendary), 3_000),
    (5, Category::Armor(Rarity::Legendary), 2_500),
];

/// Rolls one slot from a per-mille `chance` roll and an item `pick` roll.
///
/// `chance` is taken modulo 1000 and `pick` modulo the category's catalog
/// size, so any two uniform integers give the table's distribution.
#[must_use]
pub fn roll_offer(chance: u32, pick: usize) -> Offer {
    let mut chance = chance % 1_000;
    let (category, price) = TABLE
        .iter()
        .find_map(|(weight, category, price)| {
            if chance < *weight {
                Some((*category, *price))
            } else {
                chance -= weight;
                None
            }
        })
        .unwrap_or((Category::Buff, 40));
    let item = match category {
        Category::Food(food) => OfferItem::Food(food),
        Category::Buff => OfferItem::Buff(potion::ALL[pick % potion::ALL.len()]),
        Category::Weapon(rarity) => pick_artifact(&artifact::weapons_of(rarity), pick),
        Category::Armor(rarity) => pick_artifact(&artifact::armors_of(rarity), pick),
    };
    Offer { price, item }
}

fn pick_artifact(catalog: &[ArtifactId], pick: usize) -> OfferItem {
    catalog
        .get(pick % catalog.len().max(1))
        .map_or(OfferItem::Buff(BuffPotion::Strength), |id| {
            OfferItem::Artifact(*id)
        })
}

fn roll_day() -> [Offer; SLOTS] {
    array::from_fn(|_| roll_offer(rand::random_range(0..1_000), rand::random_range(0..64)))
}

/// One hunter's shop for one Moscow day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopDay {
    /// Moscow day index the slots below belong to.
    pub day: u64,
    /// Slots already bought that day.
    pub purchased: [bool; SLOTS],
    /// The day's rolled offers; `None` until the first roll.
    #[serde(default)]
    pub offers: Option<[Offer; SLOTS]>,
}

impl Default for ShopDay {
    fn default() -> Self {
        Self::unset()
    }
}

impl ShopDay {
    /// Returns the state of a hunter who has bought nothing yet.
    ///
    /// The day is deliberately not a real day index, so the first observation
    /// rolls the slots exactly once.
    #[must_use]
    pub const fn unset() -> Self {
        Self {
            day: u64::MAX,
            purchased: [false; SLOTS],
            offers: None,
        }
    }

    /// Rolls today's slots once and reports whether anything changed.
    ///
    /// A day saved before slots were rolled (an older save) gets its slots on
    /// first use without losing today's purchase flags.
    pub fn roll_over(&mut self, today: u64) -> bool {
        self.roll_over_with(today, roll_day)
    }

    /// Re-rolls a hunter's assortment for `today` and resets today's purchases.
    ///
    /// Reserved for operator maintenance; normal players receive one daily roll.
    pub fn refresh(&mut self, today: u64) {
        self.day = u64::MAX;
        self.roll_over_with(today, roll_day);
    }

    fn roll_over_with(&mut self, today: u64, roll: impl FnOnce() -> [Offer; SLOTS]) -> bool {
        if self.day == today && self.offers.is_some() {
            return false;
        }
        if self.day != today {
            self.day = today;
            self.purchased = [false; SLOTS];
        }
        self.offers = Some(roll());
        true
    }

    /// Returns today's offer in `slot`, once the day is rolled.
    #[must_use]
    pub fn offer(&self, slot: usize) -> Option<Offer> {
        self.offers?.get(slot).copied()
    }

    /// Returns whether `slot` was already bought today.
    #[must_use]
    pub fn bought(&self, slot: usize) -> bool {
        self.purchased.get(slot).copied().unwrap_or(false)
    }

    /// Marks `slot` as bought, refusing an unknown or already bought slot.
    pub fn mark(&mut self, slot: usize) -> bool {
        match self.purchased.get_mut(slot) {
            Some(flag) if !*flag => {
                *flag = true;
                true
            }
            _ => false,
        }
    }
}

/// Why a purchase was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopError {
    /// No offer uses that slot.
    UnknownSlot,
    /// The hunter has no loaded session.
    NoSession,
    /// The slot was already bought today.
    AlreadyBought,
    /// A financial write owns the balance right now.
    Busy,
    /// The hunter cannot afford the price.
    InsufficientPoints,
    /// The hunter has no room for the complete purchased stack.
    InventoryFull,
    /// The purchase could not be persisted.
    Storage,
}

impl ShopError {
    /// Returns how the refusal reads in chat.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnknownSlot => "Такого слота в магазине нет.",
            Self::NoSession => "Сессия охотника не загружена.",
            Self::AlreadyBought => "Этот слот уже куплен сегодня.",
            Self::Busy => "Дождитесь завершения другой финансовой операции.",
            Self::InsufficientPoints => "Недостаточно очков Системы.",
            Self::InventoryFull => "Освободите место в инвентаре для покупки.",
            Self::Storage => "Не удалось сохранить покупку. Обратитесь к администратору.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_covers_exactly_one_thousand_per_mille() {
        assert_eq!(TABLE.iter().map(|(weight, ..)| weight).sum::<u32>(), 1_000);
    }

    #[test]
    fn chance_rolls_land_in_the_table_rows() {
        assert_eq!(roll_offer(0, 0).item, OfferItem::Food(Food::BakedPotato));
        assert_eq!(roll_offer(199, 0).price, 10);
        assert_eq!(roll_offer(200, 0).item, OfferItem::Food(Food::CookedBeef));
        assert_eq!(roll_offer(400, 0).price, 40);
        assert!(matches!(roll_offer(749, 3).item, OfferItem::Buff(_)));
        assert_eq!(roll_offer(750, 0).price, 120);
        assert_eq!(roll_offer(850, 0).price, 100);
        assert_eq!(roll_offer(950, 0).price, 600);
        assert_eq!(roll_offer(970, 0).price, 500);
        assert_eq!(roll_offer(990, 0).price, 3_000);
        assert_eq!(roll_offer(999, 0).price, 2_500);
    }

    #[test]
    fn artifact_rolls_match_their_rarity_and_category() {
        for pick in 0..16 {
            for (chance, rarity, weapon) in [
                (750, Rarity::Rare, true),
                (850, Rarity::Rare, false),
                (950, Rarity::Epic, true),
                (970, Rarity::Epic, false),
                (990, Rarity::Legendary, true),
                (995, Rarity::Legendary, false),
            ] {
                let OfferItem::Artifact(id) = roll_offer(chance, pick).item else {
                    panic!("chance {chance} must roll an artifact");
                };
                let artifact = id.artifact();
                assert_eq!(artifact.rarity, rarity, "chance {chance}");
                assert_eq!(
                    artifact.category == artifact::Category::Weapon,
                    weapon,
                    "chance {chance}"
                );
            }
        }
    }

    #[test]
    fn the_day_is_rolled_once_and_kept_until_the_next_day() {
        let fixed = [roll_offer(0, 0); SLOTS];
        let other = [roll_offer(400, 1); SLOTS];
        let mut day = ShopDay::default();
        assert!(day.roll_over_with(5, || fixed));
        assert!(day.mark(3));
        assert!(!day.roll_over_with(5, || other));
        assert_eq!(day.offer(3), Some(fixed[3]));
        assert!(day.bought(3));
        assert!(!day.mark(3));

        assert!(day.roll_over_with(6, || other));
        assert_eq!(day.offer(3), Some(other[3]));
        assert!(!day.bought(3));
    }

    #[test]
    fn operator_refresh_clears_daily_purchases_and_rolls_a_current_day() {
        let mut day = ShopDay::default();
        day.roll_over_with(5, || [roll_offer(0, 0); SLOTS]);
        assert!(day.mark(3));

        day.refresh(5);

        assert_eq!(day.day, 5);
        assert!(!day.bought(3));
        assert!(day.offer(3).is_some());
    }

    #[test]
    fn administrative_refresh_clears_purchases_and_sets_the_requested_day() {
        let mut day = ShopDay::default();
        day.roll_over_with(5, || [roll_offer(0, 0); SLOTS]);
        assert!(day.mark(3));

        day.refresh(5);

        assert_eq!(day.day, 5);
        assert!(!day.bought(3));
        assert!(day.offer(3).is_some());
    }

    #[test]
    fn an_older_save_without_slots_keeps_todays_purchases() {
        let mut day: ShopDay =
            serde_json::from_str(&format!(r#"{{"day":5,"purchased":{:?}}}"#, [true; SLOTS]))
                .expect("legacy shop day");
        assert_eq!(day.offer(0), None);
        assert!(day.roll_over_with(5, || [roll_offer(0, 0); SLOTS]));
        assert!(day.offer(0).is_some());
        assert!(day.bought(0));
    }

    #[test]
    fn a_saved_day_round_trips() {
        let mut day = ShopDay::default();
        day.roll_over(9);
        let json = serde_json::to_string(&day).expect("serialize");
        let restored: ShopDay = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, day);
    }

    #[test]
    fn unknown_slots_are_never_marked_or_offered() {
        let mut day = ShopDay::default();
        day.roll_over(1);
        assert!(!day.mark(SLOTS));
        assert!(!day.bought(SLOTS));
        assert!(day.offer(SLOTS).is_none());
    }
}
