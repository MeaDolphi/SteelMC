//! The bosses guarding a dungeon's final room.
//!
//! Every body is a vanilla zombie: Steel has no other hostile mob yet. What
//! makes each boss a distinct encounter is its archetype (gear, stats and an
//! ability kit) plus the fight state that drives the abilities and the vanilla
//! boss bar.
//!
//! There are no spell projectiles either, so ranged abilities are drawn with
//! particles and resolved directly: a bolt is a particle line that hits at
//! once, and a meteor marks the ground first and lands a couple of seconds
//! later, giving the hunter time to step out.

use std::f64::consts::FRAC_PI_4;
use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{LivingEntity, MobEffectInstance, SharedEntity};
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::player::boss_bar::{BossBarColor, BossBarOverlay, BossBarProperties};
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::DyedItemColor;
use steel_registry::data_components::vanilla_components::DYED_COLOR;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::particle_type::{ParticleData, ParticleType};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_types, vanilla_items, vanilla_mob_effects,
    vanilla_particle_types,
};
use steel_utils::{BlockPos, Identifier};
use text_components::Modifier as _;
use text_components::TextComponent;
use text_components::format::Color;
use uuid::Uuid;

use super::RED_GATE_SCALE;
use crate::rank::HunterRank;

/// Ticks a freshly spawned boss waits before its first ability.
const OPENING_DELAY_TICKS: u64 = 40;
/// Ticks between a meteor's warning and its impact.
const METEOR_DELAY_TICKS: u64 = 40;
/// Radius of one meteor impact.
const METEOR_RADIUS: f64 = 3.0;
/// Most meteors one cast drops, one per target.
const METEOR_COUNT: usize = 3;
/// Horizontal knockback of the ground slam.
const SLAM_KNOCKBACK: f64 = 0.9;
/// Upward lift of the ground slam.
const SLAM_LIFT: f64 = 0.45;
/// Blocks the archmage blinks away from the nearest hunter.
const BLINK_DISTANCE: f64 = 9.0;
/// Ticks between idle particles so the boss reads as "charged".
const AURA_INTERVAL_TICKS: u64 = 20;
/// Target search range of every boss, far wider than a vanilla zombie's.
const BOSS_FOLLOW_RANGE: f64 = 48.0;
/// Attack damage of a vanilla zombie, the base every boss multiplier scales.
const ZOMBIE_ATTACK_DAMAGE: f64 = 3.0;
/// Leather dye of the archmage's robes.
const ARCHMAGE_ROBE: i32 = 0x5B_2A86;
/// Leather dye of the assassin's garb.
const ASSASSIN_GARB: i32 = 0x1D_1D21;

/// The archetype of a dungeon boss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BossKind {
    /// A heavy melee brute that slams, roars and enrages.
    Warlord,
    /// A frail caster that keeps its distance and rains spells.
    Archmage,
    /// A fast killer that steps through shadows and poisons.
    Assassin,
}

/// Every archetype, in roll order.
pub const BOSS_KINDS: [BossKind; 3] = [BossKind::Warlord, BossKind::Archmage, BossKind::Assassin];

impl BossKind {
    /// Returns the archetype a dungeon seed rolls.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        BOSS_KINDS[usize::try_from((seed >> 40) % BOSS_KINDS.len() as u64).unwrap_or(0)]
    }

    /// Returns the Russian name of the archetype.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Warlord => "Вождь-берсерк",
            Self::Archmage => "Архимаг",
            Self::Assassin => "Теневой убийца",
        }
    }

    /// Returns the abilities a boss of this kind uses at `rank`, by priority.
    ///
    /// Higher ranks unlock more of the kit, so an S gate fights differently
    /// from an E gate of the same archetype.
    #[must_use]
    pub const fn abilities(self, rank: HunterRank) -> &'static [BossAbility] {
        use BossAbility::{
            ArcaneBolt, Blink, DreadRoar, FrostNova, Meteor, ShadowStep, Slam, SmokeBomb,
            VenomBlades, WarCry,
        };
        let tier = match rank {
            HunterRank::E | HunterRank::D => 0,
            HunterRank::C | HunterRank::B => 1,
            HunterRank::A | HunterRank::S => 2,
        };
        match (self, tier) {
            (Self::Warlord, 0) => &[Slam, DreadRoar],
            (Self::Warlord, _) => &[Slam, WarCry, DreadRoar],
            (Self::Archmage, 0) => &[Blink, ArcaneBolt],
            (Self::Archmage, 1) => &[Blink, FrostNova, ArcaneBolt],
            (Self::Archmage, _) => &[Blink, FrostNova, Meteor, ArcaneBolt],
            (Self::Assassin, 0) => &[VenomBlades, ShadowStep],
            (Self::Assassin, _) => &[VenomBlades, SmokeBomb, ShadowStep],
        }
    }

    /// Returns health, melee damage, movement speed and size relative to the
    /// rank's warlord.
    const fn proportions(self) -> (f64, f64, f64, f64) {
        match self {
            Self::Warlord => (1.0, 1.0, 1.0, 1.0),
            // The archmage fights with spells and avoids melee.
            Self::Archmage => (0.75, 0.6, 0.7, 0.85),
            Self::Assassin => (0.6, 1.1, 1.35, 0.75),
        }
    }
}

