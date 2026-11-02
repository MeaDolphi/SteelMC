//! Lifecycle and motion of short-lived spell models.

use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::entities::ItemDisplayEntity;
use steel_core::entity::{Entity as _, RemovalReason};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::entity_data::Vector3f;
use steel_utils::{Downcast as _, Identifier};
use uuid::Uuid;

use super::display::{self, SpellModel};

const MAX_EFFECTS: usize = 256;
const MAX_EFFECTS_PER_OWNER: usize = 24;

/// How a display moves between its creation and expiration.
#[derive(Debug, Clone)]
pub(crate) enum Motion {
    Static(DVec3),
    Linear {
        from: DVec3,
        to: DVec3,
        started: u64,
        duration: u64,
    },
    Follow {
        target: Uuid,
        offset: DVec3,
    },
}

/// One model with a hard expiration and optional ground-rune spin.
#[derive(Debug, Clone)]
struct VisualEffect {
    owner: Uuid,
    world: Identifier,
    entity: i32,
    motion: Motion,
    scale: f32,
    horizontal: bool,
    spin: f32,
    started: u64,
    until: u64,
}

impl VisualEffect {
    fn position(&self, server: &Server, now: u64) -> Option<DVec3> {
        match self.motion {
            Motion::Static(position) => Some(position),
            Motion::Linear {
                from,
                to,
                started,
                duration,
            } => {
                let progress = progress(started, duration, now);
                Some(from.lerp(to, progress))
            }
            Motion::Follow { target, offset } => server
                .get_players()
                .into_iter()
                .find(|player| player.gameprofile.id() == target)
                .filter(|player| player.get_world().key == self.world)
                .map(|player| player.position() + offset),
        }
    }
}

/// Time-based progress shared by every linear visual.
#[must_use]
pub(crate) fn progress(started: u64, duration: u64, now: u64) -> f64 {
    if duration == 0 {
        return 1.0;
    }
    (now.saturating_sub(started) as f64 / duration as f64).clamp(0.0, 1.0)
}

/// All live spell models, bounded to avoid runaway entity creation.
#[derive(Debug, Default)]
pub(crate) struct VisualRuntime {
    effects: Vec<VisualEffect>,
}

impl VisualRuntime {
    #[expect(
        clippy::too_many_arguments,
        reason = "a display needs its identity, motion, lifetime and rotation"
    )]
    pub(crate) fn spawn(
        &mut self,
        world: &Arc<World>,
        owner: Uuid,
        model: SpellModel,
        motion: Motion,
        scale: f32,
        horizontal: bool,
        spin: f32,
        started: u64,
        until: u64,
    ) -> Option<i32> {
        if self.effects.len() >= MAX_EFFECTS
            || self
                .effects
                .iter()
                .filter(|effect| effect.owner == owner)
                .count()
                >= MAX_EFFECTS_PER_OWNER
        {
            return None;
        }
        let position = match motion {
            Motion::Static(position) => position,
            Motion::Linear { from, .. } => from,
            Motion::Follow { target, offset } => world
                .players
                .get_by_uuid(&target)
                .map(|player| player.position() + offset)?,
        };
        let entity = display::spawn(world, model, position, scale, horizontal)?;
        self.effects.push(VisualEffect {
            owner,
            world: world.key.clone(),
            entity,
            motion,
            scale,
            horizontal,
            spin,
            started,
            until,
        });
        Some(entity)
    }

    /// Places a horizontal rune with a fixed duration.
    #[expect(
        clippy::too_many_arguments,
        reason = "a rune needs its owner, placement, lifetime and spin"
    )]
    pub(crate) fn rune(
        &mut self,
        world: &Arc<World>,
        owner: Uuid,
        model: SpellModel,
        center: DVec3,
        radius: f32,
        duration: u64,
        now: u64,
        spin: f32,
    ) -> Option<i32> {
        self.spawn(
            world,
            owner,
            model,
            Motion::Static(center),
            radius * 2.0,
            true,
            spin,
            now,
            now.saturating_add(duration),
        )
    }

    /// Moves and rotates every live model, removing expired or orphaned ones.
    pub(crate) fn tick(&mut self, server: &Server, now: u64) {
        self.effects.retain(|effect| {
            let keep = effect.until > now
                && server
                    .get_players()
                    .iter()
                    .any(|player| player.gameprofile.id() == effect.owner);
            let Some(world) = server.worlds.get(&effect.world) else {
                return false;
            };
            let Some(entity) = world.get_entity_by_id(effect.entity) else {
                return false;
            };
            if !keep {
                entity.set_removed(RemovalReason::Discarded);
                return false;
            }
            let Some(position) = effect.position(server, now) else {
                entity.set_removed(RemovalReason::Discarded);
                return false;
            };
            if entity.try_set_position(position).is_err() {
                entity.set_removed(RemovalReason::Discarded);
                return false;
            }
            if effect.spin != 0.0
                && let Some(display) = entity.downcast_ref::<ItemDisplayEntity>()
            {
                let elapsed = now.saturating_sub(effect.started) as f32 / 1_000.0;
                display.set_transformation(
                    Vector3f::ZERO,
                    Vector3f::new(effect.scale, effect.scale, effect.scale),
                    display::rotation(effect.horizontal, effect.spin * elapsed),
                );
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::progress;

    #[test]
    fn linear_motion_is_clamped_to_its_endpoints() {
        assert_eq!(progress(100, 50, 50), 0.0);
        assert_eq!(progress(100, 50, 125), 0.5);
        assert_eq!(progress(100, 50, 200), 1.0);
        assert_eq!(progress(100, 0, 100), 1.0);
    }
}
