//! The boss guarding a dungeon's final room.
//!
//! The body is a vanilla zombie: Steel has no boss-type mob yet. What makes it
//! an encounter is this module's rank data (stats, gear, abilities) plus the
//! fight state that drives the abilities and the vanilla boss bar.

use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{LivingEntity, MobEffectInstance, SharedEntity};
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::player::boss_bar::{BossBarColor, BossBarOverlay, BossBarProperties};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_types, vanilla_items, vanilla_mob_effects,
    vanilla_particle_types,
};
use steel_utils::Identifier;
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use super::RED_GATE_SCALE;
use crate::rank::HunterRank;

/// Ticks a freshly spawned boss waits before its first ability.
const OPENING_DELAY_TICKS: u64 = 40;
/// Cooldown of the ground slam.
const SLAM_COOLDOWN_TICKS: u64 = 100;
/// Cooldown of the dread roar.
const DREAD_ROAR_COOLDOWN_TICKS: u64 = 200;
/// Cooldown of the war cry.
const WARCRY_COOLDOWN_TICKS: u64 = 300;
/// Reach of the ground slam.
const SLAM_RANGE: f64 = 4.5;
/// Reach of the dread roar.
const DREAD_ROAR_RANGE: f64 = 16.0;
/// Fraction of the boss's attack damage the ground slam deals.
const SLAM_DAMAGE_SCALE: f64 = 0.6;
/// Horizontal knockback of the ground slam.
const SLAM_KNOCKBACK: f64 = 0.9;
/// Upward lift of the ground slam.
const SLAM_LIFT: f64 = 0.45;
/// Duration of the dread roar's debuffs.
const DREAD_ROAR_EFFECT_TICKS: i32 = 120;
/// Duration of the war cry's buffs.
const WARCRY_EFFECT_TICKS: i32 = 160;
/// Ticks between idle particles so the boss reads as "charged".
const AURA_INTERVAL_TICKS: u64 = 20;
/// Target search range of every boss, far wider than a vanilla zombie's.
const BOSS_FOLLOW_RANGE: f64 = 48.0;

/// Weapon and armor tier of a boss body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BossGear {
    /// Lowest ranks.
    Iron,
    /// Middle ranks.
    Diamond,
    /// Highest ranks.
    Netherite,
}

impl BossGear {
    /// Returns the gear tier of a boss at `rank`.
    #[must_use]
    pub const fn for_rank(rank: HunterRank) -> Self {
        match rank {
            HunterRank::E | HunterRank::D => Self::Iron,
            HunterRank::C | HunterRank::B => Self::Diamond,
            HunterRank::A | HunterRank::S => Self::Netherite,
        }
    }

    /// Returns the sword and full armor set of this tier, in slot order.
    fn equipment(self) -> [(EquipmentSlot, ItemStack); 5] {
        match self {
            Self::Iron => [
                (
                    EquipmentSlot::MainHand,
                    ItemStack::new(&vanilla_items::IRON_SWORD),
                ),
                (
                    EquipmentSlot::Head,
                    ItemStack::new(&vanilla_items::IRON_HELMET),
                ),
                (
                    EquipmentSlot::Chest,
                    ItemStack::new(&vanilla_items::IRON_CHESTPLATE),
                ),
                (
                    EquipmentSlot::Legs,
                    ItemStack::new(&vanilla_items::IRON_LEGGINGS),
                ),
                (
                    EquipmentSlot::Feet,
                    ItemStack::new(&vanilla_items::IRON_BOOTS),
                ),
            ],
            Self::Diamond => [
                (
                    EquipmentSlot::MainHand,
                    ItemStack::new(&vanilla_items::DIAMOND_SWORD),
                ),
                (
                    EquipmentSlot::Head,
                    ItemStack::new(&vanilla_items::DIAMOND_HELMET),
                ),
                (
                    EquipmentSlot::Chest,
                    ItemStack::new(&vanilla_items::DIAMOND_CHESTPLATE),
                ),
                (
                    EquipmentSlot::Legs,
                    ItemStack::new(&vanilla_items::DIAMOND_LEGGINGS),
                ),
                (
                    EquipmentSlot::Feet,
                    ItemStack::new(&vanilla_items::DIAMOND_BOOTS),
                ),
            ],
            Self::Netherite => [
                (
                    EquipmentSlot::MainHand,
                    ItemStack::new(&vanilla_items::NETHERITE_SWORD),
                ),
                (
                    EquipmentSlot::Head,
                    ItemStack::new(&vanilla_items::NETHERITE_HELMET),
                ),
                (
                    EquipmentSlot::Chest,
                    ItemStack::new(&vanilla_items::NETHERITE_CHESTPLATE),
                ),
                (
                    EquipmentSlot::Legs,
                    ItemStack::new(&vanilla_items::NETHERITE_LEGGINGS),
                ),
                (
                    EquipmentSlot::Feet,
                    ItemStack::new(&vanilla_items::NETHERITE_BOOTS),
                ),
            ],
        }
    }
}