/// One ability a boss may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BossAbility {
    /// Ground-breaking area attack with knockback.
    Slam,
    /// Ranged roar that slows and weakens.
    DreadRoar,
    /// Self-buff that turns the boss into a deadlier melee threat.
    WarCry,
    /// An instant magic bolt at the nearest hunter.
    ArcaneBolt,
    /// A burst of cold that damages and freezes nearby hunters.
    FrostNova,
    /// Marked impacts that land under every hunter after a delay.
    Meteor,
    /// A teleport away from a hunter who came too close.
    Blink,
    /// A teleport behind the nearest hunter followed by a stab.
    ShadowStep,
    /// A poisoned sweep around the boss.
    VenomBlades,
    /// A smoke cloud that blinds hunters and hides the boss.
    SmokeBomb,
}

impl BossAbility {
    /// Returns the ticks before the ability can be used again.
    #[must_use]
    pub const fn cooldown(self) -> u64 {
        match self {
            Self::ArcaneBolt => 50,
            Self::VenomBlades => 80,
            Self::Slam => 100,
            Self::Blink => 120,
            Self::ShadowStep => 140,
            Self::FrostNova => 160,
            Self::DreadRoar => 200,
            Self::Meteor => 220,
            Self::SmokeBomb => 260,
            Self::WarCry => 300,
        }
    }

    /// Returns the distance band to the nearest hunter that allows the ability.
    #[must_use]
    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::Slam => (0.0, 4.5),
            Self::VenomBlades => (0.0, 3.5),
            Self::Blink => (0.0, 4.0),
            Self::FrostNova => (0.0, 6.0),
            Self::SmokeBomb => (0.0, 8.0),
            Self::ShadowStep => (4.0, 16.0),
            Self::DreadRoar | Self::WarCry => (0.0, 16.0),
            Self::Meteor => (0.0, 20.0),
            Self::ArcaneBolt => (0.0, 24.0),
        }
    }

    /// Returns the ability's damage as a fraction of the boss's power.
    #[must_use]
    pub const fn damage_scale(self) -> f64 {
        match self {
            Self::Slam => 0.6,
            Self::ArcaneBolt => 0.7,
            Self::FrostNova => 0.5,
            Self::Meteor => 1.2,
            Self::ShadowStep => 1.0,
            Self::VenomBlades => 0.4,
            Self::DreadRoar | Self::WarCry | Self::Blink | Self::SmokeBomb => 0.0,
        }
    }

    /// Returns the Russian line shown to hunters when the ability is used.
    const fn announcement(self) -> (&'static str, Color) {
        match self {
            Self::Slam => ("Удар о землю!", Color::Red),
            Self::DreadRoar => ("Устрашающий рёв: вы замедлены и ослаблены", Color::DarkRed),
            Self::WarCry => ("Боевой клич: босс становится сильнее", Color::Gold),
            Self::ArcaneBolt => ("Чародейская стрела!", Color::LightPurple),
            Self::FrostNova => ("Ледяная вспышка!", Color::Aqua),
            Self::Meteor => ("Метеоры! Уходите с отмеченной земли", Color::Gold),
            Self::Blink => ("Архимаг переместился", Color::DarkPurple),
            Self::ShadowStep => ("Шаг тени!", Color::DarkGray),
            Self::VenomBlades => ("Отравленные клинки!", Color::DarkGreen),
            Self::SmokeBomb => ("Дымовая завеса!", Color::Gray),
        }
    }
}

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

    /// Returns the sword of this tier.
    #[must_use]
    pub fn sword(self) -> ItemRef {
        match self {
            Self::Iron => &vanilla_items::IRON_SWORD,
            Self::Diamond => &vanilla_items::DIAMOND_SWORD,
            Self::Netherite => &vanilla_items::NETHERITE_SWORD,
        }
    }

    fn armor(self) -> [ItemRef; 4] {
        match self {
            Self::Iron => [
                &vanilla_items::IRON_HELMET,
                &vanilla_items::IRON_CHESTPLATE,
                &vanilla_items::IRON_LEGGINGS,
                &vanilla_items::IRON_BOOTS,
            ],
            Self::Diamond => [
                &vanilla_items::DIAMOND_HELMET,
                &vanilla_items::DIAMOND_CHESTPLATE,
                &vanilla_items::DIAMOND_LEGGINGS,
                &vanilla_items::DIAMOND_BOOTS,
            ],
            Self::Netherite => [
                &vanilla_items::NETHERITE_HELMET,
                &vanilla_items::NETHERITE_CHESTPLATE,
                &vanilla_items::NETHERITE_LEGGINGS,
                &vanilla_items::NETHERITE_BOOTS,
            ],
        }
    }
}

