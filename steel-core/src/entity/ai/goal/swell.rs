//! Creeper swell-and-explode goal.
//!
//! Mirrors vanilla `SwellGoal`.

use super::selector::{Goal, GoalControls};
use crate::entity::entities::mobs::hostile::CreeperEntity;
use crate::entity::{LivingEntity, PathfinderMob, SharedEntity};
use steel_utils::Downcast as _;

const MAX_SWELL: i32 = 30;
/// Vanilla `SwellGoal.canUse` starts swelling only when the target is closer
/// than 3 blocks; line of sight is checked per-tick, not at start.
const SWELL_START_DISTANCE_SQR: f64 = 9.0;
/// Vanilla keeps the fuse lit while the target stays within 7 blocks.
const SWELL_KEEP_DISTANCE_SQR: f64 = 49.0;

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

    fn creeper(mob: &dyn PathfinderMob) -> Option<&CreeperEntity> {
        mob.as_entity_event_source().downcast_ref::<CreeperEntity>()
    }
}

impl Goal for SwellGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(creeper) = Self::creeper(mob) else {
            return false;
        };
        // Vanilla also keeps the goal while the fuse is already burning, so an
        // ignited creeper cannot be interrupted by target selection churn.
        if creeper.swell_dir() > 0 || creeper.is_ignited() {
            self.target = mob.target();
            return true;
        }

        let Some(target) = mob.target() else {
            return false;
        };
        let Some(living) = target.as_living_entity() else {
            return false;
        };
        if LivingEntity::is_dead_or_dying(living)
            || mob.position().distance_squared(target.position()) >= SWELL_START_DISTANCE_SQR
        {
            return false;
        }

        self.target = Some(target);
        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        // Vanilla `SwellGoal.start` stops navigation and captures the target.
        mob.mob_base().navigation().lock().stop();
        self.target = mob.target();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(creeper) = Self::creeper(mob) else {
            return;
        };

        let target_alive = self.target.as_ref().is_some_and(|target| {
            target
                .as_living_entity()
                .is_some_and(LivingEntity::is_alive)
        });
        let in_keep_range = self.target.as_ref().is_some_and(|target| {
            mob.position().distance_squared(target.position()) <= SWELL_KEEP_DISTANCE_SQR
                && mob.has_line_of_sight_cached(target.as_ref())
        });

        // Vanilla `Creeper.tick` forces the fuse while the creeper is ignited.
        let inflating = creeper.is_ignited() || (target_alive && in_keep_range);
        let swell = creeper.swell_dir();
        let next_swell = if inflating {
            swell + 1
        } else {
            (swell - 1).max(-1)
        };

        if next_swell >= MAX_SWELL {
            creeper.explode(entity);
            return;
        }

        if next_swell != swell {
            creeper.set_swell_dir(next_swell);
        }
    }
}
