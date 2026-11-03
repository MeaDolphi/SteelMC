//! Combat-skill upgrades and loadout resets.

use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use steel_core::entity::damage::DamageSource;
use steel_core::entity::entities::ArrowEntity;
use steel_core::entity::{AbstractArrow as _, Entity as _, LivingEntity as _, SharedEntity};
use steel_core::player::Player;
use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_types, vanilla_items, vanilla_particle_types,
};
use steel_utils::{BlockPos, Downcast as _, WorldAabb};
use text_components::Modifier as _;
use uuid::Uuid;

use crate::combat_skill::{CombatSkill, CombatSkillBook, Flurry, UpgradeError};
use crate::dungeon::is_instance_world;
use crate::spell_fx as fx;
use crate::stats::HunterStats;

use super::SoloLevelingState;

impl SoloLevelingState {
    pub fn upgrade_class_skill(&self, uuid: Uuid) -> Result<u8, UpgradeError> {
        let result = self.with_session_mut(uuid, |session| {
            session.combat_skills.upgrade_class(
                session.class.class.ok_or(UpgradeError::ClassLocked)?,
                session.stats.level,
                session.stats.rank(),
            )
        });
        match result {
            Some(Ok(rank)) => {
                self.save_after_skill_change(uuid);
                Ok(rank)
            }
            Some(Err(error)) => Err(error),
            None => Err(UpgradeError::ClassLocked),
        }
    }

    pub fn upgrade_general_skill(
        &self,
        uuid: Uuid,
        skill: CombatSkill,
    ) -> Result<u8, UpgradeError> {
        let result = self.with_session_mut(uuid, |session| {
            session
                .combat_skills
                .upgrade_general(skill, session.stats.level, session.stats.rank())
        });
        match result {
            Some(Ok(rank)) => {
                self.save_after_skill_change(uuid);
                Ok(rank)
            }
            Some(Err(error)) => Err(error),
            None => Err(UpgradeError::RankLocked),
        }
    }

    pub fn select_general_skill(&self, uuid: Uuid, skill: CombatSkill) -> Result<(), UpgradeError> {
        let result =
            self.with_session_mut(uuid, |session| session.combat_skills.select_general(skill));
        match result {
            Some(Ok(())) => {
                self.save_after_skill_change(uuid);
                Ok(())
            }
            Some(Err(error)) => Err(error),
            None => Err(UpgradeError::NotGeneral),
        }
    }

    /// Upgrades the persistent innate skill without changing the general selection.
    pub fn upgrade_innate_skill(&self, uuid: Uuid) -> Result<u8, UpgradeError> {
        let result = self.with_session_mut(uuid, |session| {
            session.combat_skills.upgrade_innate(session.stats.level)
        });
        match result {
            Some(Ok(rank)) => {
                self.save_after_skill_change(uuid);
                Ok(rank)
            }
            Some(Err(error)) => Err(error),
            None => Err(UpgradeError::NotGeneral),
        }
    }

    fn save_after_skill_change(&self, uuid: Uuid) {
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist combat skills of {uuid}: {error}");
        }
    }

    pub fn reset_combat_skills(&self, player: &Player) -> Result<(), &'static str> {
        if is_instance_world(&player.get_world()) {
            return Err("Сброс навыков доступен только в городе.");
        }
        let uuid = player.gameprofile.id();
        let Some(()) = self.with_session_mut(uuid, |session| {
            if let Some(class) = session.class.class {
                session
                    .combat_skills
                    .unlock_class(class, session.stats.rank());
            }
            session.combat_skills.reset();
        }) else {
            return Err("Сессия охотника не найдена.");
        };
        self.save_after_skill_change(uuid);
        Ok(())
    }

    pub fn reset_hunter_stats(&self, player: &Player) -> Result<HunterStats, &'static str> {
        if is_instance_world(&player.get_world()) {
            return Err("Сброс характеристик доступен только в городе.");
        }
        let uuid = player.gameprofile.id();
        let Some(stats) = self.with_session_mut(uuid, |session| {
            session.stats.reset_allocations();
            session.stats
        }) else {
            return Err("Сессия охотника не найдена.");
        };
        stats.apply_to(&mut player.attributes().lock());
        player.set_health(player.get_health().min(player.get_max_health()));
        self.save_after_skill_change(uuid);
        Ok(stats)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUseError {
    NoClass,
    NoSkill,
    Cooldown(u64),
    NoMana,
    NoTarget,
    RankLocked,
}

