//! Consuming the themed items that only this crate understands.
//!
//! The client is unmodified vanilla, so a themed item is an ordinary stack
//! carrying a tag. Vanilla therefore has no behavior for it, and the module
//! that owns the tag is also the module that implements the right-click.

use std::sync::Arc;

use steel_core::behavior::InteractionResult;
use steel_core::entity::LivingEntity as _;
use steel_core::entity::MobEffectInstance;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_registry::item_stack::ItemStack;
use steel_utils::types::InteractionHand;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use super::{binding, chest, potion, shadow};
use crate::state::SoloLevelingState;

/// Handles a right-click on a themed item; `None` means "not a themed item".
#[must_use]
pub fn use_item(
    state: &SoloLevelingState,
    player: &Arc<Player>,
    stack: &ItemStack,
    hand: InteractionHand,
) -> Option<InteractionResult> {
    let owner = player.gameprofile.id();
    if binding::bound_to_other(stack, owner) {
        player.send_message(
            &TextComponent::plain("Этот предмет принадлежит другому охотнику.").color(Color::Red),
        );
        return Some(InteractionResult::Fail);
    }

    if let Some(buff) = potion::BuffPotion::of(stack) {
        player.add_mob_effect(MobEffectInstance::with_duration(
            buff.effect(),
            potion::buff_duration_ticks(),
            potion::buff_amplifier(),
        ));
        consume_one(player, hand);
        player.send_message(
            &TextComponent::plain(format!("Вы выпили {}.", buff.display_name()))
                .color(Color::Green),
        );
        return Some(InteractionResult::Success);
    }

    if let Some(entity) = shadow::entity(stack) {
        let Some(name) = state.arise_shadow_from_item(owner, &entity) else {
            player.send_message(
                &TextComponent::plain("Армия теней заполнена: повысьте уровень охотника.")
                    .color(Color::Red),
            );
            return Some(InteractionResult::Fail);
        };
        consume_one(player, hand);
        player.send_message(
            &TextComponent::plain(format!("{name} присоединился к вашей армии теней."))
                .color(Color::LightPurple),
        );
        return Some(InteractionResult::Success);
    }

    let kind = chest::kind_of(stack)?;
    Some(open_chest(player, stack, hand, kind))
}

/// Opens a chest item, rolling or reusing its already rolled reward.
fn open_chest(
    player: &Arc<Player>,
    stack: &ItemStack,
    hand: InteractionHand,
    kind: chest::ChestKind,
) -> InteractionResult {
    match kind {
        chest::ChestKind::Normal | chest::ChestKind::Sacred => {
            let reward = chest::roll(kind);
            consume_one(player, hand);
            grant(player, reward);
            player.send_message(
                &TextComponent::plain(format!(
                    "{}: {}.",
                    kind.display_name(),
                    reward.display_name()
                ))
                .color(kind.color()),
            );
            InteractionResult::Success
        }
        chest::ChestKind::Choice => {
            // The rolled options are stored on the chest itself, so closing the
            // menu and opening it again cannot reroll the choice.
            let options = if let Some(options) = chest::stored_options(stack) {
                options
            } else {
                let rolled = chest::roll_choice();
                if let Ok(mut rolled) = <[chest::Reward; 3]>::try_from(rolled) {
                    store_options(player, hand, &mut rolled);
                    rolled
                } else {
                    [chest::Reward::Shadow(0); 3]
                }
            };
            super::choice_menu::open(player, hand, &options);
            InteractionResult::Success
        }
    }
}

/// Writes the rolled options into the chest the player is holding.
fn store_options(player: &Player, hand: InteractionHand, options: &mut [chest::Reward; 3]) {
    let mut inventory = player.inventory.lock();
    let mut held = inventory.get_item_in_hand(hand).clone();
    if chest::kind_of(&held) != Some(chest::ChestKind::Choice) {
        return;
    }
    chest::store_options(&mut held, options);
    inventory.set_item_in_hand(hand, held);
}

/// Removes a single item from the hand that was used.
fn consume_one(player: &Player, hand: InteractionHand) {
    let mut inventory = player.inventory.lock();
    let mut held = inventory.get_item_in_hand(hand).clone();
    if held.is_empty() {
        return;
    }
    if held.count > 1 {
        held.count -= 1;
    } else {
        held = ItemStack::empty();
    }
    inventory.set_item_in_hand(hand, held);
}

/// Gives the player one reward, dropping it when the inventory is full.
fn grant(player: &Arc<Player>, reward: chest::Reward) {
    player.add_item_or_drop(reward.stack(None), Prediction::ServerOnly);
}