/// Abilities a boss of one rank can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BossAbilitySet {
    /// Ground-breaking area attack with knockback.
    pub slam: bool,
    /// Ranged roar that slows and weakens the hunter.
    pub dread_roar: bool,
    /// Self-buff that turns the boss into a far deadlier melee threat.
    pub warcry: bool,
}

impl BossAbilitySet {
    /// Returns the abilities a boss at `rank` can use.
    ///
    /// Higher ranks get more tools on top of the slam, so a hunter meeting an
    /// S gate faces a different fight than one in an E gate.
    #[must_use]
    pub const fn for_rank(rank: HunterRank) -> Self {
        match rank {
            HunterRank::E | HunterRank::D => Self {
                slam: true,
                dread_roar: false,
                warcry: false,
            },
            HunterRank::C | HunterRank::B => Self {
                slam: true,
                dread_roar: true,
                warcry: false,
            },
            HunterRank::A | HunterRank::S => Self {
                slam: true,
                dread_roar: true,
                warcry: true,
            },
        }
    }
}

/// Rank-scaled parameters of the boss guarding a dungeon's final room.
///
/// Red gates scale health, damage, and reward only: their rank already carries
/// the promotion, so gear, armor, and abilities follow the promoted rank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DungeonBoss {
    /// Visual scale applied to the boss body.
    pub scale: f64,
    /// Multiplier applied to the boss body's vanilla maximum health.
    pub health_multiplier: f64,
    /// Multiplier applied to the boss body's vanilla attack damage.
    pub damage_multiplier: f64,
    /// Armor added on top of the equipped pieces.
    pub armor: f64,
    /// Armor toughness given to the boss.
    pub armor_toughness: f64,
    /// Fraction of incoming knockback the boss ignores.
    pub knockback_resistance: f64,
    /// Knockback the boss adds to its melee hits.
    pub attack_knockback: f64,
    /// Sword and armor the boss spawns with.
    pub gear: BossGear,
    /// Abilities the boss may use during the fight.
    pub abilities: BossAbilitySet,
}

impl DungeonBoss {
    /// Writes this boss's stats and gear onto its spawned body.
    ///
    /// The caller still has to refresh the hitbox, because the scale changed
    /// after the entity was spawned at its vanilla size.
    pub fn apply_to(&self, living: &dyn LivingEntity) {
        let max_health = living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MAX_HEALTH);
        let damage = living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::ATTACK_DAMAGE);
        let armor = living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::ARMOR);
        living.set_base_attribute_value(
            vanilla_attributes::MAX_HEALTH,
            max_health * self.health_multiplier,
        );
        living.set_base_attribute_value(
            vanilla_attributes::ATTACK_DAMAGE,
            damage * self.damage_multiplier,
        );
        living.set_base_attribute_value(vanilla_attributes::ARMOR, armor + self.armor);
        living.set_base_attribute_value(vanilla_attributes::ARMOR_TOUGHNESS, self.armor_toughness);
        living.set_base_attribute_value(
            vanilla_attributes::KNOCKBACK_RESISTANCE,
            self.knockback_resistance,
        );
        living
            .set_base_attribute_value(vanilla_attributes::ATTACK_KNOCKBACK, self.attack_knockback);
        living.set_base_attribute_value(vanilla_attributes::FOLLOW_RANGE, BOSS_FOLLOW_RANGE);
        living.set_base_attribute_value(vanilla_attributes::SCALE, self.scale);
        living.set_health((max_health * self.health_multiplier) as f32);
        for (slot, item) in self.gear.equipment() {
            living.with_equipment_slot_mut(slot, &mut |stack| *stack = item.clone());
        }
    }
}

