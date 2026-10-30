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

use crate::dungeon::mobs::{MobLabel, MobRarity, clear_health_line};
use crate::dungeon::skills::{BROOD_COOLDOWN_TICKS, hatch_shadow};
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
    brood: Option<ShadowBrood>,
}

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
            if let Some(brood) = &active.brood {
                for larva in &brood.larvae {
                    if let Some(mob) = larva.as_mob() {
                        mob.set_allied_mode(mode);
                    }
                }
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
        let mut active = self.active.remove(&entity_uuid)?;
        if let Some(brood) = &mut active.brood {
            brood.discard();
        }
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
    use steel_core::entity::{AlliedMode, SharedEntity};
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use uuid::Uuid;

    use super::{ActiveShadow, ShadowBrood, ShadowRuntime, capture_origin};
    use crate::dungeon::generator::Theme;
    use crate::dungeon::mobs::{MobClass, MobLabel};

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
        let restored =
            serde_json::from_str::<crate::shadow::ShadowOrigin>(&data).expect("deserialize origin");
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
}