impl SkillUseError {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoClass => "Классовый навык открывается после выбора класса с ранга D.",
            Self::NoSkill => "Сначала выберите и изучите навык в меню «Навыки».",
            Self::Cooldown(_) => "Навык ещё перезаряжается.",
            Self::NoMana => "Недостаточно маны: нужно 10.",
            Self::NoTarget => "Нет подходящей цели или безопасной точки для применения.",
            Self::RankLocked => "Ранг охотника пока не позволяет применить этот навык.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatSkillSlot {
    Class,
    General,
    Innate,
}

impl SoloLevelingState {
    /// Resolves the shared player entity required by effects from the drop-key hook.
    pub fn use_drop_key_combat_skill(
        &self,
        player: &Player,
        slot: CombatSkillSlot,
    ) -> Result<(), SkillUseError> {
        let uuid = player.gameprofile.id();
        let Some(player) = player.get_world().players.get_by_uuid(&uuid) else {
            return Err(SkillUseError::NoTarget);
        };
        self.use_combat_skill(&player, slot)
    }

    /// Applies one of the three independent active skill slots.
    pub fn use_combat_skill(
        &self,
        player: &Arc<Player>,
        slot: CombatSkillSlot,
    ) -> Result<(), SkillUseError> {
        let uuid = player.gameprofile.id();
        let now = CombatSkillBook::now();
        let (skill, rank, stats) = self
            .with_session_mut(uuid, |session| {
                let skill = match slot {
                    CombatSkillSlot::General => session
                        .combat_skills
                        .selected_general
                        .ok_or(SkillUseError::NoSkill)?,
                    CombatSkillSlot::Innate => session
                        .combat_skills
                        .innate_skill
                        .ok_or(SkillUseError::NoSkill)?,
                    CombatSkillSlot::Class => {
                        let class = session.class.class.ok_or(SkillUseError::NoClass)?;
                        session
                            .combat_skills
                            .unlock_class(class, session.stats.rank());
                        CombatSkill::for_class(class)
                    }
                };
                let rank = if slot == CombatSkillSlot::Class {
                    session
                        .combat_skills
                        .class_rank(session.class.class.ok_or(SkillUseError::NoClass)?)
                } else {
                    session.combat_skills.general_rank(skill)
                };
                if rank == 0 {
                    return Err(SkillUseError::NoSkill);
                }
                if !session.combat_skills.ready(skill, now) {
                    let seconds = session
                        .combat_skills
                        .cooldown_millis(skill, now)
                        .div_ceil(1_000);
                    return Err(SkillUseError::Cooldown(seconds));
                }
                if session.stats.rank() < skill.required_rank() {
                    return Err(SkillUseError::RankLocked);
                }
                if skill == CombatSkill::RulersAuthority
                    && session
                        .magic
                        .mana
                        .unwrap_or_else(|| crate::spell::max_mana(session.stats.intelligence))
                        < 10.0
                {
                    return Err(SkillUseError::NoMana);
                }
                Ok((skill, rank, session.stats))
            })
            .ok_or(SkillUseError::NoSkill)??;

        let world = player.get_world();
        match skill {
            CombatSkill::Mutilation => self.start_mutilation(player, &world, rank, stats),
            CombatSkill::Taunt => self.apply_taunt(player, &world, rank, stats, now),
            CombatSkill::Stealth => self.apply_stealth(player, &world, rank, now),
            CombatSkill::VitalShot => shoot_skill_arrow(player, &world, rank, stats.agility, true),
            CombatSkill::RulersAuthority => {
                self.apply_rulers_authority(player, &world, rank, stats)
            }
            CombatSkill::DaggerThrow => {
                shoot_skill_arrow(player, &world, rank, stats.strength, false)
            }
            CombatSkill::ShadowExchange => self.apply_shadow_exchange(player, &world, rank),
            CombatSkill::QuickStep => {
                let impulse = look_direction(player) * (0.8 + f64::from(rank) * 0.3)
                    + DVec3::new(0.0, 0.2, 0.0);
                player.push_impulse(impulse);
                Ok(())
            }
            CombatSkill::SecondWind => {
                if player.get_health() >= player.get_max_health() {
                    Err(SkillUseError::NoTarget)
                } else {
                    player.heal(4.0 + f32::from(rank) * 2.0);
                    Ok(())
                }
            }
            CombatSkill::Repulse => self.apply_repulse(player, &world, rank, stats),
            CombatSkill::ShadowStep => self.apply_shadow_step(player, &world, rank),
        }?;

        self.with_session_mut(uuid, |session| {
            session
                .combat_skills
                .start_cooldown(skill, stats.sense, now);
            if skill == CombatSkill::RulersAuthority {
                let maximum = crate::spell::max_mana(session.stats.intelligence);
                session.magic.mana = Some((session.magic.mana.unwrap_or(maximum) - 10.0).max(0.0));
            }
        });
        self.save_after_skill_change(uuid);
        player.send_action_bar(
            text_components::TextComponent::plain(skill.title())
                .color(text_components::format::Color::Green),
        );
        Ok(())
    }