/// Returns the boss of a gate at `rank`, with red gates scaled up.
#[must_use]
pub fn boss_for_gate(rank: HunterRank, red: bool) -> DungeonBoss {
    let (scale, health_multiplier, damage_multiplier, armor, toughness, resistance, knockback) =
        match rank {
            HunterRank::E => (1.4, 8.0, 2.0, 2.0, 0.0, 0.25, 0.5),
            HunterRank::D => (1.5, 10.0, 2.4, 3.0, 0.0, 0.3, 0.6),
            HunterRank::C => (1.6, 14.0, 2.8, 4.0, 2.0, 0.4, 0.8),
            HunterRank::B => (1.7, 18.0, 3.4, 5.0, 4.0, 0.5, 1.0),
            HunterRank::A => (1.85, 24.0, 4.0, 6.0, 6.0, 0.6, 1.2),
            HunterRank::S => (2.0, 32.0, 5.0, 8.0, 8.0, 0.7, 1.5),
        };
    let multiplier = if red { RED_GATE_SCALE } else { 1.0 };
    DungeonBoss {
        scale,
        health_multiplier: health_multiplier * multiplier,
        damage_multiplier: damage_multiplier * multiplier,
        armor,
        armor_toughness: toughness,
        knockback_resistance: resistance,
        attack_knockback: knockback,
        gear: BossGear::for_rank(rank),
        abilities: BossAbilitySet::for_rank(rank),
    }
}

/// Returns the boss bar id used for the boss with `entity_id`.
///
/// Entity ids are unique for the lifetime of the server, so the id only has to
/// stay stable for one fight.
const fn boss_bar_id(entity_id: i32) -> Uuid {
    Uuid::from_u128(entity_id as u32 as u128)
}

/// Returns a boss body's remaining health as a `0.0..=1.0` fraction.
fn health_fraction(entity: &SharedEntity) -> Option<f32> {
    let living = entity.as_living_entity()?;
    let max_health = living.get_max_health();
    (max_health > 0.0).then(|| (living.get_health() / max_health).clamp(0.0, 1.0))
}

/// One ability a boss may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BossAbility {
    Slam,
    DreadRoar,
    WarCry,
}

/// An ability selected under the dungeon lock and executed after releasing it.
#[derive(Debug, Clone)]
pub struct BossAbilityAction {
    world: Identifier,
    boss: i32,
    viewer: Uuid,
    targets: Vec<i32>,
    ability: BossAbility,
}

impl BossAbilityAction {
    /// Resolves still-live bodies before damage can invoke gameplay/death hooks.
    pub fn apply(&self, server: &Server) {
        let Some(world) = server.worlds.get(&self.world) else {
            return;
        };
        let Some(boss) = world.get_entity_by_id(self.boss) else {
            return;
        };
        if boss
            .as_living_entity()
            .is_none_or(LivingEntity::is_dead_or_dying)
        {
            return;
        }
        let Some(viewer) = world.players.get_by_uuid(&self.viewer) else {
            return;
        };
        let targets = self
            .targets
            .iter()
            .filter_map(|id| world.get_entity_by_id(*id))
            .filter(|target| {
                target
                    .as_living_entity()
                    .is_some_and(|living| !living.is_dead_or_dying())
            })
            .collect::<Vec<_>>();
        BossFight::use_ability(self.ability, world, &boss, &viewer, &targets);
    }
}

/// Live state of one boss fight: ability cooldowns and the boss bar.
#[derive(Debug)]
pub struct BossFight {
    /// Entity id of the boss body that owns this fight.
    boss_id: i32,
    abilities: BossAbilitySet,
    /// Server tick from which each ability may fire again.
    slam_ready: u64,
    dread_roar_ready: u64,
    warcry_ready: u64,
    /// Progress last sent to the boss bar, so health updates are not spam.
    shown_progress: Option<f32>,
}