const ARMOR_SLOTS: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];

fn dyed_leather(color: i32) -> [ItemStack; 4] {
    [
        &vanilla_items::LEATHER_HELMET,
        &vanilla_items::LEATHER_CHESTPLATE,
        &vanilla_items::LEATHER_LEGGINGS,
        &vanilla_items::LEATHER_BOOTS,
    ]
    .map(|item| {
        let mut stack = ItemStack::new(item);
        stack.set(DYED_COLOR, DyedItemColor::new(color));
        stack
    })
}

/// Rank-scaled parameters of the boss guarding a dungeon's final room.
///
/// Red gates scale health, damage, and reward only: their rank already carries
/// the promotion, so gear, armor, and abilities follow the promoted rank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DungeonBoss {
    /// The boss's archetype.
    pub kind: BossKind,
    /// Visual scale applied to the boss body.
    pub scale: f64,
    /// Multiplier applied to the boss body's vanilla maximum health.
    pub health_multiplier: f64,
    /// Multiplier applied to the boss body's vanilla attack damage.
    pub damage_multiplier: f64,
    /// Multiplier applied to the boss body's vanilla movement speed.
    pub speed_multiplier: f64,
    /// Damage the boss's abilities scale from, independent of its melee.
    pub power: f64,
    /// Armor added on top of the equipped pieces.
    pub armor: f64,
    /// Armor toughness given to the boss.
    pub armor_toughness: f64,
    /// Fraction of incoming knockback the boss ignores.
    pub knockback_resistance: f64,
    /// Knockback the boss adds to its melee hits.
    pub attack_knockback: f64,
    /// Weapon and armor tier the boss spawns with.
    pub gear: BossGear,
    /// Abilities the boss may use during the fight, by priority.
    pub abilities: &'static [BossAbility],
}

