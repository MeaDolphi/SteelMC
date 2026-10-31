//! Enchantment effects beyond attributes, applied to live hunters.
//!
//! Every few ticks the enchantments a hunter wears are summed into the
//! session's [`EnchantSummary`]; the damage hook, the player tick and the
//! reward paths read that summary instead of decoding items on every event.
//! Effects that touch another entity, such as poison or reflected damage, are
//! queued here and resolved on the server tick, outside the damage callback.

use std::mem;
use std::sync::Arc;

use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity, LivingEntity, MobEffectInstance, SharedEntity};
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::{vanilla_damage_types, vanilla_mob_effects};
use steel_utils::Identifier;
use steel_utils::geometry::WorldAabb;
use steel_utils::types::InteractionHand;
use uuid::Uuid;

use super::SoloLevelingState;
use crate::abilities::now_millis;
use crate::class::HunterClass;
use crate::combat_reward::HunterReward;
use crate::dungeon::runtime::find_player;
use crate::gear;
use crate::item::artifact::{self, Category};
use crate::item::enchantment::{EnchantSummary, Enchantment};
use crate::item::serial;
use crate::spell::{self, SpellGear};

/// Ticks between two recounts of a hunter's worn enchantments.
const SUMMARY_TICKS: i32 = 5;
/// Health below which Second Wind and Executioner act, as a fraction.
const SECOND_WIND_THRESHOLD: f32 = 0.3;
/// Health Second Wind restores.
const SECOND_WIND_HEALTH: f32 = 4.0;
/// Milliseconds between two Second Winds.
const SECOND_WIND_COOLDOWN: u64 = 60_000;
/// Health below which Fury acts, as a fraction.
const FURY_THRESHOLD: f32 = 0.5;
/// Health below which Executioner acts, as a fraction of the target's.
const EXECUTIONER_THRESHOLD: f32 = 0.3;
/// Damage multiplier of a crushing hit.
const CRUSH_MULTIPLIER: f32 = 1.5;
/// Radius Cleave reaches around the struck target.
const CLEAVE_RADIUS: f64 = 2.0;
/// Milliseconds between two Flights.
const FLIGHT_COOLDOWN: u64 = 15_000;

/// An effect on another entity, resolved on the server tick.
#[derive(Debug)]
enum Proc {
    /// Damage back to an attacker, caused by the hunter it hit.
    Reflect { target: i32, damage: f32 },
    /// Part of a hit spread over mobs near the struck one.
    Cleave { struck: i32, damage: f32 },
    /// A mob effect on the struck entity.
    Afflict {
        target: i32,
        effect: &'static str,
        ticks: i32,
        amplifier: i32,
    },
    /// Fire on the struck entity.
    Ignite { target: i32, ticks: i32 },
    /// A second strike on the struck entity.
    Strike { target: i32, damage: f32 },
}

/// Queued enchantment effects; never persisted.
#[derive(Debug, Default)]
pub(super) struct EnchantRuntime {
    procs: Vec<(Uuid, Identifier, Proc)>,
}

/// Returns whether `entity` is a living mob an effect may hit.
fn is_mob(entity: &dyn Entity) -> bool {
    entity.as_player().is_none()
        && entity
            .as_living_entity()
            .is_some_and(|living| !living.is_dead_or_dying())
        && entity
            .as_mob()
            .is_none_or(|mob| mob.allied_owner().is_none())
}

fn chance(fraction: f32) -> bool {
    fraction > 0.0 && rand::random_range(0.0_f32..1.0) < fraction
}

/// Rolls the on-hit effects of a melee hit on `target`.
fn hit_procs(summary: &EnchantSummary, target: i32, damage: f32, double_strike: bool) -> Vec<Proc> {
    let mut procs = Vec::new();
    if summary.cleave > 0.0 {
        procs.push(Proc::Cleave {
            struck: target,
            damage: damage * summary.cleave,
        });
    }
    if chance(summary.venom) {
        procs.push(Proc::Afflict {
            target,
            effect: "poison",
            ticks: 80,
            amplifier: 0,
        });
    }
    if chance(summary.frost) {
        procs.push(Proc::Afflict {
            target,
            effect: "slowness",
            ticks: 40,
            amplifier: 1,
        });
    }
    if summary.flame {
        procs.push(Proc::Ignite { target, ticks: 60 });
    }
    if double_strike {
        procs.push(Proc::Strike {
            target,
            damage: damage * summary.double_strike_share,
        });
    }
    procs
}