    fn start_mutilation(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
        stats: HunterStats,
    ) -> Result<(), SkillUseError> {
        let range = 3.5 + f64::from(rank) * 0.5;
        let targets = physical_targets(player, world, range, true);
        if targets.is_empty() {
            return Err(SkillUseError::NoTarget);
        }
        let damage = 1.5 + f32::from(rank) * 0.5 + stats.strength as f32 * 0.03;
        self.with_session_mut(player.gameprofile.id(), |session| {
            session.combat_runtime.flurry = Some(Flurry {
                world: world.key.clone(),
                targets: targets.into_iter().map(|entity| entity.id()).collect(),
                damage,
                remaining: rank + 1,
                next_tick: player.tick_count(),
            });
        });
        player.play_sound(&sound_events::ENTITY_PLAYER_ATTACK_SWEEP, 1.0, 0.7);
        self.tick_combat_skills(player);
        Ok(())
    }

    fn apply_taunt(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
        stats: HunterStats,
        now: u64,
    ) -> Result<(), SkillUseError> {
        let radius = 7.0 + f64::from(rank);
        let attacker: SharedEntity = player.clone();
        for entity in physical_targets(player, world, radius, false) {
            if let Some(mob) = entity.as_mob() {
                mob.set_target(Some(&attacker));
            }
        }
        let shield = 2.0 + f32::from(rank) + stats.vitality as f32 * 0.05;
        self.with_session_mut(player.gameprofile.id(), |session| {
            session.combat_runtime.taunt_until = Some(now + u64::from(3 + rank) * 1_000);
            session.combat_runtime.taunt_absorption = shield;
        });
        player.attributes().lock().set_modifier(
            vanilla_attributes::MAX_ABSORPTION,
            AttributeModifier {
                id: taunt_absorption_id(),
                amount: f64::from(shield),
                operation: AttributeModifierOperation::AddValue,
            },
            false,
        );
        player.set_absorption_amount(player.get_absorption_amount() + shield);
        fx::ring(
            world,
            &vanilla_particle_types::HAPPY_VILLAGER,
            player.position(),
            radius,
            32,
        );
        Ok(())
    }

    fn apply_stealth(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
        now: u64,
    ) -> Result<(), SkillUseError> {
        for entity in physical_targets(player, world, 16.0, false) {
            let Some(mob) = entity.as_mob() else {
                continue;
            };
            if mob
                .target()
                .is_some_and(|target| target.id() == player.id())
            {
                mob.set_target(None);
            }
        }
        self.with_session_mut(player.gameprofile.id(), |session| {
            session.combat_runtime.stealth_until = Some(now + u64::from(2 + rank) * 1_000);
            session.combat_runtime.stealth_strike = true;
        });
        set_invisible(player, true);
        fx::burst(
            world,
            &vanilla_particle_types::SMOKE,
            player.position(),
            30,
            0.7,
            0.02,
        );
        Ok(())
    }

