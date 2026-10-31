//! Bow item behavior (`BowItem`).
//!
//! Mirrors vanilla `BowItem.use`, `releaseUsing`, and projectile spawning.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events::ENTITY_ARROW_SHOOT;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{vanilla_enchantments, vanilla_items};
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use crate::behavior::InteractionResult;
use crate::behavior::context::UseItemContext;
use crate::behavior::item::{ItemBehavior, ItemUseAnimation};
use crate::entity::Entity;
use crate::entity::LivingEntity;
use crate::entity::entities::ArrowEntity;
use crate::entity::projectile::AbstractArrow;
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
use crate::world::World;

/// Maximum draw ticks used to compute bow power.
const MAX_DRAW_TICKS: i32 = 20;

/// Sound constants for a player bow shot.
const SHOOT_SOUND_SOURCE: SoundSource = SoundSource::Players;
const SHOOT_VOLUME: f32 = 0.5;
const SHOOT_PITCH_MIN: f32 = 0.83;
const SHOOT_PITCH_MAX: f32 = 0.87;

/// Behavior for the bow item.
#[item_behavior(class = "BowItem")]
pub struct BowItem;

impl ItemBehavior for BowItem {
    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Bow
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        72000
    }

    fn use_on_release(&self, _stack: &ItemStack) -> bool {
        true
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.start_using_item(context.hand);
        InteractionResult::Consume
    }

    fn release_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        let ticks_used = 72000 - time_left;
        let power_factor = bow_power(ticks_used);
        if power_factor <= 0.1 {
            return false;
        }

        let mut ammo = ItemStack::empty();
        let mut found = false;
        if let Some(player) = user.as_player() {
            let has_infinite = player.has_infinite_materials();
            let mut inventory = player.inventory.lock();
            if let Some(arrow) = find_and_consume_arrow(
                world,
                player,
                &mut inventory,
                InteractionHand::MainHand,
                has_infinite,
                stack,
            ) {
                ammo = arrow;
                found = true;
            }
        }
        if !found {
            return false;
        }

        let speed = (power_factor * 3.0) as f32;
        let spread = 1.0;
        let crit = (power_factor - 1.0).abs() < f64::EPSILON;

        let Some(arrow) = ArrowEntity::spawn(world, user, Some(stack), &ammo, speed, spread) else {
            return false;
        };

        if let Some(arrow) = arrow.as_ref().downcast_ref::<ArrowEntity>() {
            arrow.set_crit(crit);
            let punch = stack.get_enchantment_level(&vanilla_enchantments::PUNCH.key);
            arrow.set_knockback(punch);
            if stack.get_enchantment_level(&vanilla_enchantments::FLAME.key) > 0 {
                arrow.set_remaining_fire_ticks(100);
            }
        }

        let pitch = rand::random_range(SHOOT_PITCH_MIN..SHOOT_PITCH_MAX);
        world.play_sound_at(
            &ENTITY_ARROW_SHOOT,
            SHOOT_SOUND_SOURCE,
            user.position(),
            SHOOT_VOLUME,
            pitch,
            None,
        );

        if let Some(player) = user.as_player() {
            player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        }
        true
    }
}

/// Computes vanilla bow power from ticks the bow was drawn.
fn bow_power(ticks_used: i32) -> f64 {
    let draw = (f64::from(ticks_used) / f64::from(MAX_DRAW_TICKS)).clamp(0.0, 1.0);
    (draw * draw + 2.0 * draw) / 3.0
}

/// Finds an arrow in the player's inventory, optionally consuming it.
fn find_and_consume_arrow(
    world: &Arc<World>,
    player: &Player,
    inventory: &mut PlayerInventory,
    hand: InteractionHand,
    has_infinite_materials: bool,
    bow: &ItemStack,
) -> Option<ItemStack> {
    let hand_stack = inventory.get_item_in_hand(hand);
    if !hand_stack.is_empty() && is_arrow(hand_stack) {
        let arrow = hand_stack.copy_with_count(1);
        if !should_keep_arrow(world, player, bow, &arrow, has_infinite_materials) {
            inventory.shrink_item_in_hand(hand, 1);
        }
        return Some(arrow);
    }

    let slot = inventory
        .get_items()
        .iter()
        .position(|stack| !stack.is_empty() && is_arrow(stack))?;
    let arrow = inventory.get_items()[slot].copy_with_count(1);
    if !should_keep_arrow(world, player, bow, &arrow, has_infinite_materials) {
        inventory.with_item_mut(slot, ItemStack::shrink_one);
    }
    Some(arrow)
}

fn is_arrow(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::ARROW)
        || stack.is(&vanilla_items::SPECTRAL_ARROW)
        || stack.is(&vanilla_items::TIPPED_ARROW)
}

/// Vanilla ammo-keeping (`ProjectileWeaponItem`): creative mode or a bow with
/// INFINITY firing a plain arrow. Gameplay layers may keep further ammunition
/// through [`crate::gameplay_hooks::GameplayHooks::keep_projectile_ammo`], which only runs when
/// vanilla would have consumed the arrow.
fn should_keep_arrow(
    world: &World,
    player: &Player,
    bow: &ItemStack,
    arrow: &ItemStack,
    has_infinite_materials: bool,
) -> bool {
    if has_infinite_materials {
        return true;
    }
    if bow.get_enchantment_level(&vanilla_enchantments::INFINITY.key) > 0
        && arrow.is(&vanilla_items::ARROW)
    {
        return true;
    }
    world
        .gameplay_hooks
        .keep_projectile_ammo(player, bow, arrow)
}
