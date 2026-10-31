//! Runtime-only summoning, ownership, and progression for retained shadows.

use std::fmt;
use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::{
    AlliedMode, Entity, LivingEntity, RemovalReason, SharedEntity, spawn_mob,
};
use steel_core::player::Player;
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::particle_type::ParticleData;
use steel_registry::{REGISTRY, RegistryExt, vanilla_attributes, vanilla_particle_types};
use steel_utils::{BlockPos, Identifier};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use text_components::resolving::NoResolutor;
use uuid::Uuid;

use crate::dungeon::boss::{BossAbilityAction, BossFight, boss_for_gate};
use crate::dungeon::mobs::{MobLabel, MobRarity, clear_health_line};
use crate::dungeon::skills::{BROOD_COOLDOWN_TICKS, hatch_shadow};
use crate::item::nbt;
use crate::rank::HunterRank;
use crate::session::HunterSession;
use crate::shadow::{
    ARISE_RANGE, ATTACK_PER_POINT, BOSS_EXTRACTION_ATTEMPTS, BossSource,
    ELITE_SHADOW_DAMAGE_MULTIPLIER, ELITE_SHADOW_HEALTH_MULTIPLIER, ELITE_SHADOW_SPEED_MULTIPLIER,
    EliteShadow, ExtractionResult, HEALTH_PER_POINT, MAX_ELITE_SHADOW_SLOTS, SPEED_PER_POINT,
    Shadow, ShadowGear, ShadowOrigin, ShadowStat, army_limit, attempt_extraction,
};
use steel_core::inventory::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;

/// Slots whose equipment a shadow inherits.
const GEAR_SLOTS: [EquipmentSlot; 6] = [
    EquipmentSlot::MainHand,
    EquipmentSlot::OffHand,
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];

struct ActiveShadow {
    owner: Uuid,
    shadow_id: Uuid,
    entity: SharedEntity,
    brood: Option<ShadowBrood>,
}

struct ActiveEliteShadow {
    owner: Uuid,
    shadow_id: Uuid,
    entity: SharedEntity,
}

/// Live archetype-ability fight of one summoned elite shadow body.
struct EliteFight {
    owner: Uuid,
    entity: SharedEntity,
    fight: BossFight,
}

/// How far from its body an elite shadow picks ability targets.
const ELITE_TARGET_RANGE: f64 = 24.0;

#[derive(Default)]
struct ShadowBrood {
    ready_tick: Option<u64>,
    larvae: Vec<SharedEntity>,
}

impl ShadowBrood {
    fn discard(&mut self) {
        for larva in self.larvae.drain(..) {
            Self::clear_health_line(&larva);
            larva.set_removed(RemovalReason::Discarded);
        }
    }

    fn clear_health_line(larva: &SharedEntity) {
        if let Some(world) = larva.level()
            && let Some(living) = larva.as_living_entity()
        {
            clear_health_line(&world, living);
        }
    }
}

/// Ephemeral data that must disappear on server restart.
#[derive(Default)]
pub struct ShadowRuntime {
    active: FxHashMap<Uuid, ActiveShadow>,
    elite_active: FxHashMap<Uuid, ActiveEliteShadow>,
    /// Ability fights of summoned elite bodies, keyed by body entity uuid.
    elite_fights: FxHashMap<Uuid, EliteFight>,
    /// Army-wide orders per owner; runtime-only, so a restart resets to follow.
    modes: FxHashMap<Uuid, AlliedMode>,
}

impl fmt::Debug for ShadowRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShadowRuntime")
            .field("active_count", &self.active.len())
            .field("elite_active_count", &self.elite_active.len())
            .field("elite_fight_count", &self.elite_fights.len())
            .field("modes", &self.modes)
            .finish()
    }
}

impl ShadowRuntime {
    /// Removes runtime entries whose entities were discarded outside a death hook.
    pub fn tick(&mut self, tick: u64) {
        self.active.retain(|_, active| {
            if active.entity.is_removed()
                || active
                    .entity
                    .as_living_entity()
                    .is_none_or(LivingEntity::is_dead_or_dying)
            {
                if let Some(brood) = &mut active.brood {
                    brood.discard();
                }
                return false;
            }
            if let Some(brood) = &mut active.brood {
                brood.tick(&active.entity, active.owner, tick);
            }
            true
        });
        self.elite_active.retain(|_, active| {
            !active.entity.is_removed()
                && active
                    .entity
                    .as_living_entity()
                    .is_some_and(|living| !living.is_dead_or_dying())
        });
    }

