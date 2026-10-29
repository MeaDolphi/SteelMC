//! Runtime-only summoning, ownership, and progression for retained shadows.

use std::fmt;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::{Entity, LivingEntity, RemovalReason, SharedEntity, spawn_mob};
use steel_core::player::Player;
use steel_core::world::World;
use steel_registry::particle_type::ParticleData;
use steel_registry::{REGISTRY, RegistryExt, vanilla_attributes, vanilla_particle_types};
use steel_utils::BlockPos;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::session::HunterSession;
use crate::shadow::{
    ARISE_RANGE, ATTACK_PER_POINT, HEALTH_PER_POINT, SPEED_PER_POINT, Shadow, ShadowStat,
    army_limit,
};

struct ActiveShadow {
    owner: Uuid,
    shadow_id: Uuid,
    entity: SharedEntity,
}

/// Ephemeral data that must disappear on server restart.
#[derive(Default)]
pub struct ShadowRuntime {
    active: FxHashMap<Uuid, ActiveShadow>,
}

impl fmt::Debug for ShadowRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShadowRuntime")
            .field("active_count", &self.active.len())
            .finish()
    }
}

impl ShadowRuntime {
    /// Removes runtime entries whose entities were discarded outside a death hook.
    pub fn tick(&mut self) {
        self.active.retain(|_, active| !active.entity.is_removed());
    }

    /// Retains a nearby mob the player just killed, with an immediate soul effect.
    pub fn arise_from_player_kill(
        hunter: &Player,
        world: &World,
        entity: &dyn LivingEntity,
        session: &mut HunterSession,
    ) -> Option<Shadow> {
        if entity.as_mob().is_none() || hunter.get_world().key != world.key {
            return None;
        }
        if hunter.position().distance(entity.position()) > ARISE_RANGE {
            return None;
        }
        let shadow = session
            .shadows
            .arise(entity.entity_type().key.clone())
            .clone();
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SOUL),
            entity.position(),
            48,
            DVec3::new(0.4, 0.7, 0.4),
            0.03,
        );
        Some(shadow)
    }

    /// Summons one retained shadow next to its owner.
    pub fn summon(
        &mut self,
        hunter: &Player,
        session: &HunterSession,
        name: &str,
    ) -> Result<(), &'static str> {
        let Some(shadow) = session.shadows.named(name) else {
            return Err("Тень с таким именем не найдена.");
        };
        if shadow.is_recovering() {
            return Err("Эта тень ещё восстанавливается после гибели.");
        }
        if self
            .active
            .values()
            .any(|active| active.owner == hunter.gameprofile.id() && active.shadow_id == shadow.id)
        {
            return Err("Эта тень уже призвана.");
        }
        if self
            .active
            .values()
            .filter(|active| active.owner == hunter.gameprofile.id())
            .count()
            >= army_limit(session.stats.level)
        {
            return Err("Достигнут лимит призванной армии.");
        }
        self.spawn(hunter, shadow)
    }

    /// Summons roster shadows in order until the active-army limit is reached.
    pub fn summon_all(&mut self, hunter: &Player, session: &HunterSession) -> usize {
        let limit = army_limit(session.stats.level);
        let mut summoned = 0;
        for shadow in &session.shadows.shadows {
            if self
                .active
                .values()
                .filter(|active| active.owner == hunter.gameprofile.id())
                .count()
                >= limit
            {
                break;
            }
            if self.active.values().any(|active| {
                active.owner == hunter.gameprofile.id() && active.shadow_id == shadow.id
            }) {
                continue;
            }
            if shadow.is_recovering() {
                continue;
            }
            if self.spawn(hunter, shadow).is_ok() {
                summoned += 1;
            }
        }
        summoned
    }

    /// Removes all shadows currently summoned by one hunter.
    pub fn recall(&mut self, owner: Uuid) -> usize {
        let ids: Vec<_> = self
            .active
            .iter()
            .filter_map(|(id, active)| (active.owner == owner).then_some(*id))
            .collect();
        for id in &ids {
            if let Some(active) = self.active.remove(id) {
                active.entity.set_removed(RemovalReason::Discarded);
            }
        }
        ids.len()
    }

    /// Removes a dead body and returns the persistent shadow that must rebuild.
    pub fn observe_death(&mut self, entity_uuid: Uuid) -> Option<(Uuid, Uuid, u16)> {
        let active = self.active.remove(&entity_uuid)?;
        let maximum_health = active
            .entity
            .as_living_entity()
            .map_or(1.0, LivingEntity::get_max_health)
            .ceil()
            .clamp(1.0, f32::from(u16::MAX)) as u16;
        Some((active.owner, active.shadow_id, maximum_health))
    }

    /// Identifies the owner, persistent shadow, and reward of a shadow kill.
    pub fn kill_reward(
        &mut self,
        killer_uuid: Uuid,
        victim: &dyn LivingEntity,
        world: &World,
    ) -> Option<(Uuid, Uuid, u64)> {
        let active = self.active.get(&killer_uuid)?;
        let reward = victim
            .experience_reward(world, Some(active.entity.as_ref()))
            .max(0) as u64;
        Some((active.owner, active.shadow_id, reward))
    }

    /// Allocates one point and refreshes the matching active body.
    pub fn upgrade(
        &mut self,
        owner: Uuid,
        session: &mut HunterSession,
        name: &str,
        stat: ShadowStat,
    ) -> Result<Shadow, &'static str> {
        let Some(shadow) = session.shadows.named_mut(name) else {
            return Err("Тень с таким именем не найдена.");
        };
        if !shadow.allocate_point(stat) {
            return Err("У этой тени нет нераспределённых очков.");
        }
        let shadow = shadow.clone();
        for active in self.active.values() {
            if active.owner == owner && active.shadow_id == shadow.id {
                apply_upgrade(active.entity.as_ref(), stat);
            }
        }
        Ok(shadow)
    }

    fn spawn(&mut self, hunter: &Player, shadow: &Shadow) -> Result<(), &'static str> {
        let Some(entity_type) = REGISTRY.entity_types.by_key(&shadow.entity_type) else {
            return Err("Тип этой тени больше не зарегистрирован.");
        };
        let world = hunter.get_world();
        let entity = spawn_mob(&world, entity_type, BlockPos::from(hunter.position()))
            .map_err(|_| "Не удалось призвать тень в этой точке.")?;
        if entity.as_mob().is_none() {
            entity.set_removed(RemovalReason::Discarded);
            return Err("Тип сохранённой тени не является мобом.");
        }
        entity.set_runtime_allied_owner(Some(hunter.gameprofile.id()));
        entity.set_custom_name(Some(
            TextComponent::plain(format!("[Тень] {}", shadow.name)).color(Color::DarkGray),
        ));
        entity.set_custom_name_visible(true);
        apply_stats(entity.as_ref(), shadow);
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SMOKE),
            entity.position(),
            24,
            DVec3::new(0.35, 0.5, 0.35),
            0.02,
        );
        self.active.insert(
            entity.uuid(),
            ActiveShadow {
                owner: hunter.gameprofile.id(),
                shadow_id: shadow.id,
                entity,
            },
        );
        Ok(())
    }
}

