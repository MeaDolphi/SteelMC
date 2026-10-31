//! Cartenon Temple: a rare key from gate clears opens a solo boss instance.
//!
//! The key drops once per day with a small chance after any gate clear. Using
//! it sends the hunter to a private temple arena; defeating the Smiling God
//! rewards a legendary artifact.

use std::sync::Arc;

use steel_core::player::Player;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use steel_utils::types::InteractionHand;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::daily;
use crate::dungeon::boss::{BossGear, BossKind, DungeonBoss, ZOMBIE_ATTACK_DAMAGE};
use crate::dungeon::is_instance_world;
use crate::item::{named, read_kind, record, with_lore, write_tag};
use crate::rank::HunterRank;
use crate::scenario::{self, ScenarioKind};
use crate::state::{GateClear, SoloLevelingState};

/// Percent chance to receive a temple key after a gate clear.
pub const TEMPLE_KEY_CHANCE_PERCENT: u32 = 5;
/// Kind stored in the record of a temple key.
const KIND: &str = "temple_key";

/// Creates one temple key.
#[must_use]
pub fn stack() -> ItemStack {
    let mut item = named(
        ItemStack::new(&vanilla_items::TRIAL_KEY),
        TextComponent::plain("Ключ Картенонского храма").color(Color::Gold),
    );
    item = with_lore(
        item,
        vec![
            TextComponent::plain("Открывает одиночный поход в Картенонский храм.")
                .color(Color::Gray),
            TextComponent::plain("ПКМ — войти в храм.").color(Color::Yellow),
        ],
    );
    let mut fields = record(KIND);
    fields.insert("bound", false);
    write_tag(&mut item, fields);
    item
}

/// Returns whether `stack` is a temple key.
#[must_use]
pub fn is_temple_key(stack: &ItemStack) -> bool {
    read_kind(stack, KIND).is_some()
}

impl SoloLevelingState {
    /// Rolls a temple-key drop after a gate clear.
    pub fn roll_temple_key(&self, uuid: Uuid, clear: &GateClear) {
        if clear.floor.is_some() {
            return;
        }
        let today = daily::today();
        let already_rolled = self
            .with_session_mut(uuid, |session| {
                if session.quests.temple_key_day == today {
                    return session.quests.temple_key_rolled;
                }
                session.quests.temple_key_day = today;
                session.quests.temple_key_rolled = false;
                false
            })
            .unwrap_or(true);
        if already_rolled {
            return;
        }
        if rand::random_range(0..100) >= TEMPLE_KEY_CHANCE_PERCENT {
            return;
        }
        self.with_session_mut(uuid, |session| session.quests.temple_key_rolled = true);
        self.queue_system_loot(uuid, stack());
        self.notify(uuid, "Редкая находка: Ключ Картенонского храма.");
    }

    /// Enters the Cartenon Temple using a key held by `player`.
    pub fn enter_temple(
        self: &Arc<Self>,
        player: &Arc<Player>,
        hand: InteractionHand,
    ) -> Result<(), String> {
        let uuid = player.gameprofile.id();
        if is_instance_world(&player.get_world()) {
            return Err("Ключ не работает внутри подземелья.".to_owned());
        }
        let rank = self.hunter_rank(uuid).unwrap_or(HunterRank::E);
        let inventory = player.inventory.lock();
        let held = inventory.get_item_in_hand(hand);
        if !is_temple_key(held) {
            return Err("В руке нет ключа Картенонского храма.".to_owned());
        }
        drop(inventory);
        let boss = smiling_god_boss(rank);
        let name = TextComponent::plain("Улыбающийся бог")
            .color(Color::DarkRed)
            .bold(true);
        scenario::start(
            Arc::clone(self),
            player,
            ScenarioKind::Temple,
            rank,
            boss,
            name,
        )?;
        // Consume the key only after the scenario accepted the run, so a
        // refused entry never burns it.
        let mut inventory = player.inventory.lock();
        let held = inventory.get_item_in_hand(hand).clone();
        if is_temple_key(&held) {
            if held.count > 1 {
                let mut remaining = held;
                remaining.count -= 1;
                inventory.set_item_in_hand(hand, remaining);
            } else {
                inventory.set_item_in_hand(hand, ItemStack::empty());
            }
        }
        Ok(())
    }
}

/// Returns the boss data for the Smiling God at `rank`.
///
/// The temple key can drop from a clear of any gate, so the boss sits only
/// slightly above a regular gate boss of the hunter's rank — unlike Igris,
/// who stays deliberately harsh regardless of rank.
#[must_use]
pub fn smiling_god_boss(rank: HunterRank) -> DungeonBoss {
    let gear = BossGear::for_rank(rank);
    // A notch above `boss_for_gate` of the same rank.
    let (health_multiplier, damage_multiplier, armor, armor_toughness) = match rank {
        HunterRank::E => (10.0, 2.4, 3.0, 1.0),
        HunterRank::D => (12.0, 2.8, 4.0, 2.0),
        HunterRank::C => (16.0, 3.2, 5.0, 3.0),
        HunterRank::B => (21.0, 3.8, 6.0, 4.0),
        HunterRank::A => (27.0, 4.4, 7.0, 6.0),
        HunterRank::S => (36.0, 5.2, 9.0, 8.0),
    };
    DungeonBoss {
        kind: BossKind::Warlord,
        scale: 1.8,
        health_multiplier,
        damage_multiplier,
        speed_multiplier: 0.9,
        power: ZOMBIE_ATTACK_DAMAGE * damage_multiplier,
        armor,
        armor_toughness,
        knockback_resistance: 0.7,
        attack_knockback: 1.2,
        gear,
        abilities: BossKind::Warlord.abilities(rank),
    }
}

#[cfg(test)]
mod tests {
    use super::smiling_god_boss;
    use crate::dungeon::boss::{BossKind, boss_for_gate};
    use crate::rank::HunterRank;

    #[test]
    fn smiling_god_scales_monotonically_with_rank() {
        let ranks = [
            HunterRank::E,
            HunterRank::D,
            HunterRank::C,
            HunterRank::B,
            HunterRank::A,
            HunterRank::S,
        ];
        let mut previous_health = 0.0;
        let mut previous_damage = 0.0;
        for rank in ranks {
            let boss = smiling_god_boss(rank);
            assert!(boss.health_multiplier > previous_health, "{rank:?}");
            assert!(boss.damage_multiplier > previous_damage, "{rank:?}");
            previous_health = boss.health_multiplier;
            previous_damage = boss.damage_multiplier;
            // A step above a regular gate boss of the same rank.
            let gate = boss_for_gate(BossKind::Warlord, rank, false);
            assert!(boss.health_multiplier > gate.health_multiplier, "{rank:?}");
            assert!(boss.damage_multiplier > gate.damage_multiplier, "{rank:?}");
        }
    }
}