    /// Advances the archetype ability fights of summoned elite shadows.
    ///
    /// Returns the ability actions to resolve after the runtime lock is
    /// released; each cast is announced to the shadow's owner. The dungeon
    /// fight engine targets the uuid list it is given, so the unallied mobs
    /// near the shadow stand in for the hunter party it was written for.
    pub fn tick_elite_fights(&mut self, server: &Arc<Server>, tick: u64) -> Vec<BossAbilityAction> {
        self.elite_fights.retain(|_, entry| {
            !entry.entity.is_removed()
                && entry
                    .entity
                    .as_living_entity()
                    .is_some_and(|living| !living.is_dead_or_dying())
        });
        if self.elite_fights.is_empty() {
            return Vec::new();
        }
        let players = server.get_players();
        let mut actions = Vec::new();
        for entry in self.elite_fights.values_mut() {
            let Some(owner) = players
                .iter()
                .find(|player| player.gameprofile.id() == entry.owner)
            else {
                continue;
            };
            let Some(world) = entry.entity.level() else {
                continue;
            };
            let targets = elite_targets(&world, entry.entity.as_ref());
            if let Some(action) = entry.fight.tick(&world, owner, &targets, tick) {
                actions.push(action);
            }
        }
        actions
    }

    /// Retains a nearby mob the player just killed, with an immediate soul effect.
    pub fn arise_from_player_kill(
        hunter: &Player,
        world: &World,
        entity: &dyn LivingEntity,
        label: Option<&MobLabel>,
        session: &mut HunterSession,
    ) -> Option<Shadow> {
        if entity.as_mob().is_none() || hunter.get_world().key != world.key {
            return None;
        }
        if hunter.position().distance(entity.position()) > ARISE_RANGE {
            return None;
        }
        // Most kills leave nothing behind; rare mobs and bosses even less often.
        let chance = label.map_or(MobRarity::Common, |label| label.rarity);
        if rand::random_range(0..100) >= chance.arise_chance_percent() {
            return None;
        }
        let shadow = session
            .shadows
            .arise_from(
                entity.entity_type().key.clone(),
                Some(capture_origin(entity, label)),
            )
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
            if let Some(mut active) = self.active.remove(id) {
                if let Some(brood) = &mut active.brood {
                    brood.discard();
                }
                active.entity.set_removed(RemovalReason::Discarded);
            }
        }
        ids.len() + self.recall_elite(owner)
    }

    /// Attempts to extract an elite shadow from a freshly defeated boss.
    ///
    /// Returns `Some` when the ritual succeeds and the boss joins the elite
    /// roster; `None` covers both failure and missing prerequisites.
    ///
    /// # Panics
    ///
    /// Panics only when the elite-roster slot check above is wrong; the slot
    /// is verified before rolling.
    #[expect(
        clippy::unused_self,
        reason = "kept as an instance method for symmetry with other shadow-runtime operations"
    )]
    pub fn extract_boss_shadow(
        &mut self,
        hunter: &Player,
        entity: &dyn LivingEntity,
        source: BossSource,
        rank: HunterRank,
        session: &mut HunterSession,
    ) -> Option<EliteShadow> {
        if hunter.get_world().key != entity.level()?.key {
            return None;
        }
        if session.shadows.shadows.is_empty() {
            return None;
        }
        if session.elite_shadows.shadows.len() >= MAX_ELITE_SHADOW_SLOTS {
            return None;
        }
        if attempt_extraction(rank, BOSS_EXTRACTION_ATTEMPTS, || {
            rand::random_range(0..100)
        }) == ExtractionResult::Failure
        {
            return None;
        }
        let origin = capture_origin(entity, None);
        let mut elite = EliteShadow::new(entity.entity_type().key.clone(), origin, source);
        elite.scale = capture_scale(entity);
        let elite = session
            .elite_shadows
            .add(elite)
            .expect("slot was checked above")
            .clone();
        Some(elite)
    }

    /// Summons one elite shadow next to its owner.
    pub fn summon_elite(
        &mut self,
        hunter: &Player,
        session: &HunterSession,
        name: &str,
    ) -> Result<(), &'static str> {
        let Some(shadow) = session.elite_shadows.named(name) else {
            return Err("Элитная тень с таким именем не найдена.");
        };
        if shadow.is_recovering() {
            return Err("Эта элитная тень ещё восстанавливается после гибели.");
        }
        if self
            .elite_active
            .values()
            .any(|active| active.owner == hunter.gameprofile.id() && active.shadow_id == shadow.id)
        {
            return Err("Эта элитная тень уже призвана.");
        }
        if self
            .elite_active
            .values()
            .filter(|active| active.owner == hunter.gameprofile.id())
            .count()
            >= MAX_ELITE_SHADOW_SLOTS
        {
            return Err("Достигнут лимит призванных элитных теней.");
        }
        self.spawn_elite(hunter, shadow)
    }

    /// Summons every available elite shadow up to the elite slot limit.
    pub fn summon_all_elite(&mut self, hunter: &Player, session: &HunterSession) -> usize {
        let mut summoned = 0;
        for shadow in &session.elite_shadows.shadows {
            if self
                .elite_active
                .values()
                .filter(|active| active.owner == hunter.gameprofile.id())
                .count()
                >= MAX_ELITE_SHADOW_SLOTS
            {
                break;
            }
            if self.elite_active.values().any(|active| {
                active.owner == hunter.gameprofile.id() && active.shadow_id == shadow.id
            }) {
                continue;
            }
            if shadow.is_recovering() {
                continue;
            }
            if self.spawn_elite(hunter, shadow).is_ok() {
                summoned += 1;
            }
        }
        summoned
    }

    /// Removes all elite shadows currently summoned by one hunter.
    pub fn recall_elite(&mut self, owner: Uuid) -> usize {
        let ids: Vec<_> = self
            .elite_active
            .iter()
            .filter_map(|(id, active)| (active.owner == owner).then_some(*id))
            .collect();
        for id in &ids {
            if let Some(active) = self.elite_active.remove(id) {
                self.elite_fights.remove(id);
                active.entity.set_removed(RemovalReason::Discarded);
            }
        }
        ids.len()
    }

    /// Returns the roster ids of the elite shadows one hunter has summoned.
    #[must_use]
    pub fn active_elite_shadows(&self, owner: Uuid) -> Vec<Uuid> {
        self.elite_active
            .values()
            .filter(|active| active.owner == owner && !active.entity.is_removed())
            .map(|active| active.shadow_id)
            .collect()
    }

    /// Removes one summoned elite shadow, returning whether it was out.
    pub fn dismiss_elite(&mut self, owner: Uuid, shadow_id: Uuid) -> bool {
        let Some(entity_id) = self.elite_active.iter().find_map(|(id, active)| {
            (active.owner == owner && active.shadow_id == shadow_id).then_some(*id)
        }) else {
            return false;
        };
        if let Some(active) = self.elite_active.remove(&entity_id) {
            self.elite_fights.remove(&entity_id);
            active.entity.set_removed(RemovalReason::Discarded);
        }
        true
    }

    /// Returns the army-wide mode a hunter gave their shadows.
    #[must_use]
    pub fn mode(&self, owner: Uuid) -> AlliedMode {
        self.modes.get(&owner).copied().unwrap_or_default()
    }

    /// Orders every summoned shadow of one hunter into `mode`.
    pub fn set_mode(&mut self, owner: Uuid, mode: AlliedMode) {
        self.modes.insert(owner, mode);
        for active in self.active.values().filter(|active| active.owner == owner) {
            if let Some(mob) = active.entity.as_mob() {
                mob.set_allied_mode(mode);
            }
            if let Some(brood) = &active.brood {
                for larva in &brood.larvae {
                    if let Some(mob) = larva.as_mob() {
                        mob.set_allied_mode(mode);
                    }
                }
            }
        }
        for active in self
            .elite_active
            .values()
            .filter(|active| active.owner == owner)
        {
            if let Some(mob) = active.entity.as_mob() {
                mob.set_allied_mode(mode);
            }
        }
    }

    /// Points every summoned shadow of one hunter at `target`.
    pub fn focus(&self, owner: Uuid, target: &SharedEntity) {
        for active in self.active.values().filter(|active| active.owner == owner) {
            if let Some(mob) = active.entity.as_mob() {
                mob.set_allied_focus(Some(target));
            }
            if let Some(brood) = &active.brood {
                for larva in &brood.larvae {
                    if let Some(mob) = larva.as_mob() {
                        mob.set_allied_focus(Some(target));
                    }
                }
            }
        }
        for active in self
            .elite_active
            .values()
            .filter(|active| active.owner == owner)
        {
            if let Some(mob) = active.entity.as_mob() {
                mob.set_allied_focus(Some(target));
            }
        }
    }

    /// Returns summoned shadows left in another world than their owner.
    ///
    /// Teleporting inside one world is the ally AI's job; crossing worlds is
    /// done by summoning the body again next to the owner.
    pub fn displaced(&self, owner_world: impl Fn(Uuid) -> Option<Identifier>) -> Vec<(Uuid, Uuid)> {
        let mut displaced = Vec::new();
        for active in self.active.values().filter(|active| {
            let shadow_world = active.entity.level().map(|world| world.key.clone());
            owner_world(active.owner).is_some_and(|world| Some(world) != shadow_world)
        }) {
            displaced.push((active.owner, active.shadow_id));
        }
        for active in self.elite_active.values().filter(|active| {
            let shadow_world = active.entity.level().map(|world| world.key.clone());
            owner_world(active.owner).is_some_and(|world| Some(world) != shadow_world)
        }) {
            displaced.push((active.owner, active.shadow_id));
        }
        displaced
    }

    /// Returns the roster ids of the shadows one hunter has summoned.
    #[must_use]
    pub fn active_shadows(&self, owner: Uuid) -> Vec<Uuid> {
        self.active
            .values()
            .filter(|active| active.owner == owner && !active.entity.is_removed())
            .map(|active| active.shadow_id)
            .collect()
    }

    /// Removes one summoned shadow, returning whether it was out.
    pub fn dismiss(&mut self, owner: Uuid, shadow_id: Uuid) -> bool {
        let Some(entity_id) = self.active.iter().find_map(|(id, active)| {
            (active.owner == owner && active.shadow_id == shadow_id).then_some(*id)
        }) else {
            return false;
        };
        if let Some(mut active) = self.active.remove(&entity_id) {
            if let Some(brood) = &mut active.brood {
                brood.discard();
            }
            active.entity.set_removed(RemovalReason::Discarded);
        }
        true
    }

    /// Removes a dead body and returns the persistent shadow that must rebuild.
    pub fn observe_death(&mut self, entity_uuid: Uuid) -> Option<(Uuid, Uuid, u16)> {
        if let Some(mut active) = self.active.remove(&entity_uuid) {
            if let Some(brood) = &mut active.brood {
                brood.discard();
            }
            let maximum_health = active
                .entity
                .as_living_entity()
                .map_or(1.0, LivingEntity::get_max_health)
                .ceil()
                .clamp(1.0, f32::from(u16::MAX)) as u16;
            return Some((active.owner, active.shadow_id, maximum_health));
        }
        let active = self.elite_active.remove(&entity_uuid)?;
        self.elite_fights.remove(&entity_uuid);
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
        let (active, killer) = self.kill_source(killer_uuid)?;
        let reward = victim
            .experience_reward(world, Some(killer.as_ref()))
            .max(0) as u64;
        Some((active.owner, active.shadow_id, reward))
    }

    fn kill_source(&self, killer_uuid: Uuid) -> Option<(&ActiveShadow, &SharedEntity)> {
        if let Some(active) = self.active.get(&killer_uuid) {
            return Some((active, &active.entity));
        }
        self.active.values().find_map(|active| {
            let brood = active.brood.as_ref()?;
            brood
                .larvae
                .iter()
                .find(|larva| larva.uuid() == killer_uuid)
                .map(|larva| (active, larva))
        })
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

    /// Allocates one point and refreshes the matching active elite body.
    pub fn upgrade_elite(
        &mut self,
        owner: Uuid,
        session: &mut HunterSession,
        name: &str,
        stat: ShadowStat,
    ) -> Result<EliteShadow, &'static str> {
        let Some(shadow) = session.elite_shadows.named_mut(name) else {
            return Err("Элитная тень с таким именем не найдена.");
        };
        if !shadow.allocate_point(stat) {
            return Err("У этой элитной тени нет нераспределённых очков.");
        }
        let shadow = shadow.clone();
        for active in self.elite_active.values() {
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
        if let Some(mob) = entity.as_mob() {
            mob.set_allied_mode(self.mode(hunter.gameprofile.id()));
            mob.set_allied_tank(shadow.origin.as_ref().is_some_and(|origin| origin.is_tank));
        }
        entity.set_custom_name(Some(
            TextComponent::plain(format!("[Тень] {} · ур. {}", shadow.name, shadow.level))
                .color(Color::DarkPurple),
        ));
        entity.set_custom_name_visible(true);
        if let Some(origin) = &shadow.origin {
            apply_origin(entity.as_ref(), origin);
        }
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
                brood: shadow
                    .origin
                    .as_ref()
                    .is_some_and(|origin| origin.is_broodmother == Some(true))
                    .then(ShadowBrood::default),
            },
        );
        Ok(())
    }

    fn spawn_elite(&mut self, hunter: &Player, shadow: &EliteShadow) -> Result<(), &'static str> {
        let Some(entity_type) = REGISTRY.entity_types.by_key(&shadow.entity_type) else {
            return Err("Тип этой элитной тени больше не зарегистрирован.");
        };
        let world = hunter.get_world();
        let entity = spawn_mob(&world, entity_type, BlockPos::from(hunter.position()))
            .map_err(|_| "Не удалось призвать элитную тень в этой точке.")?;
        if entity.as_mob().is_none() {
            entity.set_removed(RemovalReason::Discarded);
            return Err("Тип сохранённой элитной тени не является мобом.");
        }
        entity.set_runtime_allied_owner(Some(hunter.gameprofile.id()));
        if let Some(mob) = entity.as_mob() {
            mob.set_allied_mode(self.mode(hunter.gameprofile.id()));
            mob.set_allied_tank(shadow.origin.is_tank);
        }
        entity.set_custom_name(Some(
            TextComponent::plain(format!(
                "[Элитная тень] {} · ур. {}",
                shadow.name, shadow.level
            ))
            .color(Color::DarkRed),
        ));
        entity.set_custom_name_visible(true);
        apply_origin(entity.as_ref(), &shadow.origin);
        apply_elite_stats(entity.as_ref(), shadow);
        if shadow.scale > 0 {
            if let Some(living) = entity.as_living_entity() {
                living.set_base_attribute_value(
                    vanilla_attributes::SCALE,
                    ShadowOrigin::unscale(shadow.scale),
                );
            }
            entity.refresh_dimensions();
        }
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SOUL_FIRE_FLAME),
            entity.position(),
            32,
            DVec3::new(0.4, 0.7, 0.4),
            0.03,
        );
        if let Some(fight) = elite_fight(shadow, entity.id()) {
            self.elite_fights.insert(
                entity.uuid(),
                EliteFight {
                    owner: hunter.gameprofile.id(),
                    entity: Arc::clone(&entity),
                    fight,
                },
            );
        }
        self.elite_active.insert(
            entity.uuid(),
            ActiveEliteShadow {
                owner: hunter.gameprofile.id(),
                shadow_id: shadow.id,
                entity,
            },
        );
        Ok(())
    }
}