impl SoloLevelingState {
    /// Returns the enchantments `player` wears: the held weapon, a Mage
    /// staff in the off hand, and the System gear they may use.
    fn worn_enchantments(&self, player: &Player) -> Vec<Enchantment> {
        let Some((class, rank, strength, worn)) =
            self.with_session_mut(player.gameprofile.id(), |session| {
                (
                    session.class.class,
                    session.stats.rank(),
                    session.stats.strength,
                    session.gear.clone(),
                )
            })
        else {
            return Vec::new();
        };
        let (main, off): (ItemStack, ItemStack) = {
            let inventory = player.inventory.lock();
            (
                inventory
                    .get_item_in_hand(InteractionHand::MainHand)
                    .clone(),
                inventory.get_item_in_hand(InteractionHand::OffHand).clone(),
            )
        };
        let artifact_weapon = |stack: &ItemStack| {
            artifact::held(stack).is_some_and(|held| held.category == Category::Weapon)
        };
        let artifact_staff = |stack: &ItemStack| {
            artifact_weapon(stack)
                && artifact::of(stack).is_some_and(|id| id.spell_gear().is_some())
        };
        let serial_weapon = |stack: &ItemStack| {
            serial::held(stack)
                .is_some_and(|serial| serial.category != crate::item::artifact::Category::Armor)
        };
        let serial_staff =
            |stack: &ItemStack| serial::of(stack).is_some_and(|id| id.spell_gear().is_some());
        let artifact_enchants = [main.clone()]
            .into_iter()
            .filter(artifact_weapon)
            .chain([off.clone()].into_iter().filter(artifact_staff))
            .filter(|stack| artifact::usable_by(stack, rank) && gear::wearable(stack, class))
            .chain(worn.into_iter().filter(|stack| {
                artifact::usable_by(stack, rank)
                    && gear::wearable(stack, class)
                    && gear::strong_enough(stack, strength)
            }))
            .flat_map(|stack| artifact::enchantments_of(&stack));
        let serial_hand_enchants = [main]
            .into_iter()
            .filter(serial_weapon)
            .chain([off].into_iter().filter(serial_staff))
            .flat_map(|stack| serial::enchantments_of(&stack));
        let serial_armor_enchants = {
            [
                EquipmentSlot::Head,
                EquipmentSlot::Chest,
                EquipmentSlot::Legs,
                EquipmentSlot::Feet,
            ]
            .into_iter()
            .filter_map(|slot| {
                let mut stack = ItemStack::empty();
                player.with_equipment_slot(slot, &mut |item| stack = item.clone());
                serial::of(&stack).map(|id| (stack, id))
            })
            .filter(|(_, id)| {
                id.allowed_classes()
                    .is_none_or(|classes| class.is_some_and(|c| classes.contains(&c)))
                    && id.required_strength() <= strength
            })
            .flat_map(|(stack, _)| serial::enchantments_of(&stack))
            .collect::<Vec<_>>()
        };
        artifact_enchants
            .chain(serial_hand_enchants)
            .chain(serial_armor_enchants)
            .collect()
    }

    /// Returns the enchantment summary of a hunter.
    pub(super) fn enchant_summary(&self, uuid: Uuid) -> EnchantSummary {
        self.with_session_mut(uuid, |session| session.enchant.summary)
            .unwrap_or(EnchantSummary::NONE)
    }

    /// Returns the spellcasting bonuses a Mage's enchantments add.
    pub(super) fn enchant_spell_gear(&self, uuid: Uuid) -> SpellGear {
        self.enchant_summary(uuid).spell
    }