impl BossFight {
    /// Starts the fight of the boss body `boss_id`, which appeared at `tick`.
    #[must_use]
    pub const fn new(abilities: BossAbilitySet, boss_id: i32, tick: u64) -> Self {
        Self {
            boss_id,
            abilities,
            slam_ready: tick + OPENING_DELAY_TICKS,
            dread_roar_ready: tick + OPENING_DELAY_TICKS,
            warcry_ready: tick + OPENING_DELAY_TICKS,
            shown_progress: None,
        }
    }

    /// Announces the boss: boss bar, title, and a spawn roar.
    pub fn announce(&mut self, boss: &SharedEntity, player: &Player, name: &TextComponent) {
        let progress = health_fraction(boss).unwrap_or(1.0);
        self.shown_progress = Some(progress);
        player.show_boss_bar(
            boss_bar_id(self.boss_id),
            name.clone(),
            progress,
            BossBarColor::Red,
            BossBarOverlay::Progress,
            BossBarProperties {
                darken_screen: true,
                play_music: false,
                create_world_fog: false,
            },
        );
        player.send_title(name.clone().color(Color::DarkRed).bold(true));
        player.send_subtitle(
            TextComponent::plain("The boss of this gate has appeared").color(Color::Red),
        );
        player.send_title_times(10, 60, 20);
        boss.play_sound(&sound_events::ENTITY_WITHER_SPAWN, 1.0, 0.8);
    }