impl ShadowBrood {
    fn tick(&mut self, mother: &SharedEntity, owner: Uuid, tick: u64) {
        self.larvae.retain(|larva| {
            let live = !larva.is_removed()
                && larva
                    .as_living_entity()
                    .is_some_and(|living| !living.is_dead_or_dying());
            if !live {
                Self::clear_health_line(larva);
            }
            live
        });
        let ready_tick = *self.ready_tick.get_or_insert(tick + 40);
        if tick < ready_tick || self.larvae.len() >= 2 {
            return;
        }
        let Some(mob) = mother.as_mob() else {
            return;
        };
        if mob.allied_mode() == AlliedMode::Passive
            || mob
                .target()
                .is_none_or(|target| !mob.allied_may_fight(target.as_ref()))
        {
            return;
        }
        let Some(world) = mother.level() else {
            return;
        };
        self.ready_tick = Some(tick + BROOD_COOLDOWN_TICKS);
        let position = mother.position();
        let mut occupied = self
            .larvae
            .iter()
            .map(|larva| larva.position())
            .collect::<Vec<_>>();
        occupied.push(position);
        while self.larvae.len() < 2 {
            let Some(larva) = hatch_shadow(&world, position, &occupied) else {
                break;
            };
            larva.set_runtime_allied_owner(Some(owner));
            if let Some(larva_mob) = larva.as_mob() {
                larva_mob.set_allied_mode(mob.allied_mode());
                larva_mob.set_allied_focus(mob.allied_focus().as_ref());
            }
            occupied.push(larva.position());
            self.larvae.push(larva);
        }
    }
}

