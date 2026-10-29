//! The personal System shop: ten fixed offers, each buyable once per Moscow day.
//!
//! Prices are paid in system points, which only personal mob kills, boss pools,
//! and dungeon rewards produce, so trading cannot fund the shop. Every purchase
//! is bound to its buyer: the item can be used, upgraded and dropped, but never
//! handed to another hunter.

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use crate::item::artifact::{self, ArtifactId};
use crate::item::binding;
use crate::item::chest::ChestKind;
use crate::item::potion::BuffPotion;

/// How many offers the shop has.
pub const SLOTS: usize = 10;

/// The concrete item behind one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferItem {
    /// A themed buff potion.
    Buff(BuffPotion),
    /// A reward chest; its contents roll when it is opened.
    Chest(ChestKind),
    /// An artifact that starts personal and stays personal.
    Artifact(ArtifactId),
}

/// One shop slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// Slot index used by the purchase command and the daily counters.
    pub slot: usize,
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
            OfferItem::Buff(buff) => binding::bound(buff.stack(), owner),
            OfferItem::Chest(kind) => binding::bound(kind.stack(), owner),
            OfferItem::Artifact(id) => artifact::stack(id, Some(owner)),
        }
    }

    /// Returns the Russian name shown in the shop list.
    #[must_use]
    pub fn display_name(self) -> String {
        match self.item {
            OfferItem::Buff(buff) => buff.display_name().to_owned(),
            OfferItem::Chest(kind) => kind.display_name().to_owned(),
            OfferItem::Artifact(id) => id.artifact().name.to_owned(),
        }
    }
}

/// The ten offers, in slot order.
///
/// The catalog mixes consumables with the three chest kinds and the three Rare
/// artifacts a hunter cannot otherwise get before finding a dungeon of that
/// rank, so system points always buy something the player actually wants.
pub const OFFERS: [Offer; SLOTS] = [
    Offer {
        slot: 0,
        price: 20,
        item: OfferItem::Buff(BuffPotion::Strength),
    },
    Offer {
        slot: 1,
        price: 20,
        item: OfferItem::Buff(BuffPotion::Speed),
    },
    Offer {
        slot: 2,
        price: 25,
        item: OfferItem::Buff(BuffPotion::Resistance),
    },
    Offer {
        slot: 3,
        price: 25,
        item: OfferItem::Buff(BuffPotion::Regeneration),
    },
    Offer {
        slot: 4,
        price: 100,
        item: OfferItem::Chest(ChestKind::Normal),
    },
    Offer {
        slot: 5,
        price: 250,
        item: OfferItem::Chest(ChestKind::Sacred),
    },
    Offer {
        slot: 6,
        price: 500,
        item: OfferItem::Chest(ChestKind::Choice),
    },
    Offer {
        slot: 7,
        price: 400,
        item: OfferItem::Artifact(ArtifactId::KasakasVenomFang),
    },
    Offer {
        slot: 8,
        price: 400,
        item: OfferItem::Artifact(ArtifactId::ArchersGloves),
    },
    Offer {
        slot: 9,
        price: 450,
        item: OfferItem::Artifact(ArtifactId::AssassinsShoes),
    },
];

/// Returns the offer in `slot`.
#[must_use]
pub fn offer(slot: usize) -> Option<Offer> {
    OFFERS.get(slot).copied()
}

/// What one hunter bought during one Moscow day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopDay {
    /// Moscow day index the flags below belong to.
    pub day: u64,
    /// Slots already bought that day.
    pub purchased: [bool; SLOTS],
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
    /// rolls the counters over exactly once.
    #[must_use]
    pub const fn unset() -> Self {
        Self {
            day: u64::MAX,
            purchased: [false; SLOTS],
        }
    }

    /// Clears yesterday's purchases and reports whether anything changed.
    pub const fn roll_over(&mut self, today: u64) -> bool {
        if self.day == today {
            return false;
        }
        self.day = today;
        self.purchased = [false; SLOTS];
        true
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
    /// The purchase could not be persisted.
    Storage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_is_offered_exactly_once_with_a_positive_price() {
        let mut seen = [false; SLOTS];
        for offer in OFFERS {
            assert!(offer.price > 0, "slot {}", offer.slot);
            assert!(!seen[offer.slot], "slot {} is offered twice", offer.slot);
            seen[offer.slot] = true;
        }
        assert!(seen.iter().all(|slot| *slot));
        assert!(offer(SLOTS).is_none());
    }

    #[test]
    fn a_purchase_is_refused_twice_on_one_day_but_allowed_the_next() {
        let mut day = ShopDay::default();
        assert!(day.roll_over(5));
        assert!(!day.bought(3));
        assert!(day.mark(3));
        assert!(day.bought(3));
        assert!(!day.mark(3));

        assert!(!day.roll_over(5));
        assert!(day.bought(3));
        assert!(day.roll_over(6));
        assert!(!day.bought(3));
        assert!(day.mark(3));
    }

    #[test]
    fn unknown_slots_are_never_marked() {
        let mut day = ShopDay::default();
        day.roll_over(1);
        assert!(!day.mark(SLOTS));
        assert!(!day.bought(SLOTS));
    }
}