    /// Updates the HUD and selects an ability without invoking damage hooks.
    pub fn tick(
        &mut self,
        world: &Arc<World>,
        player: &Player,
        members: &[Uuid],
        tick: u64,
    ) -> Option<BossAbilityAction> {
        let boss = world.get_entity_by_id(self.boss_id)?;
        let living = boss.as_living_entity()?;
        if let Some(progress) = health_fraction(&boss)
            && self.shown_progress != Some(progress)
        {
            self.shown_progress = Some(progress);
            player.update_boss_bar_progress(boss_bar_id(self.boss_id), progress);
        }
        if living.is_dead_or_dying() {
            return None;
        }
        if tick.is_multiple_of(AURA_INTERVAL_TICKS) {
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::SMOKE),
                boss.position(),
                3,
                DVec3::new(0.4, 0.8, 0.4),
                0.01,
            );
        }
        let targets = world.get_entities_in_aabb_matching(
            &boss.bounding_box().inflate(BOSS_FOLLOW_RANGE),
            |entity| {
                if entity
                    .as_living_entity()
                    .is_none_or(LivingEntity::is_dead_or_dying)
                {
                    return false;
                }
                if members.contains(&entity.uuid()) {
                    return true;
                }
                let Some(mob) = entity.as_mob() else {
                    return false;
                };
                mob.allied_owner()
                    .is_some_and(|owner| members.contains(&owner))
            },
        );
        let distance = targets
            .iter()
            .map(|target| boss.position().distance(target.position()))
            .min_by(f64::total_cmp)?;
        let ability = self.choose_ability(tick, distance)?;
        Some(BossAbilityAction {
            world: world.key.clone(),
            boss: self.boss_id,
            viewer: player.gameprofile.id(),
            targets: targets.iter().map(|target| target.id()).collect(),
            ability,
        })
    }

    /// Clears the boss HUD and congratulates the hunter that killed the boss.
    pub fn finish(&self, player: &Player) {
        player.hide_boss_bar(boss_bar_id(self.boss_id));
        player.send_title(
            TextComponent::plain("GATE CLEARED")
                .color(Color::Green)
                .bold(true),
        );
        player.send_subtitle(TextComponent::plain("The exit is open").color(Color::White));
        player.send_title_times(10, 50, 20);
    }

    /// Clears the boss bar of a fight that ended without a kill, such as when
    /// the hunter died or ran out of time.
    pub fn abandon(&self, player: &Player) {
        player.hide_boss_bar(boss_bar_id(self.boss_id));
    }

    /// Picks the ability to use at `tick`, arming its cooldown.
    fn choose_ability(&mut self, tick: u64, distance: f64) -> Option<BossAbility> {
        if self.abilities.slam && tick >= self.slam_ready && distance <= SLAM_RANGE {
            self.slam_ready = tick + SLAM_COOLDOWN_TICKS;
            return Some(BossAbility::Slam);
        }
        if self.abilities.warcry && tick >= self.warcry_ready && distance <= DREAD_ROAR_RANGE {
            self.warcry_ready = tick + WARCRY_COOLDOWN_TICKS;
            return Some(BossAbility::WarCry);
        }
        if self.abilities.dread_roar
            && tick >= self.dread_roar_ready
            && distance <= DREAD_ROAR_RANGE
        {
            self.dread_roar_ready = tick + DREAD_ROAR_COOLDOWN_TICKS;
            return Some(BossAbility::DreadRoar);
        }
        None
    }

    fn use_ability(
        ability: BossAbility,
        world: &Arc<World>,
        boss: &SharedEntity,
        player: &Player,
        targets: &[SharedEntity],
    ) {
        match ability {
            BossAbility::Slam => Self::use_slam(world, boss, player, targets),
            BossAbility::DreadRoar => Self::use_dread_roar(world, boss, player, targets),
            BossAbility::WarCry => {
                let ground = boss.position();
                world.send_particles(
                    ParticleData::simple(&vanilla_particle_types::FLAME),
                    ground,
                    50,
                    DVec3::new(1.5, 2.0, 1.5),
                    0.1,
                );
                boss.play_sound(&sound_events::ENTITY_RAVAGER_ROAR, 1.5, 0.7);
                player.send_action_bar(
                    TextComponent::plain("War Cry! The boss grows stronger").color(Color::Gold),
                );
                if let Some(living) = boss.as_living_entity() {
                    let _ = living.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::STRENGTH,
                        WARCRY_EFFECT_TICKS,
                        1,
                    ));
                    let _ = living.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::RESISTANCE,
                        WARCRY_EFFECT_TICKS,
                        0,
                    ));
                }
            }
        }
    }

    fn use_slam(
        world: &Arc<World>,
        boss: &SharedEntity,
        player: &Player,
        targets: &[SharedEntity],
    ) {
        let ground = boss.position();
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::EXPLOSION_EMITTER),
            ground,
            1,
            DVec3::ZERO,
            0.0,
        );
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::LARGE_SMOKE),
            ground,
            40,
            DVec3::new(2.0, 0.4, 2.0),
            0.05,
        );
        boss.play_sound(&sound_events::ENTITY_GENERIC_EXPLODE, 1.5, 0.6);
        player.send_action_bar(TextComponent::plain("Ground Slam!").color(Color::Red));
        let damage = boss.as_living_entity().map_or(0.0, |living| {
            living
                .attributes()
                .lock()
                .required_value(vanilla_attributes::ATTACK_DAMAGE)
        }) * SLAM_DAMAGE_SCALE;
        for target in targets
            .iter()
            .filter(|target| target.position().distance(ground) <= SLAM_RANGE)
        {
            if damage > 0.0 {
                target.hurt(
                    world,
                    &DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(boss)),
                    damage as f32,
                );
            }
            let offset = target.position() - ground;
            let horizontal = DVec3::new(offset.x, 0.0, offset.z);
            if horizontal.length() > 0.01 {
                target.push_impulse(
                    horizontal.normalize() * SLAM_KNOCKBACK + DVec3::new(0.0, SLAM_LIFT, 0.0),
                );
            }
        }
    }

    fn use_dread_roar(
        world: &Arc<World>,
        boss: &SharedEntity,
        player: &Player,
        targets: &[SharedEntity],
    ) {
        let ground = boss.position();
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SOUL),
            ground,
            60,
            DVec3::new(6.0, 2.0, 6.0),
            0.02,
        );
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SMOKE),
            ground,
            30,
            DVec3::new(5.0, 1.0, 5.0),
            0.01,
        );
        boss.play_sound(&sound_events::ENTITY_WARDEN_ROAR, 1.2, 0.9);
        player.send_action_bar(
            TextComponent::plain("Dread Roar! Slowed and weakened").color(Color::DarkRed),
        );
        for target in targets
            .iter()
            .filter(|target| target.position().distance(ground) <= DREAD_ROAR_RANGE)
        {
            let Some(living) = target.as_living_entity() else {
                continue;
            };
            let _ = living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::SLOWNESS,
                DREAD_ROAR_EFFECT_TICKS,
                1,
            ));
            let _ = living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::WEAKNESS,
                DREAD_ROAR_EFFECT_TICKS,
                0,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BossAbility, BossAbilitySet, BossFight, BossGear, boss_for_gate};
    use crate::rank::HunterRank;

    #[test]
    fn bosses_grow_with_rank_and_red_gates() {
        let e_boss = boss_for_gate(HunterRank::E, false);
        assert_eq!(e_boss.health_multiplier, 8.0);
        assert_eq!(e_boss.damage_multiplier, 2.0);
        assert_eq!(e_boss.scale, 1.4);
        assert_eq!(e_boss.gear, BossGear::Iron);
        assert!(!e_boss.abilities.dread_roar);

        let red_s_boss = boss_for_gate(HunterRank::S, true);
        assert_eq!(red_s_boss.scale, 2.0);
        assert_eq!(red_s_boss.health_multiplier, 48.0);
        assert_eq!(red_s_boss.damage_multiplier, 7.5);
        assert_eq!(red_s_boss.gear, BossGear::Netherite);
        assert!(red_s_boss.abilities.warcry);
    }

    #[test]
    fn gear_follows_the_rank() {
        assert_eq!(BossGear::for_rank(HunterRank::D), BossGear::Iron);
        assert_eq!(BossGear::for_rank(HunterRank::C), BossGear::Diamond);
        assert_eq!(BossGear::for_rank(HunterRank::A), BossGear::Netherite);
    }

    #[test]
    fn higher_ranks_gain_more_abilities() {
        let e = BossAbilitySet::for_rank(HunterRank::E);
        assert!(e.slam && !e.dread_roar && !e.warcry);
        let c = BossAbilitySet::for_rank(HunterRank::C);
        assert!(c.slam && c.dread_roar && !c.warcry);
        let s = BossAbilitySet::for_rank(HunterRank::S);
        assert!(s.slam && s.dread_roar && s.warcry);
    }

    #[test]
    fn the_slam_waits_for_its_opening_delay_and_range() {
        let slam_only = BossAbilitySet::for_rank(HunterRank::E);
        let mut fight = BossFight::new(slam_only, 7, 0);
        assert_eq!(fight.choose_ability(0, 1.0), None);
        assert_eq!(fight.choose_ability(39, 1.0), None);
        assert_eq!(fight.choose_ability(40, 1.0), Some(BossAbility::Slam));
        // The cooldown holds until it expires, even in range.
        assert_eq!(fight.choose_ability(100, 1.0), None);
        assert_eq!(fight.choose_ability(140, 1.0), Some(BossAbility::Slam));
        // Out of reach the slam stays ready for the next opportunity.
        assert_eq!(fight.choose_ability(240, 99.0), None);
        assert_eq!(fight.choose_ability(241, 1.0), Some(BossAbility::Slam));
    }

    #[test]
    fn a_distant_hunter_is_never_hit_by_abilities() {
        let all = BossAbilitySet::for_rank(HunterRank::S);
        let mut fight = BossFight::new(all, 7, 0);
        assert_eq!(fight.choose_ability(40, 99.0), None);
        // Once in reach the war cry comes first, then the roar.
        assert_eq!(fight.choose_ability(41, 8.0), Some(BossAbility::WarCry));
        assert_eq!(fight.choose_ability(42, 8.0), Some(BossAbility::DreadRoar));
    }

    #[test]
    fn a_boss_without_abilities_never_acts() {
        let quiet = BossAbilitySet {
            slam: false,
            dread_roar: false,
            warcry: false,
        };
        let mut fight = BossFight::new(quiet, 7, 0);
        assert_eq!(fight.choose_ability(1_000, 1.0), None);
    }
}