/// Mobs an elite shadow may spend its abilities on: the unallied hostiles
/// near its body. Players and every hunter's summons are excluded.
fn elite_targets(world: &World, shadow: &dyn Entity) -> Vec<Uuid> {
    let area = shadow.bounding_box().inflate(ELITE_TARGET_RANGE);
    world
        .get_entities_in_aabb_matching(&area, |entity| {
            entity.uuid() != shadow.uuid()
                && entity.as_player().is_none()
                && entity
                    .as_living_entity()
                    .is_some_and(|living| !living.is_dead_or_dying())
                && entity
                    .as_mob()
                    .is_some_and(|mob| mob.allied_owner().is_none())
        })
        .into_iter()
        .map(|entity| entity.uuid())
        .collect()
}

/// Starts the archetype ability fight of a freshly summoned elite body.
///
/// Ability damage scales from the elite body's inherited attack with the elite
/// damage multiplier; the fight opens without a boss bar or an intro delay.
fn elite_fight(shadow: &EliteShadow, boss_id: i32) -> Option<BossFight> {
    let kind = shadow.source.boss_kind()?;
    let rank = shadow.source.ability_rank();
    let mut template = boss_for_gate(kind, rank, false);
    if template.abilities.is_empty() {
        return None;
    }
    template.power =
        ShadowOrigin::unscale(shadow.origin.attack_damage) * ELITE_SHADOW_DAMAGE_MULTIPLIER;
    Some(BossFight::new(&template, boss_id, 0))
}