impl DungeonBoss {
    /// Writes this boss's stats and gear onto its spawned body.
    ///
    /// The caller still has to refresh the hitbox, because the scale changed
    /// after the entity was spawned at its vanilla size.
    pub fn apply_to(&self, living: &dyn LivingEntity) {
        let (max_health, damage, armor, speed) = {
            let attributes = living.attributes().lock();
            (
                attributes.required_value(vanilla_attributes::MAX_HEALTH),
                attributes.required_value(vanilla_attributes::ATTACK_DAMAGE),
                attributes.required_value(vanilla_attributes::ARMOR),
                attributes.required_value(vanilla_attributes::MOVEMENT_SPEED),
            )
        };
        living.set_base_attribute_value(
            vanilla_attributes::MAX_HEALTH,
            max_health * self.health_multiplier,
        );
        living.set_base_attribute_value(
            vanilla_attributes::ATTACK_DAMAGE,
            damage * self.damage_multiplier,
        );
        living.set_base_attribute_value(
            vanilla_attributes::MOVEMENT_SPEED,
            speed * self.speed_multiplier,
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
        for (slot, item) in self.equipment() {
            living.with_equipment_slot_mut(slot, &mut |stack| *stack = item.clone());
        }
    }

    /// Returns what the boss holds and wears.
    fn equipment(&self) -> Vec<(EquipmentSlot, ItemStack)> {
        let (weapon, armor) = match self.kind {
            BossKind::Warlord => (
                ItemStack::new(self.gear.sword()),
                self.gear.armor().map(ItemStack::new),
            ),
            BossKind::Archmage => (
                ItemStack::new(&vanilla_items::BLAZE_ROD),
                dyed_leather(ARCHMAGE_ROBE),
            ),
            BossKind::Assassin => (
                ItemStack::new(self.gear.sword()),
                dyed_leather(ASSASSIN_GARB),
            ),
        };
        let mut equipment = vec![(EquipmentSlot::MainHand, weapon)];
        equipment.extend(ARMOR_SLOTS.into_iter().zip(armor));
        equipment
    }
}

/// Returns the boss of a gate at `rank`, with red gates scaled up.
#[must_use]
pub fn boss_for_gate(kind: BossKind, rank: HunterRank, red: bool) -> DungeonBoss {
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
    let (health, damage, speed, size) = kind.proportions();
    DungeonBoss {
        kind,
        scale: (scale * size).max(1.1),
        health_multiplier: health_multiplier * multiplier * health,
        damage_multiplier: damage_multiplier * multiplier * damage,
        speed_multiplier: speed,
        power: ZOMBIE_ATTACK_DAMAGE * damage_multiplier * multiplier,
        armor,
        armor_toughness: toughness,
        knockback_resistance: resistance,
        attack_knockback: knockback,
        gear: BossGear::for_rank(rank),
        abilities: kind.abilities(rank),
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

/// An ability selected under the dungeon lock and executed after releasing it.
#[derive(Debug, Clone)]
pub struct BossAbilityAction {
    world: Identifier,
    boss: i32,
    viewer: Uuid,
    targets: Vec<i32>,
    ability: BossAbility,
    /// Whether this resolves earlier meteor marks instead of a new cast.
    impact: bool,
    /// Ground points of a meteor cast or impact.
    points: Vec<DVec3>,
    power: f64,
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
        let cast = Cast {
            world,
            boss: &boss,
            viewer: &viewer,
            targets: &targets,
            power: self.power,
        };
        if self.impact {
            cast.meteor_impact(&self.points);
        } else {
            cast.run(self.ability, &self.points);
        }
    }
}

/// Live state of one boss fight: ability cooldowns, pending meteors, and the
/// boss bar.
#[derive(Debug)]
pub struct BossFight {
    /// Entity id of the boss body that owns this fight.
    boss_id: i32,
    abilities: &'static [BossAbility],
    /// Server tick from which each ability, by index, may fire again.
    ready: Vec<u64>,
    /// Damage the abilities scale from.
    power: f64,
    /// Marked meteor impacts and the tick they land.
    meteors: Vec<(DVec3, u64)>,
    /// Progress last sent to the boss bar, so health updates are not spam.
    shown_progress: Option<f32>,
}

impl BossFight {
    /// Starts the fight of the boss body `boss_id`, which appeared at `tick`.
    #[must_use]
    pub fn new(boss: &DungeonBoss, boss_id: i32, tick: u64) -> Self {
        Self {
            boss_id,
            abilities: boss.abilities,
            ready: vec![tick + OPENING_DELAY_TICKS; boss.abilities.len()],
            power: boss.power,
            meteors: Vec::new(),
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
        player.send_subtitle(TextComponent::plain("Хозяин врат пробудился").color(Color::Red));
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
        let (boss_id, power) = (self.boss_id, self.power);
        let action = |ability, impact, points| BossAbilityAction {
            world: world.key.clone(),
            boss: boss_id,
            viewer: player.gameprofile.id(),
            targets: targets.iter().map(|target| target.id()).collect(),
            ability,
            impact,
            points,
            power,
        };

        // Marked meteors land whether or not anyone is still in reach.
        let (landing, waiting): (Vec<_>, Vec<_>) =
            self.meteors.iter().partition(|(_, due)| *due <= tick);
        self.meteors = waiting;
        if !landing.is_empty() {
            let points = landing.into_iter().map(|(point, _)| point).collect();
            return Some(action(BossAbility::Meteor, true, points));
        }
        if tick.is_multiple_of(10) {
            for (point, _) in &self.meteors {
                mark_ground(world, *point);
            }
        }

        // A boss only acts on hunters it can see, so nothing reaches through walls.
        let eye = boss.position() + DVec3::new(0.0, EYE_HEIGHT, 0.0);
        let visible: Vec<&SharedEntity> = targets
            .iter()
            .filter(|target| line_of_sight(world, eye, eye_of(target)))
            .collect();
        let distance = visible
            .iter()
            .map(|target| boss.position().distance(target.position()))
            .min_by(f64::total_cmp)?;
        let ability = self.choose_ability(tick, distance)?;
        let points = if ability == BossAbility::Meteor {
            let points: Vec<DVec3> = visible
                .iter()
                .take(METEOR_COUNT)
                .map(|target| target.position())
                .collect();
            self.meteors.extend(
                points
                    .iter()
                    .map(|point| (*point, tick + METEOR_DELAY_TICKS)),
            );
            points
        } else {
            Vec::new()
        };
        Some(action(ability, false, points))
    }

    /// Clears the boss HUD and congratulates the hunter that killed the boss.
    pub fn finish(&self, player: &Player) {
        player.hide_boss_bar(boss_bar_id(self.boss_id));
        player.send_title(
            TextComponent::plain("ВРАТА ЗАКРЫТЫ")
                .color(Color::Green)
                .bold(true),
        );
        player.send_subtitle(TextComponent::plain("Выход открыт").color(Color::White));
        player.send_title_times(10, 50, 20);
    }

    /// Clears the boss bar of a fight that ended without a kill, such as when
    /// the hunter died or ran out of time.
    pub fn abandon(&self, player: &Player) {
        player.hide_boss_bar(boss_bar_id(self.boss_id));
    }

    /// Picks the first ready ability whose range covers `distance`, arming its
    /// cooldown.
    fn choose_ability(&mut self, tick: u64, distance: f64) -> Option<BossAbility> {
        for (index, ability) in self.abilities.iter().enumerate() {
            let (near, far) = ability.range();
            if tick >= self.ready[index] && distance >= near && distance <= far {
                self.ready[index] = tick + ability.cooldown();
                return Some(*ability);
            }
        }
        None
    }
}

/// Draws a meteor's warning ring on the ground.
fn mark_ground(world: &World, point: DVec3) {
    world.send_particles(
        ParticleData::simple(&vanilla_particle_types::FLAME),
        point + DVec3::new(0.0, 0.1, 0.0),
        16,
        DVec3::new(METEOR_RADIUS * 0.6, 0.0, METEOR_RADIUS * 0.6),
        0.0,
    );
}

/// Height above the feet that sight lines start and end at.
const EYE_HEIGHT: f64 = 1.5;
/// Distance between two sampled points of a sight line.
const SIGHT_STEP: f64 = 0.25;

fn eye_of(entity: &SharedEntity) -> DVec3 {
    entity.position() + DVec3::new(0.0, EYE_HEIGHT, 0.0)
}

/// Returns whether nothing but air lies on the straight line between two points.
///
/// The blocks holding the end points themselves are not checked, so an eye
/// pressed against a wall still sees what stands in front of it.
#[must_use]
pub fn line_of_sight(world: &World, from: DVec3, to: DVec3) -> bool {
    let start = BlockPos::from(from);
    let end = BlockPos::from(to);
    let steps = (from.distance(to) / SIGHT_STEP).ceil().max(1.0) as i32;
    (1..steps).all(|step| {
        let point = from.lerp(to, f64::from(step) / f64::from(steps));
        let block = BlockPos::from(point);
        block == start || block == end || world.get_block_state(block).is_air()
    })
}

/// Returns whether a body fits standing at `pos`.
fn standable(world: &World, pos: DVec3) -> bool {
    let feet = BlockPos::from(pos);
    world.get_block_state(feet).is_air()
        && world.get_block_state(feet.offset(0, 1, 0)).is_air()
        && !world.get_block_state(feet.offset(0, -1, 0)).is_air()
}

/// One resolved ability, with everything it acts on.
struct Cast<'a> {
    world: &'a Arc<World>,
    boss: &'a SharedEntity,
    viewer: &'a Player,
    targets: &'a [SharedEntity],
    power: f64,
}

impl Cast<'_> {
    fn run(&self, ability: BossAbility, points: &[DVec3]) {
        // Single-target abilities fizzle silently when every hunter hid
        // between the choice and the cast.
        if matches!(ability, BossAbility::ArcaneBolt | BossAbility::ShadowStep)
            && self.nearest().is_none()
        {
            return;
        }
        let (line, color) = ability.announcement();
        self.viewer
            .send_action_bar(TextComponent::plain(line).color(color));
        match ability {
            BossAbility::Slam => self.slam(),
            BossAbility::DreadRoar => self.dread_roar(),
            BossAbility::WarCry => self.war_cry(),
            BossAbility::ArcaneBolt => self.arcane_bolt(),
            BossAbility::FrostNova => self.frost_nova(),
            BossAbility::Meteor => self.meteor_warning(points),
            BossAbility::Blink => self.blink(),
            BossAbility::ShadowStep => self.shadow_step(),
            BossAbility::VenomBlades => self.venom_blades(),
            BossAbility::SmokeBomb => self.smoke_bomb(),
        }
    }

    fn particles(&self, particle: &'static ParticleType, at: DVec3, count: i32, spread: f64) {
        self.world.send_particles(
            ParticleData::simple(particle),
            at,
            count,
            DVec3::new(spread, spread * 0.5, spread),
            0.02,
        );
    }

    fn sound(&self, sound: SoundEventRef, pitch: f32) {
        self.boss.play_sound(sound, 1.4, pitch);
    }

    /// Returns the hunters within `range` of `center` that it can see.
    fn within(&self, center: DVec3, range: f64) -> impl Iterator<Item = &SharedEntity> {
        let source = center + DVec3::new(0.0, EYE_HEIGHT, 0.0);
        self.targets.iter().filter(move |target| {
            target.position().distance(center) <= range
                && line_of_sight(self.world, source, eye_of(target))
        })
    }

    /// Returns the nearest hunter the boss can see.
    fn nearest(&self) -> Option<&SharedEntity> {
        let origin = self.boss.position();
        self.within(origin, f64::INFINITY).min_by(|a, b| {
            a.position()
                .distance(origin)
                .total_cmp(&b.position().distance(origin))
        })
    }

    fn strike(&self, target: &SharedEntity, ability: BossAbility) {
        let damage = (self.power * ability.damage_scale()) as f32;
        if damage > 0.0 {
            target.hurt(
                self.world,
                &DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(self.boss)),
                damage,
            );
        }
    }