    fn apply_rulers_authority(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
        stats: HunterStats,
    ) -> Result<(), SkillUseError> {
        let radius = 7.0 + f64::from(rank);
        let targets = physical_targets(player, world, radius, false);
        if targets.is_empty() {
            return Err(SkillUseError::NoTarget);
        }
        let damage = 2.0 + f32::from(rank) + stats.intelligence as f32 * 0.1;
        let source = DamageSource::environment(&vanilla_damage_types::MAGIC)
            .with_causing_entity(player.clone());
        for entity in targets {
            let pull = (player.position() - entity.position()).normalize_or_zero() * 0.8
                + DVec3::new(0.0, 0.25, 0.0);
            entity.push_impulse(pull);
            entity.hurt(world, &source, damage);
        }
        fx::ring(
            world,
            &vanilla_particle_types::WITCH,
            player.position(),
            radius,
            40,
        );
        player.play_sound(&sound_events::ENTITY_EVOKER_CAST_SPELL, 1.0, 0.6);
        Ok(())
    }

    fn apply_repulse(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
        stats: HunterStats,
    ) -> Result<(), SkillUseError> {
        let radius = 4.0 + f64::from(rank);
        let targets = physical_targets(player, world, radius, false);
        if targets.is_empty() {
            return Err(SkillUseError::NoTarget);
        }
        let damage = 1.0 + f32::from(rank) + stats.strength as f32 * 0.03;
        let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, player.clone());
        for entity in targets {
            let away = (entity.position() - player.position()).normalize_or_zero() + DVec3::Y * 0.3;
            entity.push_impulse(away);
            entity.hurt(world, &source, damage);
        }
        fx::ring(
            world,
            &vanilla_particle_types::CRIT,
            player.position(),
            radius,
            24,
        );
        Ok(())
    }

    fn apply_shadow_step(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
    ) -> Result<(), SkillUseError> {
        let destination = player.position() + look_direction(player) * f64::from(4 + rank * 2);
        if !standable(world, destination) {
            return Err(SkillUseError::NoTarget);
        }
        player
            .teleport_to(destination)
            .map_err(|_| SkillUseError::NoTarget)?;
        fx::burst(
            world,
            &vanilla_particle_types::PORTAL,
            destination,
            24,
            0.5,
            0.02,
        );
        Ok(())
    }

    fn apply_shadow_exchange(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        rank: u8,
    ) -> Result<(), SkillUseError> {
        let range = 20.0 + f64::from(rank) * 4.0;
        let shadow = self
            .shadow_runtime
            .lock()
            .nearest_shadow(player.gameprofile.id(), world, player.position(), range)
            .ok_or(SkillUseError::NoTarget)?;
        let hunter_position = player.position();
        let shadow_position = shadow.position();
        if !standable(world, hunter_position) || !standable(world, shadow_position) {
            return Err(SkillUseError::NoTarget);
        }
        shadow
            .teleport_to(hunter_position)
            .map_err(|_| SkillUseError::NoTarget)?;
        if player.teleport_to(shadow_position).is_err() {
            let _ = shadow.teleport_to(shadow_position);
            return Err(SkillUseError::NoTarget);
        }
        fx::burst(
            world,
            &vanilla_particle_types::PORTAL,
            hunter_position,
            30,
            0.6,
            0.02,
        );
        fx::burst(
            world,
            &vanilla_particle_types::PORTAL,
            shadow_position,
            30,
            0.6,
            0.02,
        );
        player.play_sound(&sound_events::ENTITY_ENDERMAN_TELEPORT, 1.0, 1.0);
        Ok(())
    }

    /// Advances timed combat effects for one player tick.
    pub fn tick_combat_skills(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let now = CombatSkillBook::now();
        let tick = player.tick_count();
        let (clear_stealth, clear_taunt, flurry) = self
            .with_session_mut(uuid, |session| {
                let clear_stealth = session
                    .combat_runtime
                    .stealth_until
                    .is_some_and(|until| until <= now);
                if clear_stealth {
                    session.combat_runtime.stealth_until = None;
                    session.combat_runtime.stealth_strike = false;
                }
                let clear_taunt = session
                    .combat_runtime
                    .taunt_until
                    .is_some_and(|until| until <= now);
                let taunt_absorption = if clear_taunt {
                    session.combat_runtime.taunt_until = None;
                    std::mem::take(&mut session.combat_runtime.taunt_absorption)
                } else {
                    0.0
                };
                let mut pending = session.combat_runtime.flurry.take();
                let flurry = if let Some(flurry) = pending.as_mut() {
                    if tick < flurry.next_tick {
                        session.combat_runtime.flurry = pending;
                        None
                    } else {
                        let current = (flurry.world.clone(), flurry.targets.clone(), flurry.damage);
                        if flurry.remaining > 1 {
                            flurry.remaining -= 1;
                            flurry.next_tick = tick + 20;
                            session.combat_runtime.flurry = pending;
                        }
                        Some(current)
                    }
                } else {
                    None
                };
                (clear_stealth, taunt_absorption, flurry)
            })
            .unwrap_or((false, 0.0, None));
        if clear_stealth {
            set_invisible(player, false);
        }
        if clear_taunt > 0.0 {
            player
                .attributes()
                .lock()
                .remove_modifier(vanilla_attributes::MAX_ABSORPTION, &taunt_absorption_id());
            player.set_absorption_amount((player.get_absorption_amount() - clear_taunt).max(0.0));
        }
        let Some((world_key, targets, damage)) = flurry else {
            return;
        };
        let world = player.get_world();
        if world.key != world_key {
            return;
        }
        let Some(attacker) = world.get_entity_by_id(player.id()) else {
            return;
        };
        let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, attacker);
        for entity in targets
            .into_iter()
            .filter_map(|id| world.get_entity_by_id(id))
            .filter(|entity| !entity.is_removed())
        {
            entity.hurt(&world, &source, damage);
            fx::burst(
                &world,
                &vanilla_particle_types::CRIT,
                entity.position() + DVec3::new(0.0, 1.0, 0.0),
                8,
                0.25,
                0.02,
            );
        }
    }

    /// Consumes the empowered hit granted by stealth and reveals the hunter.
    pub fn consume_stealth_strike(&self, player: &Player) -> bool {
        let now = CombatSkillBook::now();
        let empowered = self
            .with_session_mut(player.gameprofile.id(), |session| {
                let active = session
                    .combat_runtime
                    .stealth_until
                    .is_some_and(|until| until > now);
                if active && session.combat_runtime.stealth_strike {
                    session.combat_runtime.stealth_until = None;
                    session.combat_runtime.stealth_strike = false;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if empowered {
            set_invisible(player, false);
        }
        empowered
    }
}

fn shoot_skill_arrow(
    player: &Arc<Player>,
    world: &Arc<World>,
    rank: u8,
    stat: u32,
    vital: bool,
) -> Result<(), SkillUseError> {
    let arrow = ArrowEntity::spawn(
        world,
        player.as_ref(),
        None,
        &steel_registry::item_stack::ItemStack::new(&vanilla_items::ARROW),
        if vital { 1.8 } else { 1.5 },
        0.0,
    )
    .ok_or(SkillUseError::NoTarget)?;
    let Some(arrow) = arrow.as_ref().downcast_ref::<ArrowEntity>() else {
        return Err(SkillUseError::NoTarget);
    };
    let base = if vital {
        3.0 + f64::from(rank) + f64::from(stat) * 0.04
    } else {
        2.0 + f64::from(rank) + f64::from(stat) * 0.05
    };
    arrow.set_base_damage(base);
    player.play_sound(
        &sound_events::ENTITY_ARROW_SHOOT,
        1.0,
        if vital { 0.7 } else { 1.1 },
    );
    Ok(())
}

fn physical_targets(
    player: &Player,
    world: &Arc<World>,
    range: f64,
    cone: bool,
) -> Vec<SharedEntity> {
    let position = player.position();
    let forward = look_direction(player);
    let area = WorldAabb::of_size(position, range * 2.0, 6.0, range * 2.0);
    world
        .get_entities_in_aabb_matching(&area, |entity| {
            entity
                .as_mob()
                .is_some_and(|mob| mob.allied_owner().is_none())
        })
        .into_iter()
        .filter(|entity| {
            if entity.is_removed() {
                return false;
            }
            let offset = entity.position() - position;
            let horizontal = DVec3::new(offset.x, 0.0, offset.z);
            if horizontal.length() > range {
                return false;
            }
            if !cone || horizontal.length() < 0.01 {
                return true;
            }
            let direction = horizontal.normalize();
            direction.dot(DVec3::new(forward.x, 0.0, forward.z).normalize_or_zero()) >= 0.7
                && crate::dungeon::boss::line_of_sight(
                    world,
                    position + DVec3::new(0.0, 1.6, 0.0),
                    entity.position() + DVec3::new(0.0, 1.0, 0.0),
                )
        })
        .collect()
}

fn look_direction(player: &Player) -> DVec3 {
    let (yaw, pitch) = player.rotation();
    let yaw = f64::from(yaw).to_radians();
    let pitch = f64::from(pitch).to_radians();
    DVec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
}

fn set_invisible(player: &Player, invisible: bool) {
    if let Some(data) = player.synced_data() {
        data.set_base_invisible_flag(invisible);
    }
}

fn taunt_absorption_id() -> steel_utils::Identifier {
    crate::identifier::from_sololeveling("combat_taunt_absorption")
}

fn standable(world: &World, position: DVec3) -> bool {
    let feet = BlockPos::from(position);
    world.get_block_state(feet).is_air()
        && world.get_block_state(feet.offset(0, 1, 0)).is_air()
        && !world.get_block_state(feet.offset(0, -1, 0)).is_air()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use steel_core::entity::entities::ZombieEntity;
    use steel_core::entity::{SharedEntity, next_entity_id};
    use steel_registry::{vanilla_blocks, vanilla_entities};
    use steel_utils::types::UpdateFlags;

    use super::*;
    use crate::class::HunterClass;
    use crate::player_data::SoloLevelingPlayerData;
    use crate::shadow::Shadow;
    use crate::test_support::TestWorld;

    struct Fixture {
        state: Arc<SoloLevelingState>,
        world: TestWorld,
        player: Arc<Player>,
        directory: PathBuf,
    }

    impl Fixture {
        fn new(class: HunterClass, level: u32) -> Self {
            let directory =
                std::env::temp_dir().join(format!("combat-skill-test-{}", Uuid::new_v4()));
            let state = Arc::new(SoloLevelingState::new(directory.clone()).expect("state"));
            let world =
                crate::test_support::world(Arc::new(steel_core::gameplay_hooks::NoopGameplayHooks));
            crate::test_support::load_full_chunks(
                &world.world,
                vec![steel_utils::ChunkPos::new(0, 0)],
            );
            let player = crate::test_support::player(&world.world, "Hunter", next_entity_id());
            world
                .world
                .try_add_entity(player.clone())
                .expect("player entity");
            let uuid = player.gameprofile.id();
            let mut data = SoloLevelingPlayerData::new(uuid);
            data.stats.level = level;
            data.class.class = Some(class);
            state.begin_session(uuid, data);
            state.with_session_mut(uuid, |session| {
                session.magic.mana = Some(100.0);
                session
                    .combat_skills
                    .unlock_class(class, session.stats.rank());
            });
            let _ = player.teleport(DVec3::new(0.5, 65.0, 0.5), 0.0, 0.0);
            Self {
                state,
                world,
                player,
                directory,
            }
        }

        fn mob(&self, x: f64, z: f64) -> SharedEntity {
            let entity = Arc::new(ZombieEntity::new(
                &vanilla_entities::ZOMBIE,
                next_entity_id(),
                DVec3::new(x, 65.0, z),
                Arc::downgrade(&self.world.world),
            ));
            self.world
                .world
                .try_add_entity(entity.clone())
                .expect("mob");
            entity
        }

        fn use_general(&self, skill: CombatSkill) -> Result<(), SkillUseError> {
            self.state
                .upgrade_general_skill(self.player.gameprofile.id(), skill)
                .expect("general skill upgrade");
            self.state
                .use_combat_skill(&self.player, CombatSkillSlot::General)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn mutilation_hits_in_series_and_starts_cooldown() {
        let fixture = Fixture::new(HunterClass::Fighter, 10);
        let mob = fixture.mob(0.5, 3.5);
        let living = mob.as_living_entity().expect("living");
        let initial = living.get_health();

        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::Class)
            .expect("mutilation");
        assert!((living.get_health() - (initial - 2.0)).abs() < 0.1);
        assert!(matches!(
            fixture
                .state
                .use_combat_skill(&fixture.player, CombatSkillSlot::Class),
            Err(SkillUseError::Cooldown(_))
        ));

        living.set_invulnerable_time(0);
        fixture
            .state
            .with_session_mut(fixture.player.gameprofile.id(), |session| {
                if let Some(flurry) = &mut session.combat_runtime.flurry {
                    flurry.next_tick = 0;
                }
            });
        fixture.state.tick_combat_skills(&fixture.player);
        assert!((living.get_health() - (initial - 4.0)).abs() < 0.2);
        assert!(
            fixture
                .state
                .with_session_mut(fixture.player.gameprofile.id(), |session| {
                    session.combat_runtime.flurry.is_none()
                })
                .unwrap_or(false)
        );
    }

    #[test]
    fn taunt_and_stealth_have_timed_effects() {
        let fixture = Fixture::new(HunterClass::Tanker, 10);
        let mob = fixture.mob(0.5, 3.5);
        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::Class)
            .expect("taunt");
        assert!(
            mob.as_mob()
                .and_then(|mob| mob.target())
                .is_some_and(|target| target.id() == fixture.player.id())
        );
        assert!(fixture.player.get_absorption_amount() > 0.0);
        fixture
            .state
            .with_session_mut(fixture.player.gameprofile.id(), |session| {
                session.combat_runtime.taunt_until = Some(0);
            });
        fixture.state.tick_combat_skills(&fixture.player);
        assert_eq!(fixture.player.get_absorption_amount(), 0.0);

        fixture
            .state
            .with_session_mut(fixture.player.gameprofile.id(), |session| {
                session.class.class = Some(HunterClass::Assassin);
                session
                    .combat_skills
                    .unlock_class(HunterClass::Assassin, session.stats.rank());
            });
        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::Class)
            .expect("stealth");
        assert!(fixture.player.is_invisible());
        assert!(fixture.state.consume_stealth_strike(&fixture.player));
        assert!(!fixture.player.is_invisible());
        assert!(!fixture.state.consume_stealth_strike(&fixture.player));
    }

    #[test]
    fn ruler_spends_mana_and_damages_targets() {
        let fixture = Fixture::new(HunterClass::Mage, 10);
        let mob = fixture.mob(0.5, 3.5);
        let living = mob.as_living_entity().expect("living");
        let initial = living.get_health();
        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::Class)
            .expect("ruler");
        assert_eq!(living.get_health(), initial - 3.0);
        assert_eq!(
            fixture
                .state
                .with_session_mut(fixture.player.gameprofile.id(), |session| {
                    session.magic.mana
                })
                .flatten(),
            Some(90.0)
        );
        assert!(matches!(
            fixture
                .state
                .use_combat_skill(&fixture.player, CombatSkillSlot::Class),
            Err(SkillUseError::Cooldown(_))
        ));
    }

    #[test]
    fn ranged_skills_and_shadow_exchange_use_real_entities() {
        let fixture = Fixture::new(HunterClass::Ranger, 50);
        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::Class)
            .expect("vital shot");
        fixture
            .use_general(CombatSkill::DaggerThrow)
            .expect("dagger");
        assert!(matches!(
            fixture.use_general(CombatSkill::DaggerThrow),
            Err(SkillUseError::Cooldown(_))
        ));
        let arrows = fixture.world.world.get_entities_in_aabb_matching(
            &WorldAabb::of_size(fixture.player.position(), 100.0, 100.0, 100.0),
            |entity| entity.entity_type().key == vanilla_entities::ARROW.key,
        );
        assert_eq!(arrows.len(), 2);

        let shadow = Shadow::new(steel_utils::Identifier::vanilla_static("zombie"), 0, None);
        fixture
            .state
            .shadow_runtime
            .lock()
            .spawn_for_test(&fixture.player, &shadow);
        let shadow = fixture
            .state
            .shadow_runtime
            .lock()
            .nearest_shadow(
                fixture.player.gameprofile.id(),
                &fixture.world.world,
                fixture.player.position(),
                100.0,
            )
            .expect("spawned shadow");
        for position in [fixture.player.position(), DVec3::new(4.5, 65.0, 0.5)] {
            let floor = BlockPos::from(position).offset(0, -1, 0);
            assert!(fixture.world.world.set_block(
                floor,
                vanilla_blocks::STONE.default_state(),
                UpdateFlags::UPDATE_ALL,
            ));
        }
        shadow
            .teleport_to(DVec3::new(4.5, 65.0, 0.5))
            .expect("move shadow");
        let hunter_position = fixture.player.position();
        let shadow_position = shadow.position();
        fixture
            .use_general(CombatSkill::ShadowExchange)
            .expect("exchange");
        assert_eq!(fixture.player.position(), shadow_position);
        assert_eq!(shadow.position(), hunter_position);
    }

    #[test]
    fn new_general_skills_work_from_e_rank() {
        let fixture = Fixture::new(HunterClass::Fighter, 20);
        let uuid = fixture.player.gameprofile.id();
        fixture
            .use_general(CombatSkill::QuickStep)
            .expect("quick step");
        assert!(fixture.player.velocity().length() > 0.0);
        fixture.state.with_session_mut(uuid, |session| {
            session.combat_skills.innate_skill = Some(CombatSkill::QuickStep);
        });
        assert!(matches!(
            fixture
                .state
                .use_drop_key_combat_skill(&fixture.player, CombatSkillSlot::Innate),
            Err(SkillUseError::Cooldown(_))
        ));

        fixture
            .state
            .upgrade_general_skill(uuid, CombatSkill::SecondWind)
            .expect("learn second wind");
        fixture.state.with_session_mut(uuid, |session| {
            session.combat_skills.innate_skill = Some(CombatSkill::SecondWind);
        });
        assert_eq!(
            fixture
                .state
                .use_drop_key_combat_skill(&fixture.player, CombatSkillSlot::Innate),
            Err(SkillUseError::NoTarget)
        );
        fixture.player.set_health(10.0);
        fixture
            .state
            .use_drop_key_combat_skill(&fixture.player, CombatSkillSlot::Innate)
            .expect("innate second wind");
        assert!(fixture.player.get_health() > 10.0);

        let mob = fixture.mob(0.5, 3.5);
        let living = mob.as_living_entity().expect("living");
        let initial = living.get_health();
        fixture.use_general(CombatSkill::Repulse).expect("repulse");
        assert!(living.get_health() < initial);
        assert!(mob.velocity().length() > 0.0);

        let rank = fixture
            .state
            .upgrade_general_skill(uuid, CombatSkill::ShadowStep)
            .expect("learn shadow step");
        let destination = fixture.player.position() + DVec3::Z * f64::from(4 + rank * 2);
        let floor = BlockPos::from(destination).offset(0, -1, 0);
        assert!(fixture.world.world.set_block(
            floor,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        fixture
            .player
            .teleport(fixture.player.position(), 0.0, -90.0)
            .expect("aim up");
        assert_eq!(
            fixture
                .state
                .use_combat_skill(&fixture.player, CombatSkillSlot::General),
            Err(SkillUseError::NoTarget)
        );
        fixture
            .player
            .teleport(fixture.player.position(), 0.0, 0.0)
            .expect("aim forward");
        assert_eq!(fixture.player.rotation(), (0.0, 0.0));
        assert!(
            standable(&fixture.world.world, destination),
            "destination {destination:?} has invalid support at {floor:?}"
        );
        fixture
            .state
            .use_combat_skill(&fixture.player, CombatSkillSlot::General)
            .expect("safe shadow step after rejected destination");
        assert_eq!(fixture.player.position(), destination);
        assert_eq!(
            fixture
                .state
                .upgrade_general_skill(uuid, CombatSkill::ShadowExchange),
            Ok(1)
        );
    }
}