/// Captures the body scale an elite shadow inherits, in thousandths; a
/// vanilla-sized body stores zero so old and new saves behave the same.
fn capture_scale(entity: &dyn LivingEntity) -> u32 {
    let scale = entity
        .attributes()
        .lock()
        .get_base_value(vanilla_attributes::SCALE)
        .unwrap_or(1.0);
    if (scale - 1.0).abs() < 0.001 {
        0
    } else {
        ShadowOrigin::scale(scale)
    }
}

/// Records what a dying mob was: its nameplate, base attributes and gear.
fn capture_origin(entity: &dyn LivingEntity, label: Option<&MobLabel>) -> ShadowOrigin {
    let title = label.map_or_else(
        || {
            entity
                .custom_name()
                .map(|name| name.to_plain(&NoResolutor))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| mob_name(&entity.entity_type().key.path).to_owned())
        },
        |label| format!("{} {}", label.name, label.title),
    );
    let attributes = entity.attributes().lock();
    let base = |attribute| ShadowOrigin::scale(attributes.get_base_value(attribute).unwrap_or(0.0));
    let origin_stats = (
        base(vanilla_attributes::MAX_HEALTH),
        base(vanilla_attributes::ATTACK_DAMAGE),
        base(vanilla_attributes::MOVEMENT_SPEED),
        base(vanilla_attributes::ARMOR),
    );
    drop(attributes);
    let mut equipment = Vec::new();
    for slot in GEAR_SLOTS {
        let mut stack = ItemStack::empty();
        entity.with_equipment_slot(slot, &mut |item| stack = item.clone());
        if stack.is_empty() {
            continue;
        }
        match nbt::encode(&stack) {
            Ok(item) => equipment.push(ShadowGear {
                slot: slot.index() as u8,
                item,
            }),
            Err(error) => log::warn!("sololeveling: shadow gear was not kept: {error}"),
        }
    }
    ShadowOrigin {
        title,
        max_health: origin_stats.0,
        attack_damage: origin_stats.1,
        movement_speed: origin_stats.2,
        armor: origin_stats.3,
        equipment,
        is_tank: label.is_some_and(|label| label.is_tank),
        is_broodmother: Some(label.is_some_and(|label| label.is_broodmother)),
    }
}