    fn spell(&self, target: &SharedEntity, ability: BossAbility) {
        let damage = (self.power * ability.damage_scale()) as f32;
        if damage > 0.0 {
            let source = DamageSource::environment(&vanilla_damage_types::MAGIC)
                .with_causing_entity(Arc::clone(self.boss));
            target.hurt(self.world, &source, damage);
        }
    }

    fn afflict(target: &SharedEntity, effect: MobEffectRef, ticks: i32, amplifier: i32) {
        if let Some(living) = target.as_living_entity() {
            let _ =
                living.add_mob_effect(MobEffectInstance::with_duration(effect, ticks, amplifier));
        }
    }

    fn slam(&self) {
        let ground = self.boss.position();
        self.particles(&vanilla_particle_types::EXPLOSION_EMITTER, ground, 1, 0.0);
        self.particles(&vanilla_particle_types::LARGE_SMOKE, ground, 40, 2.0);
        self.sound(&sound_events::ENTITY_GENERIC_EXPLODE, 0.6);
        let (_, range) = BossAbility::Slam.range();
        for target in self.within(ground, range) {
            self.strike(target, BossAbility::Slam);
            let offset = target.position() - ground;
            let horizontal = DVec3::new(offset.x, 0.0, offset.z);
            if horizontal.length() > 0.01 {
                target.push_impulse(
                    horizontal.normalize() * SLAM_KNOCKBACK + DVec3::new(0.0, SLAM_LIFT, 0.0),
                );
            }
        }
    }

