//! The Association's one-time starter kit.
//!
//! Every hunter, new or existing, may claim one free kit: a weapon of their
//! choice plus a shield, four armor pieces and a pickaxe for dungeon crystals.
//! Hunters who claimed the kit before the pickaxe joined it may collect the
//! pickaxe once on its own. All of it is bound to the
//! hunter, unbreakable, and kept on death, because bound items never leave
//! their owner. The right is spent for good; losing the kit does not restore
//! it. Until the hunter first enters an instance, the chosen weapon may be
//! swapped for another starter weapon by handing the old one back.

use std::iter;

use serde::{Deserialize, Serialize};
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use crate::item::artifact::{self, ArtifactId, STARTER_OUTFIT, STARTER_WEAPONS};

/// A hunter's starter-kit record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarterKit {
    /// The weapon the kit was claimed with; `None` while the right is unused.
    #[serde(default)]
    pub weapon: Option<ArtifactId>,
    /// Set once the hunter entered an instance; the free swap ends there.
    #[serde(default)]
    pub swap_closed: bool,
    /// Set once the starter pickaxe was handed out, with the kit or alone.
    #[serde(default)]
    pub pickaxe: bool,
}

impl StarterKit {
    /// Returns whether the free kit can still be claimed.
    #[must_use]
    pub const fn claimable(self) -> bool {
        self.weapon.is_none()
    }

    /// Returns whether the claimed weapon may still be swapped for free.
    #[must_use]
    pub const fn swappable(self) -> bool {
        self.weapon.is_some() && !self.swap_closed
    }

    /// Returns whether a kit claimed before the pickaxe existed still owes it.
    #[must_use]
    pub const fn pickaxe_owed(self) -> bool {
        self.weapon.is_some() && !self.pickaxe
    }
}

/// Returns whether `id` is one of the selectable starter weapons.
#[must_use]
pub fn is_weapon(id: ArtifactId) -> bool {
    STARTER_WEAPONS.contains(&id)
}

/// Builds the full kit for `owner` with the chosen `weapon`.
#[must_use]
pub fn kit(owner: Uuid, weapon: ArtifactId) -> Vec<ItemStack> {
    iter::once(weapon)
        .chain(STARTER_OUTFIT)
        .map(|id| artifact::stack(id, Some(owner)))
        .collect()
}

/// Returns the short role description shown in the weapon choice.
#[must_use]
pub const fn weapon_role(id: ArtifactId) -> &'static str {
    match id {
        ArtifactId::HunterDagger => "Быстрые лёгкие удары, меньше урона за попадание.",
        ArtifactId::HunterAxe => "Медленные тяжёлые удары, дорогой промах.",
        _ => "Универсальный темп и урон.",
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::vanilla_components::UNBREAKABLE;
    use steel_registry::init_vanilla_registry;
    use uuid::Uuid;

    use super::{StarterKit, kit};
    use crate::gear;
    use crate::item::artifact::{self, ArtifactId, STARTER};
    use crate::item::binding;
    use crate::rank::HunterRank;

    #[test]
    fn the_kit_is_bound_unbreakable_and_one_weapon() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let items = kit(owner, ArtifactId::HunterAxe);
        assert_eq!(items.len(), 7);
        for stack in &items {
            assert_eq!(binding::bound_owner(stack), Some(owner));
            assert!(!binding::may_drop(stack));
            assert!(stack.has(UNBREAKABLE));
        }
        let weapons = items
            .iter()
            .filter(|stack| artifact::of(stack).is_some_and(super::is_weapon))
            .count();
        assert_eq!(weapons, 1);
    }

    #[test]
    fn starter_armor_gives_fifteen_armor_through_gear() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let items: Vec<_> = kit(owner, ArtifactId::HunterSword)
            .into_iter()
            .filter(|stack| gear::kind_of(stack).is_some())
            .collect();
        assert_eq!(items.len(), 4);
        let armor: f64 = gear::modifiers(&items, HunterRank::E, None, 0)
            .into_iter()
            .filter(|(attribute, _)| attribute.key.path == "armor")
            .map(|(_, modifier)| modifier.amount)
            .sum();
        assert!((armor - 15.0).abs() < 1e-9);
    }

    #[test]
    fn starter_pieces_round_trip_and_stay_out_of_reward_rolls() {
        init_vanilla_registry();
        for id in STARTER {
            assert_eq!(ArtifactId::from_name(id.name()), Some(id));
            assert!(!artifact::ALL.contains(&id));
            assert_eq!(id.artifact().abilities.len(), 0);
        }
    }

    #[test]
    fn the_right_is_spent_once_and_the_swap_closes() {
        let mut record = StarterKit::default();
        assert!(record.claimable());
        assert!(!record.swappable());
        record.weapon = Some(ArtifactId::HunterSword);
        assert!(!record.claimable());
        assert!(record.swappable());
        record.swap_closed = true;
        assert!(!record.swappable());
        assert!(!record.claimable());
        assert!(record.pickaxe_owed());
        record.pickaxe = true;
        assert!(!record.pickaxe_owed());
    }
}
