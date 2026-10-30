//! Runtime-only summoning, ownership, and progression for retained shadows.

use std::fmt;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::{
    AlliedMode, Entity, LivingEntity, RemovalReason, SharedEntity, spawn_mob,
};
use steel_core::player::Player;
use steel_core::world::World;
use steel_registry::particle_type::ParticleData;
use steel_registry::{REGISTRY, RegistryExt, vanilla_attributes, vanilla_particle_types};
use steel_utils::{BlockPos, Identifier};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use crate::dungeon::mobs::{MobLabel, MobRarity};
use crate::item::nbt;
use crate::session::HunterSession;
use crate::shadow::{
    ARISE_RANGE, ATTACK_PER_POINT, HEALTH_PER_POINT, SPEED_PER_POINT, Shadow, ShadowGear,
    ShadowOrigin, ShadowStat, army_limit,
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
}

/// Ephemeral data that must disappear on server restart.
#[derive(Default)]
pub struct ShadowRuntime {
    active: FxHashMap<Uuid, ActiveShadow>,
    /// Army-wide orders per owner; runtime-only, so a restart resets to follow.
    modes: FxHashMap<Uuid, AlliedMode>,
}

impl fmt::Debug for ShadowRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShadowRuntime")
            .field("active_count", &self.active.len())
            .field("modes", &self.modes)
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
            if let Some(active) = self.active.remove(id) {
                active.entity.set_removed(RemovalReason::Discarded);
            }
        }
        ids.len()
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
        }
    }

    /// Points every summoned shadow of one hunter at `target`.
    pub fn focus(&self, owner: Uuid, target: &SharedEntity) {
        for active in self.active.values().filter(|active| active.owner == owner) {
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
        self.active
            .values()
            .filter(|active| {
                let shadow_world = active.entity.level().map(|world| world.key.clone());
                owner_world(active.owner).is_some_and(|world| Some(world) != shadow_world)
            })
            .map(|active| (active.owner, active.shadow_id))
            .collect()
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
        if let Some(active) = self.active.remove(&entity_id) {
            active.entity.set_removed(RemovalReason::Discarded);
        }
        true
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
            },
        );
        Ok(())
    }
}

/// Records what a dying mob was: its nameplate, base attributes and gear.
fn capture_origin(entity: &dyn LivingEntity, label: Option<&MobLabel>) -> ShadowOrigin {
    let title = label.map_or_else(
        || mob_name(&entity.entity_type().key.path).to_owned(),
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