    fn dread_roar(&self) {
        let ground = self.boss.position();
        self.particles(&vanilla_particle_types::SOUL, ground, 60, 6.0);
        self.sound(&sound_events::ENTITY_WARDEN_ROAR, 0.9);
        let (_, range) = BossAbility::DreadRoar.range();
        for target in self.within(ground, range) {
            Self::afflict(target, vanilla_mob_effects::SLOWNESS, 120, 1);
            Self::afflict(target, vanilla_mob_effects::WEAKNESS, 120, 0);
        }
    }

    fn war_cry(&self) {
        self.particles(
            &vanilla_particle_types::FLAME,
            self.boss.position(),
            50,
            1.5,
        );
        self.sound(&sound_events::ENTITY_RAVAGER_ROAR, 0.7);
        Self::afflict(self.boss, vanilla_mob_effects::STRENGTH, 160, 1);
        Self::afflict(self.boss, vanilla_mob_effects::RESISTANCE, 160, 0);
    }

    fn arcane_bolt(&self) {
        let Some(target) = self.nearest() else {
            return;
        };
        let from = self.boss.position() + DVec3::new(0.0, 1.6, 0.0);
        let to = target.position() + DVec3::new(0.0, 1.0, 0.0);
        let steps = (from.distance(to) * 2.0).ceil().max(1.0) as i32;
        for step in 0..=steps {
            let point = from.lerp(to, f64::from(step) / f64::from(steps));
            self.world.send_particles(
                ParticleData::simple(&vanilla_particle_types::END_ROD),
                point,
                1,
                DVec3::ZERO,
                0.0,
            );
        }
        self.sound(&sound_events::ENTITY_EVOKER_CAST_SPELL, 1.4);
        self.spell(target, BossAbility::ArcaneBolt);
    }