    /// Heals a hunter, weakened by Blood Curse; returns the health restored.
    pub(super) fn heal_hunter(&self, player: &Player, amount: f32) -> f32 {
        if player.is_dead_or_dying() {
            return 0.0;
        }
        let amount = self.enchant_summary(player.gameprofile.id()).healed(amount);
        let before = player.get_health();
        let health = (before + amount).min(player.get_max_health());
        if health > before {
            player.set_health(health);
        }
        (health - before).max(0.0)
    }

    /// Recounts worn enchantments and runs the timed ones: regeneration,
    /// Second Wind, Flight, Mining Rush and the immunities.
    pub(super) fn tick_enchantments(&self, player: &Player) {
        if player.tick_count() % SUMMARY_TICKS != 0 {
            return;
        }
        let uuid = player.gameprofile.id();
        let summary = EnchantSummary::of(self.worn_enchantments(player))
            .with_abilities(&self.gear_abilities(player));
        let now = now_millis();
        let fraction = player.get_health() / player.get_max_health().max(1.0);
        let seconds = SUMMARY_TICKS as f32 / 20.0;
        let Some((regen, second_wind, flight, rush)) = self.with_session_mut(uuid, |session| {
            let state = &mut session.enchant;
            state.summary = summary;
            state.regen_owed += summary.regen_per_second * seconds;
            let regen = if state.regen_owed >= 1.0 {
                mem::take(&mut state.regen_owed)
            } else {
                0.0
            };
            let second_wind = summary.second_wind
                && fraction < SECOND_WIND_THRESHOLD
                && state.trigger(Enchantment::SecondWind, now, SECOND_WIND_COOLDOWN);
            let flight = mem::take(&mut state.flight_due)
                && summary.flight
                && state.trigger(Enchantment::Flight, now, FLIGHT_COOLDOWN);
            let rush = mem::take(&mut state.rush_due) && summary.mining_rush;
            (regen, second_wind, flight, rush)
        }) else {
            return;
        };
        if player.is_dead_or_dying() {
            return;
        }
        if regen > 0.0 {
            let _ = self.heal_hunter(player, regen);
        }
        if second_wind {
            let _ = self.heal_hunter(player, SECOND_WIND_HEALTH);
        }
        if flight {
            player.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::SPEED,
                60,
                0,
            ));
        }
        if rush {
            player.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::HASTE,
                100,
                0,
            ));
        }
        if summary.antidote {
            player.remove_mob_effect(vanilla_mob_effects::POISON);
            player.remove_mob_effect(vanilla_mob_effects::WITHER);
        }
        if summary.clear_sight {
            player.remove_mob_effect(vanilla_mob_effects::BLINDNESS);
            player.remove_mob_effect(vanilla_mob_effects::DARKNESS);
        }
        // Haste is also vanilla's +10% attack speed.
        if fraction < summary.berserk_threshold {
            player.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::HASTE,
                SUMMARY_TICKS * 2,
                0,
            ));
        }
    }

    /// Adjusts damage for the enchantments of the attacker and the victim.
    ///
    /// Called from the class damage hook after class and spell bonuses.
    pub(super) fn enchant_damage(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        let mut damage = amount;
        let world = victim.level().map(|world| world.key.clone());
        if let Some(attacker) = source
            .causing_entity()
            .and_then(|entity| entity.as_player())
        {
            damage = self.attacker_enchantments(attacker, victim, source, damage, world.as_ref());
        }
        if let Some(player) = victim.as_player() {
            damage = self.victim_enchantments(player, source, damage, world.as_ref());
        }
        damage
    }

    fn attacker_enchantments(
        &self,
        attacker: &Player,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
        world: Option<&Identifier>,
    ) -> f32 {
        let uuid = attacker.gameprofile.id();
        let Some((summary, class)) = self.with_session_mut(uuid, |session| {
            (session.enchant.summary, session.class.class)
        }) else {
            return amount;
        };
        let melee = source.is_direct();
        let magic = matches!(
            source.damage_type.key.path.as_ref(),
            "magic" | "indirect_magic"
        );
        let mut bonus = 0.0;
        let health = attacker.get_health();
        if health < attacker.get_max_health() * FURY_THRESHOLD {
            bonus += summary.fury;
        }
        if health < attacker.get_max_health() * summary.berserk_threshold {
            bonus += summary.berserk_bonus;
        }
        if melee {
            if victim.get_health() < victim.get_max_health() * EXECUTIONER_THRESHOLD {
                bonus += summary.executioner;
            }
            if summary.bossbane > 0.0
                && victim
                    .level()
                    .is_some_and(|world| self.is_boss(&world, victim.id()))
            {
                bonus += summary.bossbane;
            }
        }
        let mut damage = amount * (1.0 + bonus);
        if melee && chance(summary.crush) {
            damage *= CRUSH_MULTIPLIER;
        }
        let mage = class == Some(HunterClass::Mage);
        let double_strike = self
            .with_session_mut(uuid, |session| {
                if magic && mage {
                    session.class_heal += damage * summary.bloodsucker;
                }
                if melee {
                    session.class_heal += damage * summary.bloodthirst;
                }
                if mage && (melee || magic) && summary.mana_feed > 0.0 {
                    let maximum = spell::max_mana(session.stats.intelligence);
                    let mana = session.magic.mana.get_or_insert(maximum);
                    *mana += summary.mana_feed;
                }
                let every = summary.double_strike_every;
                if melee && every > 0 {
                    let strikes = &mut session.enchant.strikes;
                    *strikes += 1;
                    if *strikes >= every {
                        *strikes = 0;
                        return true;
                    }
                }
                false
            })
            .unwrap_or(false);
        if melee && let Some(world) = world {
            let procs = hit_procs(&summary, victim.id(), damage, double_strike);
            if !procs.is_empty() {
                let mut runtime = self.enchant_runtime.lock();
                runtime
                    .procs
                    .extend(procs.into_iter().map(|proc| (uuid, world.clone(), proc)));
            }
        }
        damage
    }

    fn victim_enchantments(
        &self,
        player: &Player,
        source: &DamageSource,
        amount: f32,
        world: Option<&Identifier>,
    ) -> f32 {
        let uuid = player.gameprofile.id();
        let Some(summary) = self.with_session_mut(uuid, |session| {
            session.enchant.flight_due = true;
            session.enchant.summary
        }) else {
            return amount;
        };
        if chance(summary.evasion) {
            return 0.0;
        }
        let mut factor = 1.0 + summary.incoming;
        if source.is(&DamageTypeTag::IS_FIRE) {
            factor -= summary.fire_ward;
        }
        if source.is(&DamageTypeTag::IS_EXPLOSION) {
            factor -= summary.blast_ward;
        }
        if source.is(&DamageTypeTag::IS_PROJECTILE) {
            factor -= summary.arrow_ward;
        }
        let damage = amount * factor.max(0.0);
        if summary.reflect > 0.0
            && source.is_direct()
            && let Some(attacker) = source.causing_entity()
            && attacker.as_player().is_none()
            && let Some(world) = world
        {
            self.enchant_runtime.lock().procs.push((
                uuid,
                world.clone(),
                Proc::Reflect {
                    target: attacker.id(),
                    damage: damage * summary.reflect,
                },
            ));
        }
        damage
    }

    /// Resolves queued enchantment effects.
    pub fn tick_enchant_effects(&self, server: &Server) {
        let procs = mem::take(&mut self.enchant_runtime.lock().procs);
        for (owner, world_key, proc) in procs {
            let Some(world) = server.worlds.get(&world_key) else {
                continue;
            };
            let Some(owner) = find_player(server, owner) else {
                continue;
            };
            let cause: SharedEntity = Arc::<Player>::clone(&owner);
            match proc {
                Proc::Reflect { target, damage } | Proc::Strike { target, damage } => {
                    let source = DamageSource::environment(&vanilla_damage_types::THORNS)
                        .with_causing_entity(cause);
                    if let Some(living) = world
                        .get_entity_by_id(target)
                        .as_ref()
                        .and_then(|entity| entity.as_living_entity())
                    {
                        living.hurt(world, &source, damage);
                    }
                }
                Proc::Cleave { struck, damage } => {
                    let Some(center) = world
                        .get_entity_by_id(struck)
                        .map(|entity| entity.position())
                    else {
                        continue;
                    };
                    let source = DamageSource::environment(&vanilla_damage_types::GENERIC)
                        .with_causing_entity(cause);
                    let area = WorldAabb::of_size(
                        center,
                        CLEAVE_RADIUS * 2.0,
                        CLEAVE_RADIUS * 2.0,
                        CLEAVE_RADIUS * 2.0,
                    );
                    for target in world.get_entities_in_aabb_matching(&area, |entity| {
                        entity.id() != struck
                            && is_mob(entity)
                            && entity.position().distance_squared(center)
                                <= CLEAVE_RADIUS * CLEAVE_RADIUS
                    }) {
                        if let Some(living) = target.as_living_entity() {
                            living.hurt(world, &source, damage);
                        }
                    }
                }
                Proc::Afflict {
                    target,
                    effect,
                    ticks,
                    amplifier,
                } => {
                    let effect = match effect {
                        "poison" => vanilla_mob_effects::POISON,
                        _ => vanilla_mob_effects::SLOWNESS,
                    };
                    if let Some(living) = world
                        .get_entity_by_id(target)
                        .as_ref()
                        .and_then(|entity| entity.as_living_entity())
                    {
                        living.add_mob_effect(MobEffectInstance::with_duration(
                            effect, ticks, amplifier,
                        ));
                    }
                }
                Proc::Ignite { target, ticks } => {
                    if let Some(entity) = world.get_entity_by_id(target) {
                        entity.set_remaining_fire_ticks(ticks);
                    }
                }
            }
        }
    }

    /// Saves a hunter wearing the Shadow Guard amulet from a lethal hit.
    pub fn shadow_guard_prevent_death(&self, victim: &dyn LivingEntity) -> bool {
        let Some(player) = victim.as_player() else {
            return false;
        };
        let now = now_millis();
        let guard = self
            .with_session_mut(player.gameprofile.id(), |session| {
                let (seconds, cooldown) = session.enchant.summary.shadow_guard?;
                let ready = session
                    .enchant
                    .guard_ready_at
                    .is_none_or(|ready| ready <= now);
                if !ready {
                    return None;
                }
                session.enchant.guard_ready_at = Some(now + cooldown * 1_000);
                Some(seconds)
            })
            .flatten();
        let Some(seconds) = guard else {
            return false;
        };
        player.set_health(1.0);
        player.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::RESISTANCE,
            i32::try_from(seconds * 20).unwrap_or(60),
            4,
        ));
        true
    }

    /// Scales a kill reward by the hunter's Hunter, Wisdom, Gatherer, Greed
    /// and Distraction enchantments.
    pub(super) fn enchanted_reward(&self, uuid: Uuid, reward: HunterReward) -> HunterReward {
        let summary = self.enchant_summary(uuid);
        let scale = |value: u64, change: f32| {
            ((value as f64) * f64::from((1.0 + change).max(0.0))).round() as u64
        };
        HunterReward {
            experience: scale(reward.experience, summary.experience),
            system_points: scale(reward.system_points, summary.points),
        }
    }

    /// Returns the extra crystals and essence chance the worn enchantments
    /// add to one mined ore, and owes Mining Rush.
    pub fn enchant_mining(&self, player: &Player) -> (i32, bool) {
        let uuid = player.gameprofile.id();
        let summary = self.enchant_summary(uuid);
        let extra = i32::from(rand::random_range(0..100) < summary.crystal_percent);
        let essence = rand::random_range(0..100) < summary.essence_percent;
        if summary.mining_rush {
            self.with_session_mut(uuid, |session| session.enchant.rush_due = true);
        }
        (extra, essence)
    }
}
