//! Creeper swell-and-explode goal.
//!
//! Mirrors vanilla `SwellGoal`.

use super::selector::{Goal, GoalControls};
use crate::entity::entities::mobs::hostile::CreeperEntity;
use crate::entity::{LivingEntity, PathfinderMob, SharedEntity};
use steel_utils::Downcast as _;

const MAX_SWELL: i32 = 30;
const EXPLOSION_RADIUS: f64 = 3.0;
const EXPLOSION_DAMAGE: f32 = 43.0;

/// Creeper-specific AI goal that swells up and explodes near a target.
pub struct SwellGoal {
    target: Option<SharedEntity>,
}

impl SwellGoal {
    /// Creates a new swell goal.
    #[must_use]
    pub const fn new() -> Self {
        Self { target: None }
    }

    fn is_target_valid(mob: &dyn PathfinderMob, target: &SharedEntity) -> bool {
        let Some(living) = target.as_living_entity() else {
            return false;
        };
        LivingEntity::is_alive(living) && mob.has_line_of_sight_cached(target.as_ref())
    }

    fn creeper(mob: &dyn PathfinderMob) -> Option<&CreeperEntity> {
        mob.as_entity_event_source().downcast_ref::<CreeperEntity>()
    }
}

impl Goal for SwellGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        if !Self::is_target_valid(mob, &target) {
            return false;
        }
        if Self::creeper(mob).is_none() {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        Self::creeper(mob).is_some() && Self::is_target_valid(mob, target)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(creeper) = Self::creeper(mob) {
            creeper.set_swell_dir(-1);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(creeper) = Self::creeper(mob) {
            creeper.set_swell_dir(-1);
        }
        self.target = None;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(creeper) = Self::creeper(mob) else {
            return;
        };
        let Some(target) = &self.target else {
            return;
        };

        let distance_sqr = mob.position().distance_squared(target.position());
        let swell = creeper.swell_dir();
        let mut next_swell = swell;

        if distance_sqr <= EXPLOSION_RADIUS * EXPLOSION_RADIUS
            && mob.has_line_of_sight_cached(target.as_ref())
        {
            next_swell += 1;
            if next_swell >= MAX_SWELL {
                creeper.explode(entity);
                return;
            }
        } else {
            next_swell -= 1;
        }

        next_swell = next_swell.clamp(-1, MAX_SWELL);
        if next_swell != swell {
            creeper.set_swell_dir(next_swell);
        }
    }
}