    fn frost_nova(&self) {
        let ground = self.boss.position();
        self.particles(&vanilla_particle_types::SNOWFLAKE, ground, 80, 4.0);
        self.sound(&sound_events::BLOCK_GLASS_BREAK, 0.6);
        let (_, range) = BossAbility::FrostNova.range();
        for target in self.within(ground, range) {
            self.spell(target, BossAbility::FrostNova);
            Self::afflict(target, vanilla_mob_effects::SLOWNESS, 60, 2);
        }
    }

    fn meteor_warning(&self, points: &[DVec3]) {
        for point in points {
            mark_ground(self.world, *point);
        }
        self.sound(&sound_events::ENTITY_BLAZE_SHOOT, 0.6);
    }

    fn meteor_impact(&self, points: &[DVec3]) {
        for point in points {
            self.particles(&vanilla_particle_types::EXPLOSION_EMITTER, *point, 1, 0.0);
            self.particles(&vanilla_particle_types::LAVA, *point, 20, 1.5);
            for target in self.within(*point, METEOR_RADIUS) {
                self.spell(target, BossAbility::Meteor);
            }
        }
        self.sound(&sound_events::ENTITY_GENERIC_EXPLODE, 0.8);
    }

    fn blink(&self) {
        let Some(threat) = self.nearest() else {
            return;
        };
        let origin = self.boss.position();
        let destination = (0..8)
            .map(|step| {
                let angle = f64::from(step) * FRAC_PI_4;
                origin + DVec3::new(angle.cos(), 0.0, angle.sin()) * BLINK_DISTANCE
            })
            .filter(|point| standable(self.world, *point))
            .max_by(|a, b| {
                a.distance(threat.position())
                    .total_cmp(&b.distance(threat.position()))
            });
        let Some(destination) = destination else {
            return;
        };
        self.particles(&vanilla_particle_types::PORTAL, origin, 40, 0.6);
        if self.boss.teleport_to(destination).is_ok() {
            self.particles(&vanilla_particle_types::PORTAL, destination, 40, 0.6);
            self.sound(&sound_events::ENTITY_ENDERMAN_TELEPORT, 1.0);
        }
    }

    fn shadow_step(&self) {
        let Some(target) = self.nearest() else {
            return;
        };
        let origin = self.boss.position();
        let away = target.position() - origin;
        let away = DVec3::new(away.x, 0.0, away.z).normalize_or_zero();
        let behind = target.position() + away * 1.5;
        if !standable(self.world, behind) {
            return;
        }
        self.particles(&vanilla_particle_types::LARGE_SMOKE, origin, 25, 0.5);
        if self.boss.teleport_to(behind).is_ok() {
            self.particles(&vanilla_particle_types::LARGE_SMOKE, behind, 25, 0.5);
            self.sound(&sound_events::ENTITY_ILLUSIONER_MIRROR_MOVE, 1.2);
            self.strike(target, BossAbility::ShadowStep);
        }
    }

    fn venom_blades(&self) {
        let ground = self.boss.position();
        self.particles(&vanilla_particle_types::SWEEP_ATTACK, ground, 6, 1.5);
        self.sound(&sound_events::ENTITY_PLAYER_ATTACK_SWEEP, 0.8);
        let (_, range) = BossAbility::VenomBlades.range();
        for target in self.within(ground, range) {
            self.strike(target, BossAbility::VenomBlades);
            Self::afflict(target, vanilla_mob_effects::POISON, 100, 1);
        }
    }

