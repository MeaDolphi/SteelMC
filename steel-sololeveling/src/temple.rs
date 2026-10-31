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
        let mut consumed = held.clone();
        if consumed.count > 1 {
            consumed.count -= 1;
        } else {
            consumed = ItemStack::empty();
        }
        drop(inventory);
        player.inventory.lock().set_item_in_hand(hand, consumed);

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
        )
    }
}

/// Returns the boss data for the Smiling God at `rank`.
#[must_use]
pub fn smiling_god_boss(rank: HunterRank) -> DungeonBoss {
    let gear = BossGear::for_rank(rank);
    let damage_multiplier = 5.0;
    DungeonBoss {
        kind: BossKind::Warlord,
        scale: 1.8,
        health_multiplier: 35.0,
        damage_multiplier,
        speed_multiplier: 0.9,
        power: ZOMBIE_ATTACK_DAMAGE * damage_multiplier,
        armor: 8.0,
        armor_toughness: 6.0,
        knockback_resistance: 0.7,
        attack_knockback: 1.2,
        gear,
        abilities: BossKind::Warlord.abilities(rank),
    }
}