/// Russian names of the vanilla mobs a shadow may arise from outside a dungeon label.
fn mob_name(path: &str) -> &str {
    match path {
        "zombie" => "Зомби",
        "husk" => "Кадавр",
        "drowned" => "Утопленник",
        "zombie_villager" => "Зомби-житель",
        "skeleton" => "Скелет",
        "stray" => "Зимогор",
        "bogged" => "Болотник",
        "wither_skeleton" => "Скелет-иссушитель",
        "spider" => "Паук",
        "cave_spider" => "Пещерный паук",
        "creeper" => "Крипер",
        "enderman" => "Эндермен",
        "piglin" => "Пиглин",
        "piglin_brute" => "Пиглин-громила",
        "zombified_piglin" => "Зомбифицированный пиглин",
        "vindicator" => "Поборник",
        "pillager" => "Разбойник",
        "witch" => "Ведьма",
        other => other,
    }
}

/// Gives a summoned body the stats and gear its shadow inherited.
///
/// The gear never drops: a shadow body is allied, and allied mobs drop no
/// death loot, so summoning cannot duplicate items.
fn apply_origin(entity: &dyn Entity, origin: &ShadowOrigin) {
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    for (attribute, value) in [
        (vanilla_attributes::MAX_HEALTH, origin.max_health),
        (vanilla_attributes::ATTACK_DAMAGE, origin.attack_damage),
        (vanilla_attributes::MOVEMENT_SPEED, origin.movement_speed),
        (vanilla_attributes::ARMOR, origin.armor),
    ] {
        if value > 0 && living.attributes().lock().has_attribute(attribute) {
            living.set_base_attribute_value(attribute, ShadowOrigin::unscale(value));
        }
    }
    for gear in &origin.equipment {
        let Some(slot) = EquipmentSlot::ALL
            .into_iter()
            .find(|slot| slot.index() == usize::from(gear.slot))
        else {
            continue;
        };
        match nbt::decode(&gear.item) {
            Ok(stack) => living.with_equipment_slot_mut(slot, &mut |item| *item = stack.clone()),
            Err(error) => log::warn!("sololeveling: shadow gear could not be restored: {error}"),
        }
        if let Some(mob) = entity.as_mob() {
            mob.set_equipment_drop_chance(slot, 0.0);
        }
    }
}