fn apply_stats(entity: &dyn Entity, shadow: &Shadow) {
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    let attributes = living.attributes().lock();
    let health = attributes.required_value(vanilla_attributes::MAX_HEALTH);
    let attack = attributes.required_value(vanilla_attributes::ATTACK_DAMAGE);
    let speed = attributes.required_value(vanilla_attributes::MOVEMENT_SPEED);
    drop(attributes);
    let max_health = health + f64::from(shadow.health) * HEALTH_PER_POINT;
    living.set_base_attribute_value(vanilla_attributes::MAX_HEALTH, max_health);
    living.set_base_attribute_value(
        vanilla_attributes::ATTACK_DAMAGE,
        attack + f64::from(shadow.attack) * ATTACK_PER_POINT,
    );
    living.set_base_attribute_value(
        vanilla_attributes::MOVEMENT_SPEED,
        speed + f64::from(shadow.speed) * SPEED_PER_POINT,
    );
    living.set_health(max_health as f32);
}

fn apply_upgrade(entity: &dyn Entity, stat: ShadowStat) {
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    let attribute = match stat {
        ShadowStat::Health => vanilla_attributes::MAX_HEALTH,
        ShadowStat::Attack => vanilla_attributes::ATTACK_DAMAGE,
        ShadowStat::Speed => vanilla_attributes::MOVEMENT_SPEED,
    };
    let increment = match stat {
        ShadowStat::Health => HEALTH_PER_POINT,
        ShadowStat::Attack => ATTACK_PER_POINT,
        ShadowStat::Speed => SPEED_PER_POINT,
    };
    let current = living.attributes().lock().required_value(attribute);
    living.set_base_attribute_value(attribute, current + increment);
    if stat == ShadowStat::Health {
        living.set_health(living.get_max_health());
    }
}
