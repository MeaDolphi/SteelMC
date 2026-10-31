//! Skeleton-style ranged bow attack goal.
//!
//! Mirrors vanilla `RangedBowAttackGoal`.

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items::BOW;
use steel_utils::types::InteractionHand;

use super::selector::{Goal, GoalControls};
use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::{LivingEntity, PathfinderMob, SharedEntity};
use crate::inventory::equipment::EquipmentSlot;

const ATTACK_RADIUS_SQR: f64 = 225.0; // 15^2
const DRAW_TICKS: i32 = 20;

pub struct RangedBowAttackGoal {
    attack_radius_sq: f64,
    attack_interval_min: i32,
    attack_interval_max: i32,
    attack_time: i32,
    see_time: i32,
    strafing_clockwise: bool,
    strafing_backwards: bool,
    strafing_time: i32,
}

impl RangedBowAttackGoal {
    #[must_use]
    pub fn new(attack_radius: f64, attack_interval_min: i32, attack_interval_max: i32) -> Self {
        Self {
            attack_radius_sq: attack_radius * attack_radius,
            attack_interval_min,
            attack_interval_max,
            attack_time: -1,
            see_time: 0,
            strafing_clockwise: false,
            strafing_backwards: false,
            strafing_time: -1,
        }
    }

    fn is_holding_bow(mob: &dyn PathfinderMob) -> bool {
        let mut holding = false;
        mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
            holding = stack.is(&BOW);
        });
        holding
    }
}

impl Goal for RangedBowAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !Self::is_holding_bow(mob) {
            return false;
        }
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(living) = target.as_living_entity() else {
            return false;
        };
        LivingEntity::is_alive(living)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(true);
        self.attack_time = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(false);
        mob.living_base().stop_using_item();
        self.see_time = 0;
        self.attack_time = -1;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };
        let Some(ranged) = mob.as_ranged_attack_mob() else {
            return;
        };

        let target_pos = target.position();
        let distance_sq = mob.position().distance_squared(target_pos);
        let can_see = mob.has_line_of_sight_cached(target.as_ref());

        if self.see_time < 0 && !can_see {
            self.see_time = 0;
        }

        if can_see {
            self.see_time += 1;
        } else {
            self.see_time = 0;
        }

        // Look at target.
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(target_pos.x, target.get_eye_y(), target_pos.z),
            30.0,
            30.0,
        );

        if distance_sq <= self.attack_radius_sq && self.see_time >= 5 {
            mob.mob_base().navigation().lock().stop();
        } else {
            mob.move_to_pos(target_pos, 1.0);
        }

        if self.attack_time > 0 {
            self.attack_time -= 1;
        }

        if self.attack_time == 0 && can_see && distance_sq <= self.attack_radius_sq {
            // Start drawing the bow.
            let mut bow_stack = ItemStack::empty();
            mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| {
                bow_stack = stack.clone();
            });
            let duration = ITEM_BEHAVIORS
                .get_behavior(bow_stack.item())
                .get_use_duration(&bow_stack, mob);
            let _ =
                mob.living_base()
                    .start_using_item(InteractionHand::MainHand, &bow_stack, duration);
            self.attack_time = -1;
        } else if self.attack_time < 0
            && mob.get_ticks_using_item() >= DRAW_TICKS
            && can_see
            && distance_sq <= self.attack_radius_sq
        {
            ranged.perform_ranged_attack(entity, &target, distance_sq);
            mob.living_base().stop_using_item();
            self.attack_time =
                rand::random_range(self.attack_interval_min..=self.attack_interval_max);
        }
    }
}