    fn smoke_bomb(&self) {
        let ground = self.boss.position();
        self.particles(&vanilla_particle_types::LARGE_SMOKE, ground, 120, 4.0);
        self.sound(&sound_events::ENTITY_ILLUSIONER_MIRROR_MOVE, 0.6);
        let (_, range) = BossAbility::SmokeBomb.range();
        for target in self.within(ground, range) {
            Self::afflict(target, vanilla_mob_effects::BLINDNESS, 60, 0);
        }
        Self::afflict(self.boss, vanilla_mob_effects::INVISIBILITY, 60, 0);
        Self::afflict(self.boss, vanilla_mob_effects::SPEED, 60, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::{BOSS_KINDS, BossAbility, BossFight, BossGear, BossKind, boss_for_gate};
    use crate::rank::HunterRank;

    #[test]
    fn bosses_grow_with_rank_and_red_gates() {
        let e_boss = boss_for_gate(BossKind::Warlord, HunterRank::E, false);
        assert_eq!(e_boss.health_multiplier, 8.0);
        assert_eq!(e_boss.damage_multiplier, 2.0);
        assert_eq!(e_boss.scale, 1.4);
        assert_eq!(e_boss.gear, BossGear::Iron);
        assert!(!e_boss.abilities.contains(&BossAbility::WarCry));

        let red_s_boss = boss_for_gate(BossKind::Warlord, HunterRank::S, true);
        assert_eq!(red_s_boss.scale, 2.0);
        assert_eq!(red_s_boss.health_multiplier, 48.0);
        assert_eq!(red_s_boss.damage_multiplier, 7.5);
        assert_eq!(red_s_boss.gear, BossGear::Netherite);
        assert!(red_s_boss.abilities.contains(&BossAbility::WarCry));
    }

    #[test]
    fn archetypes_trade_health_for_their_kit() {
        let warlord = boss_for_gate(BossKind::Warlord, HunterRank::C, false);
        let mage = boss_for_gate(BossKind::Archmage, HunterRank::C, false);
        let assassin = boss_for_gate(BossKind::Assassin, HunterRank::C, false);
        assert!(mage.health_multiplier < warlord.health_multiplier);
        assert!(assassin.health_multiplier < mage.health_multiplier);
        assert!(assassin.speed_multiplier > warlord.speed_multiplier);
        assert!(mage.speed_multiplier < warlord.speed_multiplier);
        // Spells scale from the rank, not from the mage's weak melee.
        assert_eq!(mage.power, warlord.power);
    }

    #[test]
    fn higher_ranks_unlock_more_of_each_kit() {
        for kind in BOSS_KINDS {
            let low = kind.abilities(HunterRank::E).len();
            let high = kind.abilities(HunterRank::S).len();
            assert!(low >= 2, "{kind:?}");
            assert!(high > low, "{kind:?}");
        }
        assert!(
            BossKind::Archmage
                .abilities(HunterRank::S)
                .contains(&BossAbility::Meteor)
        );
        assert!(
            !BossKind::Archmage
                .abilities(HunterRank::C)
                .contains(&BossAbility::Meteor)
        );
    }

    #[test]
    fn every_archetype_is_rolled() {
        let kinds: Vec<_> = (0..3_u64)
            .map(|roll| BossKind::from_seed(roll << 40))
            .collect();
        for kind in BOSS_KINDS {
            assert!(kinds.contains(&kind), "{kind:?}");
        }
    }

    #[test]
    fn abilities_wait_for_the_opening_delay_range_and_cooldown() {
        let boss = boss_for_gate(BossKind::Warlord, HunterRank::E, false);
        let mut fight = BossFight::new(&boss, 7, 0);
        assert_eq!(fight.choose_ability(0, 1.0), None);
        assert_eq!(fight.choose_ability(39, 1.0), None);
        assert_eq!(fight.choose_ability(40, 1.0), Some(BossAbility::Slam));
        // The roar fires next; the slam holds its cooldown even in range.
        assert_eq!(fight.choose_ability(41, 1.0), Some(BossAbility::DreadRoar));
        assert_eq!(fight.choose_ability(100, 1.0), None);
        assert_eq!(fight.choose_ability(140, 1.0), Some(BossAbility::Slam));
        // Out of reach nothing fires, and the slam stays ready.
        assert_eq!(fight.choose_ability(300, 99.0), None);
        assert_eq!(fight.choose_ability(301, 1.0), Some(BossAbility::Slam));
    }

    #[test]
    fn the_archmage_blinks_away_up_close_and_casts_from_afar() {
        let boss = boss_for_gate(BossKind::Archmage, HunterRank::S, false);
        let mut fight = BossFight::new(&boss, 7, 0);
        assert_eq!(fight.choose_ability(40, 15.0), Some(BossAbility::Meteor));
        assert_eq!(
            fight.choose_ability(41, 15.0),
            Some(BossAbility::ArcaneBolt)
        );
        assert_eq!(fight.choose_ability(42, 2.0), Some(BossAbility::Blink));
        assert_eq!(fight.choose_ability(43, 2.0), Some(BossAbility::FrostNova));
    }

    #[test]
    fn the_assassin_only_shadow_steps_from_a_distance() {
        let boss = boss_for_gate(BossKind::Assassin, HunterRank::E, false);
        let mut fight = BossFight::new(&boss, 7, 0);
        assert_eq!(
            fight.choose_ability(40, 2.0),
            Some(BossAbility::VenomBlades)
        );
        assert_eq!(fight.choose_ability(41, 2.0), None);
        assert_eq!(
            fight.choose_ability(42, 10.0),
            Some(BossAbility::ShadowStep)
        );
    }
}