fn apply_stats(entity: &dyn Entity, shadow: &Shadow) {
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    // Base values, so the inherited weapon's and armor's own modifiers are
    // not folded into the upgrade arithmetic.
    let attributes = living.attributes().lock();
    let base = |attribute| attributes.get_base_value(attribute).unwrap_or(0.0);
    let health = base(vanilla_attributes::MAX_HEALTH);
    let attack = base(vanilla_attributes::ATTACK_DAMAGE);
    let speed = base(vanilla_attributes::MOVEMENT_SPEED);
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
    living.set_health(living.get_max_health());
}

fn apply_elite_stats(entity: &dyn Entity, shadow: &EliteShadow) {
    let Some(living) = entity.as_living_entity() else {
        return;
    };
    let attributes = living.attributes().lock();
    let base = |attribute| attributes.get_base_value(attribute).unwrap_or(0.0);
    let health = base(vanilla_attributes::MAX_HEALTH);
    let attack = base(vanilla_attributes::ATTACK_DAMAGE);
    let speed = base(vanilla_attributes::MOVEMENT_SPEED);
    drop(attributes);
    let max_health =
        health * ELITE_SHADOW_HEALTH_MULTIPLIER + f64::from(shadow.health) * HEALTH_PER_POINT;
    living.set_base_attribute_value(vanilla_attributes::MAX_HEALTH, max_health);
    living.set_base_attribute_value(
        vanilla_attributes::ATTACK_DAMAGE,
        attack * ELITE_SHADOW_DAMAGE_MULTIPLIER + f64::from(shadow.attack) * ATTACK_PER_POINT,
    );
    living.set_base_attribute_value(
        vanilla_attributes::MOVEMENT_SPEED,
        speed * ELITE_SHADOW_SPEED_MULTIPLIER + f64::from(shadow.speed) * SPEED_PER_POINT,
    );
    living.set_health(living.get_max_health());
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
    let current = living
        .attributes()
        .lock()
        .get_base_value(attribute)
        .unwrap_or(0.0);
    living.set_base_attribute_value(attribute, current + increment);
    if stat == ShadowStat::Health {
        living.set_health(living.get_max_health());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_core::entity::entities::{EndermiteEntity, ZombieEntity};
    use steel_core::entity::{AlliedMode, Entity as _, LivingEntity, SharedEntity};
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use uuid::Uuid;

    use super::{
        ActiveShadow, ShadowBrood, ShadowRuntime, capture_origin, capture_scale, elite_fight,
    };
    use crate::dungeon::generator::Theme;
    use crate::dungeon::mobs::{MobClass, MobLabel};
    use crate::rank::HunterRank;
    use crate::shadow::{BossSource, EliteShadow, ShadowOrigin, elite_ability_kit};

    fn active_brood() -> (ShadowRuntime, Uuid, Uuid, SharedEntity) {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let shadow_id = Uuid::new_v4();
        let mother: SharedEntity = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));
        let larva: SharedEntity = Arc::new(EndermiteEntity::new(
            &vanilla_entities::ENDERMITE,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));
        mother.set_runtime_allied_owner(Some(owner));
        larva.set_runtime_allied_owner(Some(owner));
        let mut runtime = ShadowRuntime::default();
        runtime.active.insert(
            mother.uuid(),
            ActiveShadow {
                owner,
                shadow_id,
                entity: mother,
                brood: Some(ShadowBrood {
                    ready_tick: None,
                    larvae: vec![Arc::clone(&larva)],
                }),
            },
        );
        (runtime, owner, shadow_id, larva)
    }

    #[test]
    fn broodmother_origin_survives_serialization() {
        init_vanilla_registry();
        let mother = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
        let label = MobLabel::mob(Theme::Crypt, MobClass::Broodmother);
        let origin = capture_origin(&mother, Some(&label));
        let data = serde_json::to_string(&origin).expect("serialize origin");
        let restored = serde_json::from_str::<ShadowOrigin>(&data).expect("deserialize origin");
        assert_eq!(restored.is_broodmother, Some(true));
        let guard = MobLabel::mob(Theme::Crypt, MobClass::Guard);
        assert_eq!(
            capture_origin(&mother, Some(&guard)).is_broodmother,
            Some(false)
        );
    }

    #[test]
    fn brood_follows_orders_and_dismissal_discards_it() {
        let (mut runtime, owner, shadow_id, larva) = active_brood();
        assert_eq!(runtime.active_shadows(owner), vec![shadow_id]);
        runtime.set_mode(owner, AlliedMode::Passive);
        assert_eq!(
            larva.as_mob().expect("mob").allied_mode(),
            AlliedMode::Passive
        );
        assert!(runtime.dismiss(owner, shadow_id));
        assert!(larva.is_removed());
    }

    #[test]
    fn brood_is_discarded_on_recall_and_mother_death() {
        let (mut runtime, owner, _, larva) = active_brood();
        assert_eq!(runtime.recall(owner), 1);
        assert!(larva.is_removed());

        let (mut runtime, _, _, larva) = active_brood();
        let mother = *runtime.active.keys().next().expect("mother");
        assert!(runtime.observe_death(mother).is_some());
        assert!(larva.is_removed());
    }

    #[test]
    fn larva_kills_are_credited_to_the_owning_shadow() {
        let (mut runtime, owner, shadow_id, larva) = active_brood();
        let (active, killer) = runtime.kill_source(larva.uuid()).expect("larva source");
        assert_eq!((active.owner, active.shadow_id), (owner, shadow_id));
        assert_eq!(killer.uuid(), larva.uuid());
        assert!(runtime.kill_source(Uuid::new_v4()).is_none());
        assert!(runtime.dismiss(owner, shadow_id));
        assert!(runtime.kill_source(larva.uuid()).is_none());
    }

    #[test]
    fn boss_extraction_takes_the_nameplate_as_the_shadow_name() {
        init_vanilla_registry();
        let zombie = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
        zombie.set_custom_name(Some(text_components::TextComponent::plain(
            "Король муравьёв".to_owned(),
        )));
        let origin = capture_origin(&zombie, None);
        assert_eq!(origin.title, "Король муравьёв");
        assert_eq!(capture_scale(&zombie), 0);

        // A dungeon label still wins: plates like «Мумифицированный Босс»
        // carry the class information the roster shows.
        let label = MobLabel::mob(Theme::Crypt, MobClass::Warrior);
        let labeled = capture_origin(&zombie, Some(&label));
        assert_eq!(labeled.title, "Упырь Воин");
    }

    #[test]
    fn boss_extraction_keeps_an_oversized_body() {
        init_vanilla_registry();
        let zombie = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
        assert_eq!(capture_scale(&zombie), 0, "vanilla size stores as zero");
        zombie.set_base_attribute_value(steel_registry::vanilla_attributes::SCALE, 6.0);
        assert_eq!(capture_scale(&zombie), ShadowOrigin::scale(6.0));
    }

    fn sample_elite(source: BossSource) -> EliteShadow {
        EliteShadow::new(
            steel_utils::Identifier::vanilla_static("zombie"),
            ShadowOrigin {
                title: "Вождь-берсерк".to_owned(),
                max_health: ShadowOrigin::scale(200.0),
                attack_damage: ShadowOrigin::scale(15.0),
                movement_speed: ShadowOrigin::scale(0.23),
                armor: ShadowOrigin::scale(8.0),
                equipment: Vec::new(),
                is_tank: true,
                is_broodmother: Some(false),
            },
            source,
        )
    }

    #[test]
    fn elite_bodies_of_archetype_bosses_fight_with_abilities() {
        init_vanilla_registry();
        let dungeon = sample_elite(BossSource::Dungeon {
            rank: HunterRank::C,
            archetype: "Вождь-берсерк".to_owned(),
            red: false,
        });
        assert!(elite_fight(&dungeon, 1).is_some());

        let king = sample_elite(BossSource::IslandKing);
        assert!(elite_fight(&king, 1).is_some());

        let nest = sample_elite(BossSource::IslandNest {
            zone_rank: HunterRank::A,
            zone_name: "Поля гнёзд".to_owned(),
        });
        assert!(elite_fight(&nest, 1).is_none());

        // Ability damage scales from the inherited attack with the elite
        // multiplier; the kit comes from the archetype at the source rank.
        let (_, rank) = elite_ability_kit(&king.source).expect("the King has a kit");
        assert_eq!(rank, HunterRank::S);
    }
}
