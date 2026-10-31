//! Crossbow item behavior (`CrossbowItem`).
//!
//! Mirrors vanilla crossbow charging and shooting.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::ChargedProjectiles;
use steel_registry::data_components::vanilla_components::CHARGED_PROJECTILES;
use steel_registry::item_stack::ItemStack;
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::sound_events::{
    ITEM_CROSSBOW_LOADING_END, ITEM_CROSSBOW_LOADING_START, ITEM_CROSSBOW_SHOOT,
};
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{vanilla_enchantments, vanilla_items};
use steel_utils::types::InteractionHand;

use crate::behavior::InteractionResult;
use crate::behavior::context::UseItemContext;
use crate::behavior::item::{ItemBehavior, ItemUseAnimation};
use crate::entity::Entity;
use crate::entity::LivingEntity;
use crate::entity::entities::ArrowEntity;
use crate::player::player_inventory::PlayerInventory;
use crate::world::World;

/// Base charge duration in ticks (1.25 seconds at 20 tps).
const BASE_CHARGE_TICKS: i32 = 25;
/// Reduction per Quick Charge level, in ticks.
const QUICK_CHARGE_REDUCTION_PER_LEVEL: i32 = 5;
/// Minimum charge duration.
const MIN_CHARGE_TICKS: i32 = 5;

/// Sound constants.
const SHOOT_SOUND_SOURCE: SoundSource = SoundSource::Players;
const SHOOT_VOLUME: f32 = 0.5;
const SHOOT_PITCH: f32 = 1.0;

/// Behavior for the crossbow item.
#[item_behavior(class = "CrossbowItem")]
pub struct CrossbowItem;

impl ItemBehavior for CrossbowItem {
    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Crossbow
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        72000
    }

    fn use_on_release(&self, _stack: &ItemStack) -> bool {
        true
    }

    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let is_charged = context.inv.with_item(|item| item.has(CHARGED_PROJECTILES));
        if is_charged {
            Self::shoot(context);
            return InteractionResult::Success;
        }

        context.player.start_using_item(context.hand);
        context.world.play_sound_at(
            &ITEM_CROSSBOW_LOADING_START,
            SHOOT_SOUND_SOURCE,
            context.player.position(),
            SHOOT_VOLUME,
            SHOOT_PITCH,
            None,
        );
        InteractionResult::Consume
    }

    fn release_using(
        &self,
        stack: &mut ItemStack,
        world: &Arc<World>,
        user: &dyn LivingEntity,
        time_left: i32,
    ) -> bool {
        if stack.has(CHARGED_PROJECTILES) {
            return Self::shoot_loaded(stack, world, user);
        }

        let ticks_used = 72000 - time_left;
        if ticks_used < charge_ticks(stack) {
            return false;
        }

        if let Some(player) = user.as_player() {
            let mut inventory = player.inventory.lock();
            if let Some(ammo) = find_and_consume_arrow(
                &mut inventory,
                InteractionHand::MainHand,
                player.has_infinite_materials(),
            ) {
                let template = ItemStackTemplate::from_stack(&ammo)
                    .unwrap_or(ItemStackTemplate::new(ammo.item()));
                let charged = ChargedProjectiles::new(vec![template]).unwrap_or_else(|_| {
                    ChargedProjectiles::new(vec![ItemStackTemplate::new(&vanilla_items::ARROW)])
                        .expect("single arrow fits charged projectiles")
                });
                stack.set(CHARGED_PROJECTILES, charged);
                world.play_sound_at(
                    &ITEM_CROSSBOW_LOADING_END,
                    SHOOT_SOUND_SOURCE,
                    user.position(),
                    SHOOT_VOLUME,
                    SHOOT_PITCH,
                    None,
                );
            }
        }

        if stack.has(CHARGED_PROJECTILES) {
            return Self::shoot_loaded(stack, world, user);
        }
        false
    }
}

impl CrossbowItem {
    fn shoot(context: &mut UseItemContext) -> InteractionResult {
        let mut stack = context.inv.with_item(|item| item.clone());
        if Self::shoot_loaded(&mut stack, context.world, context.player.as_ref()) {
            context.inv.with_item(|item| *item = stack);
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }

    fn shoot_loaded(stack: &mut ItemStack, world: &Arc<World>, user: &dyn Entity) -> bool {
        let Some(charged) = stack.get(CHARGED_PROJECTILES).cloned() else {
            return false;
        };
        let Some(template) = charged.items().first() else {
            return false;
        };

        let ammo = template.create();
        let arrow = ArrowEntity::spawn(world, user, Some(stack), &ammo, 3.15, 1.0);
        if arrow.is_none() {
            return false;
        }

        stack.remove(CHARGED_PROJECTILES);
        world.play_sound_at(
            &ITEM_CROSSBOW_SHOOT,
            SHOOT_SOUND_SOURCE,
            user.position(),
            SHOOT_VOLUME,
            SHOOT_PITCH,
            None,
        );

        if let Some(player) = user.as_player() {
            player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        }
        true
    }
}

/// Computes crossbow charge time from the Quick Charge enchantment.
fn charge_ticks(stack: &ItemStack) -> i32 {
    let quick_charge = stack.get_enchantment_level(&vanilla_enchantments::QUICK_CHARGE.key);
    (BASE_CHARGE_TICKS - quick_charge * QUICK_CHARGE_REDUCTION_PER_LEVEL).max(MIN_CHARGE_TICKS)
}

/// Finds an arrow in the player's inventory, optionally consuming it.
fn find_and_consume_arrow(
    inventory: &mut PlayerInventory,
    hand: InteractionHand,
    has_infinite_materials: bool,
) -> Option<ItemStack> {
    let hand_stack = inventory.get_item_in_hand(hand);
    if !hand_stack.is_empty() && is_arrow(hand_stack) {
        let arrow = hand_stack.copy_with_count(1);
        if !has_infinite_materials {
            inventory.shrink_item_in_hand(hand, 1);
        }
        return Some(arrow);
    }

    let slot = inventory
        .get_items()
        .iter()
        .position(|stack| !stack.is_empty() && is_arrow(stack))?;
    let arrow = inventory.get_items()[slot].copy_with_count(1);
    if !has_infinite_materials {
        inventory.with_item_mut(slot, ItemStack::shrink_one);
    }
    Some(arrow)
}

fn is_arrow(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::ARROW)
        || stack.is(&vanilla_items::SPECTRAL_ARROW)
        || stack.is(&vanilla_items::TIPPED_ARROW)
}
