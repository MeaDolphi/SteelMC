//! Spells cast from the grimoire, applied to the live world.
//!
//! Item use only queues a cast; the server tick resolves it, the same way
//! artifact storms run, so no spell touches entities from inside another
//! entity's callback. Spell damage is magic caused by the caster, so the
//! Mage bonus, robes and magic artifacts raise it through the ordinary
//! damage hook.

use std::fmt::Write as _;
use std::mem;
use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity, LivingEntity, MobEffectInstance, SharedEntity};
use steel_core::inventory::container::Container as _;
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_core::server::Server;
use steel_core::world::World;
use steel_protocol::packets::game::CSetExperience;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::particle_type::{ParticleData, ParticleType};
use steel_registry::{vanilla_damage_types, vanilla_mob_effects, vanilla_particle_types};
use steel_utils::geometry::WorldAabb;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, Identifier};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use super::abilities::Storm;
use crate::abilities::now_millis;
use crate::class::{self, HunterClass};
use crate::dungeon::is_instance_world;
use crate::dungeon::runtime::find_player;
use crate::gear;
use crate::item::artifact::{self, Category};
use crate::item::grimoire;
use crate::item::serial;
use crate::quest_book::Goal;
use crate::rank::HunterRank;
use crate::spell::{self, School, SpellError, SpellGear, SpellId};

/// Reach of aimed spells, in blocks.
const AIM_RANGE: f64 = 24.0;
/// Reach of the Meteor and the Void Lance, in blocks.
const LONG_RANGE: f64 = 32.0;
/// Step of a line-of-sight trace, in blocks.
const TRACE_STEP: f64 = 0.25;
/// Radius of party spells, in blocks.
const PARTY_RADIUS: f64 = 10.0;
/// Milliseconds between two pulses of a healing effect.
const PULSE_MILLIS: u64 = 1_000;
/// Damage taken by a Weakened mob's victim, as a fraction.
const WEAKEN_FACTOR: f32 = 0.75;
/// Damage taken inside a Sanctuary, as a fraction.
const WARD_FACTOR: f32 = 0.70;
/// Extra damage of a blessed hunter, as a fraction.
const BLESSING_BONUS: f32 = 0.15;
/// Extra destruction damage under Monarch's Wrath.
const WRATH_DAMAGE: f32 = 1.3;
/// Destruction mana cost under Monarch's Wrath, as a fraction.
const WRATH_MANA: f32 = 0.5;
/// Mana a Mana Shield spends per point of health it absorbs.
const SHIELD_MANA_PER_HEALTH: f32 = 2.0;
/// Milliseconds of a full Meditation.
const MEDITATION_MILLIS: u64 = 4_000;
/// Share of maximum mana a full Meditation restores.
const MEDITATION_SHARE: f32 = 0.5;

/// A Meteor on its way down.
#[derive(Debug)]
struct Meteor {
    owner: Uuid,
    world: Identifier,
    center: DVec3,
    damage: f32,
    burn_ticks: i32,
    due: u64,
}

/// A Sanctuary circle.
#[derive(Debug)]
struct Sanctuary {
    owner: Uuid,
    world: Identifier,
    center: DVec3,
    heal: f32,
    until: u64,
    next_pulse: u64,
}

/// Restoration on one hunter.
#[derive(Debug)]
struct Regrowth {
    caster: Uuid,
    target: Uuid,
    heal: f32,
    until: u64,
    next_pulse: u64,
}

/// Queued casts and running spell effects; never persisted.
#[derive(Debug, Default)]
pub(super) struct SpellRuntime {
    casts: Vec<Uuid>,
    meteors: Vec<Meteor>,
    sanctuaries: Vec<Sanctuary>,
    regrowths: Vec<Regrowth>,
    /// Weakened mobs by entity id, with the end of the effect.
    weakened: FxHashMap<i32, u64>,
}

/// Returns whether `entity` is a living mob a spell may hit.
fn is_enemy(entity: &dyn Entity) -> bool {
    entity.as_player().is_none()
        && entity
            .as_living_entity()
            .is_some_and(|living| !living.is_dead_or_dying())
        && entity
            .as_mob()
            .is_none_or(|mob| mob.allied_owner().is_none())
}

/// Returns whether a body passes through the block at `pos`.
fn passable(world: &World, pos: DVec3) -> bool {
    !world.get_block_state(BlockPos::from(pos)).blocks_motion()
}

/// Returns whether `point` lies inside `entity`'s rough body.
fn touches(entity: &dyn Entity, point: DVec3) -> bool {
    let feet = entity.position();
    let dx = feet.x - point.x;
    let dz = feet.z - point.z;
    dx * dx + dz * dz <= 0.8 * 0.8 && point.y >= feet.y - 0.3 && point.y <= feet.y + 2.2
}

/// A line of sight: where it ended and what it passed through, nearest first.
struct Trace {
    end: DVec3,
    hits: Vec<SharedEntity>,
}

/// Walks from `from` along `direction` until a solid block or `range`,
/// collecting entities `accept` lets through; `pierce` keeps going past them.
fn trace(
    world: &World,
    from: DVec3,
    direction: DVec3,
    range: f64,
    pierce: bool,
    accept: impl Fn(&dyn Entity) -> bool,
) -> Trace {
    let direction = direction.normalize_or_zero();
    let mut hits: Vec<SharedEntity> = Vec::new();
    let mut end = from;
    let steps = (range / TRACE_STEP) as u32;
    for step in 1..=steps {
        let point = from + direction * (f64::from(step) * TRACE_STEP);
        if !passable(world, point) {
            break;
        }
        end = point;
        let area = WorldAabb::of_size(point, 4.0, 4.0, 4.0);
        for entity in world
            .get_entities_in_aabb_matching(&area, |entity| accept(entity) && touches(entity, point))
        {
            if !hits.iter().any(|hit| hit.id() == entity.id()) {
                hits.push(entity);
            }
        }
        if !pierce && !hits.is_empty() {
            break;
        }
    }
    Trace { end, hits }
}

/// Returns the living mobs within `radius` of `center`.
fn enemies_near(world: &World, center: DVec3, radius: f64) -> Vec<SharedEntity> {
    let area = WorldAabb::of_size(center, radius * 2.0, radius * 2.0, radius * 2.0);
    world.get_entities_in_aabb_matching(&area, |entity| {
        is_enemy(entity) && entity.position().distance_squared(center) <= radius * radius
    })
}

fn particles(world: &World, kind: &'static ParticleType, at: DVec3, count: i32, spread: f64) {
    world.send_particles(
        ParticleData::simple(kind),
        at,
        count,
        DVec3::new(spread, spread, spread),
        0.0,
    );
}

/// Draws a line of particles from `from` to `to`.
fn beam(world: &World, kind: &'static ParticleType, from: DVec3, to: DVec3) {
    let length = from.distance(to);
    let steps = (length / 0.75).ceil().max(1.0) as u32;
    for step in 0..=steps {
        particles(
            world,
            kind,
            from.lerp(to, f64::from(step) / f64::from(steps)),
            1,
            0.0,
        );
    }
}

fn afflict(entity: &SharedEntity, effect: MobEffectRef, ticks: i32, amplifier: i32) {
    if let Some(living) = entity.as_living_entity() {
        living.add_mob_effect(MobEffectInstance::with_duration(effect, ticks, amplifier));
    }
}

fn tell(player: &Player, text: &str, color: Color) {
    player.send_message(&TextComponent::plain(text.to_owned()).color(color));
}

/// Compass direction from `from` to `to`, in Russian.
fn compass(from: DVec3, to: DVec3) -> &'static str {
    const NAMES: [&str; 8] = [
        "север",
        "северо-восток",
        "восток",
        "юго-восток",
        "юг",
        "юго-запад",
        "запад",
        "северо-запад",
    ];
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    // Minecraft's north is negative z.
    let angle = dx.atan2(-dz).to_degrees().rem_euclid(360.0);
    NAMES[(((angle + 22.5) / 45.0) as usize) % 8]
}

/// Everything one cast needs to know about its caster.
struct Caster<'a> {
    state: &'a SoloLevelingState,
    player: &'a Arc<Player>,
    world: Arc<World>,
    eye: DVec3,
    look: DVec3,
    /// Rank scale of healing and protection.
    scale: f32,
    /// Healing multiplier of the caster's artifacts.
    healing: f32,
    /// Extra Barrier shield health of the caster's artifacts.
    barrier: f32,
    /// Fire duration multiplier of the caster's artifacts.
    burn: i32,
    /// Damage multiplier of destruction spells.
    power: f32,
    now: u64,
}

impl Caster<'_> {
    fn source(&self) -> DamageSource {
        let cause: SharedEntity = Arc::<Player>::clone(self.player);
        DamageSource::environment(&vanilla_damage_types::MAGIC).with_causing_entity(cause)
    }

    fn hurt(&self, target: &SharedEntity, damage: f32) {
        if let Some(living) = target.as_living_entity() {
            living.hurt(&self.world, &self.source(), damage * self.power);
        }
    }

    fn aim(&self, range: f64, pierce: bool) -> Trace {
        trace(&self.world, self.eye, self.look, range, pierce, is_enemy)
    }

    /// Party members online in the caster's world within `radius`, the
    /// caster included.
    fn allies(&self, radius: f64) -> Vec<Arc<Player>> {
        let uuid = self.player.gameprofile.id();
        let members = self
            .state
            .party_of(uuid)
            .map_or_else(|| vec![uuid], |party| party.members().to_vec());
        let center = self.player.position();
        members
            .iter()
            .filter_map(|member| self.world.players.get_by_uuid(member))
            .filter(|ally| {
                !ally.is_dead_or_dying()
                    && ally.position().distance_squared(center) <= radius * radius
            })
            .collect()
    }

    /// The party member in the line of sight, or the caster.
    fn ally_in_sight(&self) -> Arc<Player> {
        let caster = self.player.gameprofile.id();
        let allies: Vec<Uuid> = self
            .allies(AIM_RANGE)
            .iter()
            .map(|ally| ally.gameprofile.id())
            .filter(|ally| *ally != caster)
            .collect();
        trace(
            &self.world,
            self.eye,
            self.look,
            AIM_RANGE,
            false,
            |entity| {
                entity
                    .as_player()
                    .is_some_and(|player| allies.contains(&player.gameprofile.id()))
            },
        )
        .hits
        .first()
        .and_then(|hit| hit.as_player().map(|player| player.gameprofile.id()))
        .and_then(|ally| self.world.players.get_by_uuid(&ally))
        .unwrap_or_else(|| Arc::clone(self.player))
    }

    fn for_allies(&self, effect: impl Fn(&Arc<Player>)) {
        for ally in self.allies(PARTY_RADIUS) {
            effect(&ally);
            particles(
                &self.world,
                &vanilla_particle_types::HAPPY_VILLAGER,
                ally.position() + DVec3::Y,
                8,
                0.4,
            );
        }
    }
}

impl SoloLevelingState {
    /// Queues a cast of the selected spell for the next server tick.
    pub fn queue_cast(&self, uuid: Uuid) {
        self.spell_runtime.lock().casts.push(uuid);
    }

    /// Selects the next prepared spell and shows it.
    pub fn cycle_spell(&self, player: &Player) {
        let selected = self
            .with_session_mut(player.gameprofile.id(), |session| session.spells.cycle())
            .flatten();
        let text = selected.map_or_else(
            || "Нет подготовленных заклинаний.".to_owned(),
            |spell| format!("Выбрано: {}", spell.info().name),
        );
        player.send_action_bar(TextComponent::plain(text).color(Color::LightPurple));
    }

    /// Selects the prepared spell at `index`.
    pub fn select_spell(&self, uuid: Uuid, index: usize) {
        self.with_session_mut(uuid, |session| session.spells.select(index));
    }

    /// Owes `uuid` a grimoire, handed out on the next tick if it has none.
    pub fn request_grimoire(&self, uuid: Uuid) {
        self.with_session_mut(uuid, |session| session.magic.grimoire_due = true);
    }

    /// Unlocks `spell` for system points and saves the spell book.
    pub fn unlock_spell(&self, player: &Player, spell: SpellId) -> Result<u64, SpellError> {
        let uuid = player.gameprofile.id();
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(SpellError::Storage);
        }
        let price = self
            .with_session_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(SpellError::Storage);
                }
                if session.class.class != Some(HunterClass::Mage) {
                    return Err(SpellError::NotAMage);
                }
                let price = session.spells.unlock_price(
                    spell,
                    session.stats.rank(),
                    session.system_points.amount(),
                )?;
                if !session.system_points.debit(price) {
                    return Err(SpellError::InsufficientPoints);
                }
                session.spells.unlock(spell);
                Ok(price)
            })
            .ok_or(SpellError::Storage)??;
        self.record_goal(uuid, Goal::SpellUnlocked, 1);
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist spells of {uuid}: {error}");
        }
        Ok(price)
    }

    /// Prepares `spell` or puts it away, outside instances only.
    pub fn toggle_spell(&self, player: &Player, spell: SpellId) -> Result<(), SpellError> {
        if is_instance_world(&player.get_world()) {
            return Err(SpellError::InInstance);
        }
        let uuid = player.gameprofile.id();
        self.with_session_mut(uuid, |session| {
            let rank = session.stats.rank();
            session.spells.toggle_prepared(spell, rank)
        })
        .ok_or(SpellError::Storage)??;
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist spells of {uuid}: {error}");
        }
        Ok(())
    }

    /// Regenerates mana, finishes Meditation, hands out an owed grimoire and
    /// shows mana on the experience bar of a Mage.
    #[expect(
        clippy::too_many_lines,
        reason = "the session is read once per tick and every result applied after"
    )]
    pub(super) fn tick_mana(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let now = now_millis();
        let position = player.position();
        // Vanilla experience updates may repaint the bar, so it is redrawn
        // every two seconds even when mana stands still.
        let repaint = player.tick_count() % 40 == 0;
        let gear = self.spell_gear(player);
        let Some(update) = self
            .with_session_mut(uuid, |session| {
                let magic = &mut session.magic;
                if session.class.class != Some(HunterClass::Mage) {
                    // A former Mage gets the vanilla bar back once.
                    return magic.shown_mana.take().map(|_| ManaTick::Cleared);
                }
                let mut learned = false;
                if session.spells.unlocked.is_empty() {
                    session.spells.learn_starting();
                    magic.grimoire_due = true;
                    learned = true;
                }
                let intelligence = session.stats.intelligence;
                let maximum = spell::max_mana(intelligence) + gear.mana;
                let calm = now.saturating_sub(magic.last_hurt) >= spell::CALM_AFTER_MILLIS;
                let mut mana = magic.mana.unwrap_or(maximum);
                mana += spell::mana_regen(intelligence, calm) * (1.0 + gear.regen) / 20.0;
                let mut note = None;
                if let Some((start, place)) = magic.meditation {
                    if magic.last_hurt > start || place.distance_squared(position) > 0.25 {
                        magic.meditation = None;
                        note = Some(("Медитация прервана.", Color::Red));
                    } else if now >= start + MEDITATION_MILLIS {
                        magic.meditation = None;
                        mana += maximum * MEDITATION_SHARE;
                        note = Some(("Медитация завершена: мана восстановлена.", Color::Aqua));
                    }
                }
                let mana = mana.min(maximum);
                magic.mana = Some(mana);
                let shown = mana.floor() as i32;
                let refresh = repaint || magic.shown_mana != Some(shown);
                magic.shown_mana = Some(shown);
                let grimoire = mem::take(&mut magic.grimoire_due);
                let hint = session.spells.selected_spell().map(|spell| {
                    let ready = magic
                        .cooldowns
                        .get(&spell)
                        .map_or(0, |deadline| deadline.saturating_sub(now));
                    let wrath =
                        magic.wrath_until > now && spell.info().school == School::Destruction;
                    (spell, ready, cast_cost(gear, spell, wrath))
                });
                Some(ManaTick::Mage {
                    mana,
                    maximum,
                    refresh,
                    grimoire,
                    learned,
                    note,
                    hint,
                })
            })
            .flatten()
        else {
            return;
        };
        match update {
            ManaTick::Cleared => player.send_packet(CSetExperience {
                progress: 0.0,
                level: 0,
                total_experience: 0,
            }),
            ManaTick::Mage {
                mana,
                maximum,
                refresh,
                grimoire,
                learned,
                note,
                hint,
            } => {
                if refresh {
                    player.send_packet(CSetExperience {
                        progress: (mana / maximum).clamp(0.0, 1.0),
                        level: mana.floor() as i32,
                        total_experience: 0,
                    });
                }
                if let Some((text, color)) = note {
                    tell(player, text, color);
                }
                if learned && let Err(error) = self.save_session(uuid) {
                    log::warn!("sololeveling: failed to persist spells of {uuid}: {error}");
                }
                if grimoire && !player.inventory_transaction_active() {
                    let has = {
                        let inventory = player.inventory.lock();
                        (0..inventory.get_container_size())
                            .any(|slot| grimoire::is_grimoire(inventory.get_item(slot)))
                    };
                    if !has {
                        player.add_item_or_drop(grimoire::stack(uuid), Prediction::ServerOnly);
                        tell(player, "Гримуар Системы получен.", Color::LightPurple);
                    }
                }
                if holds_grimoire(player) && player.tick_count() % 10 == 0 {
                    let line = hint.map_or_else(
                        || "Нет подготовленных заклинаний: Shift + ПКМ".to_owned(),
                        |(spell, ready, cost)| {
                            let status = if ready > 0 {
                                format!("{:.1} с", ready as f64 / 1_000.0)
                            } else {
                                "готово".to_owned()
                            };
                            format!(
                                "{} · {} маны · {status} | Мана {}/{}",
                                spell.info().name,
                                cost.round(),
                                mana.floor(),
                                maximum.floor()
                            )
                        },
                    );
                    player.send_action_bar(TextComponent::plain(line).color(Color::LightPurple));
                }
            }
        }
    }

    /// Resolves queued casts and advances running spell effects.
    pub fn tick_spells(&self, server: &Server) {
        let now = now_millis();
        let (casts, meteors) = {
            let mut runtime = self.spell_runtime.lock();
            runtime.weakened.retain(|_, until| *until > now);
            let casts = mem::take(&mut runtime.casts);
            let (due, waiting): (Vec<_>, Vec<_>) = mem::take(&mut runtime.meteors)
                .into_iter()
                .partition(|meteor| meteor.due <= now);
            runtime.meteors = waiting;
            (casts, due)
        };
        for uuid in casts {
            if let Some(player) = find_player(server, uuid) {
                self.cast(&player, now);
            }
        }
        for meteor in meteors {
            Self::land_meteor(server, &meteor);
        }
        self.pulse_sanctuaries(server, now);
        self.pulse_regrowths(server, now);
    }

    fn land_meteor(server: &Server, meteor: &Meteor) {
        let Some(owner) = find_player(server, meteor.owner) else {
            return;
        };
        let Some(world) = server.worlds.get(&meteor.world) else {
            return;
        };
        particles(
            world,
            &vanilla_particle_types::EXPLOSION_EMITTER,
            meteor.center,
            1,
            0.0,
        );
        particles(world, &vanilla_particle_types::LAVA, meteor.center, 30, 2.5);
        let cause: SharedEntity = Arc::<Player>::clone(&owner);
        let source =
            DamageSource::environment(&vanilla_damage_types::MAGIC).with_causing_entity(cause);
        for target in enemies_near(world, meteor.center, 5.0) {
            if let Some(living) = target.as_living_entity() {
                living.hurt(world, &source, meteor.damage);
            }
            target.set_remaining_fire_ticks(meteor.burn_ticks);
        }
    }

    fn pulse_sanctuaries(&self, server: &Server, now: u64) {
        let due: Vec<(Uuid, Identifier, DVec3, f32)> = {
            let mut runtime = self.spell_runtime.lock();
            runtime.sanctuaries.retain(|zone| zone.until > now);
            runtime
                .sanctuaries
                .iter_mut()
                .filter(|zone| zone.next_pulse <= now)
                .map(|zone| {
                    zone.next_pulse += PULSE_MILLIS;
                    (zone.owner, zone.world.clone(), zone.center, zone.heal)
                })
                .collect()
        };
        for (owner, world_key, center, amount) in due {
            let Some(world) = server.worlds.get(&world_key) else {
                continue;
            };
            let members = self
                .party_of(owner)
                .map_or_else(|| vec![owner], |party| party.members().to_vec());
            particles(world, &vanilla_particle_types::END_ROD, center, 16, 3.0);
            let mut healed = 0.0;
            for member in members {
                let Some(ally) = world.players.get_by_uuid(&member) else {
                    continue;
                };
                if ally.position().distance_squared(center) > 6.0 * 6.0 {
                    continue;
                }
                healed += self.heal_hunter(&ally, amount);
                self.with_session_mut(member, |session| {
                    session.magic.ward_until = now + PULSE_MILLIS + 500;
                });
            }
            self.record_healing(owner, healed);
        }
    }

    /// Counts spell healing toward the healer's errands.
    fn record_healing(&self, caster: Uuid, healed: f32) {
        let healed = healed.round() as u32;
        if healed > 0 {
            self.record_goal(caster, Goal::SpellHealing, healed);
        }
    }

    fn pulse_regrowths(&self, server: &Server, now: u64) {
        let due: Vec<(Uuid, Uuid, f32)> = {
            let mut runtime = self.spell_runtime.lock();
            runtime.regrowths.retain(|regrowth| regrowth.until > now);
            runtime
                .regrowths
                .iter_mut()
                .filter(|regrowth| regrowth.next_pulse <= now)
                .map(|regrowth| {
                    regrowth.next_pulse += PULSE_MILLIS;
                    (regrowth.caster, regrowth.target, regrowth.heal)
                })
                .collect()
        };
        for (caster, target, amount) in due {
            if let Some(player) = find_player(server, target) {
                let healed = self.heal_hunter(&player, amount);
                self.record_healing(caster, healed);
            }
        }
    }

    /// Casts the caster's selected spell if it is ready and affordable.
    fn cast(&self, player: &Arc<Player>, now: u64) {
        let uuid = player.gameprofile.id();
        if !holds_grimoire(player) {
            return;
        }
        let gear = self.spell_gear(player);
        let Some(check) = self.with_session_mut(uuid, |session| {
            if session.class.class != Some(HunterClass::Mage) {
                return Err(SpellError::NotAMage.message().to_owned());
            }
            let spell = session
                .spells
                .selected_spell()
                .ok_or_else(|| "Нет подготовленных заклинаний.".to_owned())?;
            let magic = &session.magic;
            if let Some(deadline) = magic.cooldowns.get(&spell)
                && *deadline > now
            {
                return Err(format!(
                    "{}: перезарядка {:.1} с.",
                    spell.info().name,
                    (*deadline - now) as f64 / 1_000.0
                ));
            }
            let wrath = magic.wrath_until > now;
            let free = gear.free_every > 0 && (magic.casts + 1) % gear.free_every == 0;
            let cost = if free {
                0.0
            } else {
                cast_cost(
                    gear,
                    spell,
                    wrath && spell.info().school == School::Destruction,
                )
            };
            let maximum = spell::max_mana(session.stats.intelligence) + gear.mana;
            if magic.mana.unwrap_or(maximum) < cost {
                return Err(format!("Не хватает маны: нужно {}.", cost.round()));
            }
            Ok((spell, cost, wrath, session.stats.rank()))
        }) else {
            return;
        };
        let (spell, cost, wrath, rank) = match check {
            Ok(check) => check,
            Err(message) => {
                player.send_action_bar(TextComponent::plain(message).color(Color::Red));
                return;
            }
        };
        let world = player.get_world();
        let caster = Caster {
            state: self,
            player,
            eye: DVec3::new(player.position().x, player.get_eye_y(), player.position().z),
            look: player.look_angle(),
            world,
            scale: class::rank_scale(rank),
            healing: 1.0 + gear.healing,
            barrier: gear.barrier,
            burn: if gear.burning { 2 } else { 1 },
            power: if wrath && spell.info().school == School::Destruction {
                WRATH_DAMAGE
            } else {
                1.0
            },
            now,
        };
        if let Err(message) = self.perform(&caster, spell, rank) {
            player.send_action_bar(TextComponent::plain(message).color(Color::Red));
            return;
        }
        // A destruction echo strikes again for free; Monarch's Wrath does not
        // stack with itself.
        let echo = spell.info().school == School::Destruction
            && spell != SpellId::MonarchsWrath
            && rand::random_range(0..100) < gear.echo_percent;
        if echo {
            let _ = self.perform(&caster, spell, rank);
        }
        self.with_session_mut(uuid, |session| {
            let maximum = spell::max_mana(session.stats.intelligence) + gear.mana;
            let magic = &mut session.magic;
            magic.mana = Some((magic.mana.unwrap_or(maximum) - cost).max(0.0));
            magic.casts = magic.casts.wrapping_add(1);
            magic.cooldowns.insert(spell, now + gear.cooldown_of(spell));
            magic.cooldowns.retain(|_, deadline| *deadline > now);
        });
        player.send_action_bar(
            TextComponent::plain(spell.info().name.to_owned()).color(Color::LightPurple),
        );
        self.record_goal(uuid, Goal::SpellCast, 1);
    }

    /// Applies one spell; an error leaves mana and the cooldown untouched.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per spell reads better than a table of callbacks"
    )]
    fn perform(
        &self,
        caster: &Caster<'_>,
        spell: SpellId,
        rank: HunterRank,
    ) -> Result<(), &'static str> {
        let world = &caster.world;
        let now = caster.now;
        let scale = caster.scale;
        match spell {
            SpellId::MagicArrow => {
                let shot = caster.aim(AIM_RANGE, false);
                beam(
                    world,
                    &vanilla_particle_types::END_ROD,
                    caster.eye,
                    shot.end,
                );
                if let Some(target) = shot.hits.first() {
                    caster.hurt(target, 6.0);
                }
            }
            SpellId::Fireball => {
                let shot = caster.aim(AIM_RANGE, false);
                let center = shot.hits.first().map_or(shot.end, |hit| hit.position());
                beam(world, &vanilla_particle_types::FLAME, caster.eye, center);
                particles(world, &vanilla_particle_types::EXPLOSION, center, 3, 1.0);
                for target in enemies_near(world, center, 3.0) {
                    caster.hurt(&target, 8.0);
                    target.set_remaining_fire_ticks(80 * caster.burn);
                }
            }
            SpellId::IceSpear => {
                let shot = caster.aim(AIM_RANGE, false);
                beam(
                    world,
                    &vanilla_particle_types::SNOWFLAKE,
                    caster.eye,
                    shot.end,
                );
                if let Some(target) = shot.hits.first() {
                    caster.hurt(target, 10.0);
                    afflict(target, vanilla_mob_effects::SLOWNESS, 60, 2);
                }
            }
            SpellId::ChainLightning => {
                let shot = caster.aim(AIM_RANGE, false);
                let Some(first) = shot.hits.first().cloned() else {
                    beam(
                        world,
                        &vanilla_particle_types::ELECTRIC_SPARK,
                        caster.eye,
                        shot.end,
                    );
                    return Ok(());
                };
                let mut struck = vec![first.id()];
                let mut from = caster.eye;
                let mut current = first;
                let mut damage = 8.0;
                for _ in 0..4 {
                    beam(
                        world,
                        &vanilla_particle_types::ELECTRIC_SPARK,
                        from,
                        current.position() + DVec3::Y,
                    );
                    caster.hurt(&current, damage);
                    damage *= 0.75;
                    from = current.position() + DVec3::Y;
                    let Some(next) = enemies_near(world, current.position(), 6.0)
                        .into_iter()
                        .filter(|enemy| !struck.contains(&enemy.id()))
                        .min_by(|a, b| {
                            let origin = current.position();
                            a.position()
                                .distance_squared(origin)
                                .total_cmp(&b.position().distance_squared(origin))
                        })
                    else {
                        break;
                    };
                    struck.push(next.id());
                    current = next;
                }
            }
            SpellId::ManaStorm => {
                self.storms.lock().push(Storm::new(
                    caster.player.gameprofile.id(),
                    world.key.clone(),
                    caster.player.position(),
                    5.0,
                    4.0 * caster.power,
                    now + 6_000,
                ));
            }
            SpellId::Meteor => {
                let shot = caster.aim(LONG_RANGE, false);
                let center = shot.hits.first().map_or(shot.end, |hit| hit.position());
                particles(world, &vanilla_particle_types::FLAME, center, 40, 2.5);
                self.spell_runtime.lock().meteors.push(Meteor {
                    owner: caster.player.gameprofile.id(),
                    world: world.key.clone(),
                    center,
                    damage: 25.0 * caster.power,
                    burn_ticks: 100 * caster.burn,
                    due: now + 1_500,
                });
            }
            SpellId::VoidLance => {
                let shot = caster.aim(LONG_RANGE, true);
                beam(world, &vanilla_particle_types::PORTAL, caster.eye, shot.end);
                for target in &shot.hits {
                    caster.hurt(target, 30.0);
                }
            }
            SpellId::MonarchsWrath => {
                self.with_session_mut(caster.player.gameprofile.id(), |session| {
                    session.magic.wrath_until = now + 10_000;
                });
                particles(
                    world,
                    &vanilla_particle_types::SOUL_FIRE_FLAME,
                    caster.player.position() + DVec3::Y,
                    40,
                    0.8,
                );
            }
            SpellId::LesserHeal => {
                let target = caster.ally_in_sight();
                let healed = self.heal_hunter(&target, 6.0 * scale * caster.healing);
                self.record_healing(caster.player.gameprofile.id(), healed);
                particles(
                    world,
                    &vanilla_particle_types::HEART,
                    target.position() + DVec3::Y * 2.0,
                    5,
                    0.4,
                );
            }
            SpellId::Restoration => {
                // World queries run before the spell lock is taken.
                let allies = caster.allies(PARTY_RADIUS);
                let mut runtime = self.spell_runtime.lock();
                for ally in allies {
                    runtime.regrowths.push(Regrowth {
                        caster: caster.player.gameprofile.id(),
                        target: ally.gameprofile.id(),
                        heal: scale * caster.healing,
                        until: now + 8_000,
                        next_pulse: now + PULSE_MILLIS,
                    });
                }
                drop(runtime);
                caster.for_allies(|_| {});
            }
            SpellId::Cleanse => caster.for_allies(|ally| {
                for effect in [
                    vanilla_mob_effects::POISON,
                    vanilla_mob_effects::WITHER,
                    vanilla_mob_effects::SLOWNESS,
                    vanilla_mob_effects::WEAKNESS,
                    vanilla_mob_effects::BLINDNESS,
                    vanilla_mob_effects::DARKNESS,
                ] {
                    ally.remove_mob_effect(effect);
                }
                ally.set_remaining_fire_ticks(0);
            }),
            SpellId::Barrier => {
                // Absorption gives 4 health per level.
                let amplifier = (((8.0 * scale + caster.barrier) / 4.0).round() as i32 - 1).max(0);
                caster.for_allies(|ally| {
                    ally.add_mob_effect(MobEffectInstance::with_duration(
                        vanilla_mob_effects::ABSORPTION,
                        160,
                        amplifier,
                    ));
                });
            }
            SpellId::GreaterHeal => {
                let target = caster.ally_in_sight();
                let healed = self.heal_hunter(
                    &target,
                    target.get_max_health() * (0.5 * scale * caster.healing).min(1.0),
                );
                self.record_healing(caster.player.gameprofile.id(), healed);
                particles(
                    world,
                    &vanilla_particle_types::HEART,
                    target.position() + DVec3::Y * 2.0,
                    12,
                    0.6,
                );
            }
            SpellId::Sanctuary => {
                self.spell_runtime.lock().sanctuaries.push(Sanctuary {
                    owner: caster.player.gameprofile.id(),
                    world: world.key.clone(),
                    center: caster.player.position(),
                    heal: scale * caster.healing,
                    until: now + 10_000,
                    next_pulse: now,
                });
            }
            SpellId::LastStand => caster.for_allies(|ally| {
                self.with_session_mut(ally.gameprofile.id(), |session| {
                    session.magic.last_stand_until = now + 6_000;
                });
            }),
            SpellId::Repulse => {
                let center = caster.player.position();
                particles(
                    world,
                    &vanilla_particle_types::CLOUD,
                    center + DVec3::Y,
                    30,
                    2.5,
                );
                for target in enemies_near(world, center, 5.0) {
                    if let Some(living) = target.as_living_entity() {
                        let away = center - target.position();
                        living.knockback(1.5, away.x, away.z);
                    }
                }
            }
            SpellId::Haste => caster.for_allies(|ally| {
                ally.add_mob_effect(MobEffectInstance::with_duration(
                    vanilla_mob_effects::SPEED,
                    200,
                    1,
                ));
            }),
            SpellId::Shackles => {
                let shot = caster.aim(AIM_RANGE, false);
                let target = shot.hits.first().ok_or("Оковы: нет цели в прицеле.")?;
                let boss = self.is_boss(world, target.id());
                afflict(
                    target,
                    vanilla_mob_effects::SLOWNESS,
                    if boss { 20 } else { 60 },
                    6,
                );
                beam(
                    world,
                    &vanilla_particle_types::ENCHANT,
                    caster.eye,
                    target.position() + DVec3::Y,
                );
            }
            SpellId::BlessingOfStrength => caster.for_allies(|ally| {
                self.with_session_mut(ally.gameprofile.id(), |session| {
                    session.magic.blessed_until = now + 30_000;
                });
            }),
            SpellId::Weaken => {
                let center = caster.player.position();
                particles(
                    world,
                    &vanilla_particle_types::WITCH,
                    center + DVec3::Y,
                    30,
                    3.0,
                );
                let targets = enemies_near(world, center, 6.0);
                let mut runtime = self.spell_runtime.lock();
                for target in targets {
                    runtime.weakened.insert(target.id(), now + 8_000);
                }
            }
            SpellId::GravityWell => {
                let shot = caster.aim(AIM_RANGE, false);
                let center = shot.hits.first().map_or(shot.end, |hit| hit.position());
                particles(world, &vanilla_particle_types::PORTAL, center, 60, 3.0);
                for target in enemies_near(world, center, 8.0) {
                    if let Some(living) = target.as_living_entity() {
                        let outward = target.position() - center;
                        living.knockback(1.2, outward.x, outward.z);
                    }
                    afflict(&target, vanilla_mob_effects::SLOWNESS, 80, 1);
                }
            }
            SpellId::NightVision => caster.for_allies(|ally| {
                ally.add_mob_effect(MobEffectInstance::with_duration(
                    vanilla_mob_effects::NIGHT_VISION,
                    3_600,
                    0,
                ));
            }),
            SpellId::Blink => {
                let player = caster.player;
                let start = player.position();
                let direction = caster.look.normalize_or_zero();
                let mut destination = None;
                let steps = (8.0 / TRACE_STEP) as u32;
                for step in 1..=steps {
                    let feet = start + direction * (f64::from(step) * TRACE_STEP);
                    if !passable(world, feet) || !passable(world, feet + DVec3::Y) {
                        break;
                    }
                    destination = Some(feet);
                }
                let destination = destination
                    .filter(|feet| feet.distance(start) >= 1.0)
                    .ok_or("Скачок: путь прегражден.")?;
                let (yaw, pitch) = player.rotation();
                player
                    .teleport(destination, yaw, pitch)
                    .map_err(|_| "Скачок не удался.")?;
                particles(
                    world,
                    &vanilla_particle_types::PORTAL,
                    start + DVec3::Y,
                    30,
                    0.5,
                );
                particles(
                    world,
                    &vanilla_particle_types::PORTAL,
                    destination + DVec3::Y,
                    30,
                    0.5,
                );
            }
            SpellId::Detection => {
                let center = caster.player.position();
                let found = enemies_near(world, center, 32.0);
                for target in &found {
                    afflict(target, vanilla_mob_effects::GLOWING, 300, 0);
                }
                let mut report = format!("Обнаружено мобов: {}.", found.len());
                if let Some(boss) = self.boss_position(world) {
                    let _ = write!(
                        report,
                        " Босс: {} блоков, {}.",
                        boss.distance(center).round(),
                        compass(center, boss)
                    );
                }
                tell(caster.player, &report, Color::Aqua);
            }
            SpellId::Meditation => {
                self.with_session_mut(caster.player.gameprofile.id(), |session| {
                    session.magic.meditation = Some((now, caster.player.position()));
                });
                tell(
                    caster.player,
                    "Медитация: стойте на месте 4 секунды.",
                    Color::Aqua,
                );
            }
            SpellId::ManaShield => {
                self.with_session_mut(caster.player.gameprofile.id(), |session| {
                    session.magic.mana_shield_until = now + 10_000;
                });
                particles(
                    world,
                    &vanilla_particle_types::ENCHANT,
                    caster.player.position() + DVec3::Y,
                    40,
                    0.8,
                );
            }
        }
        let _ = rank;
        Ok(())
    }

    /// Returns the spellcasting bonuses of a Mage's staffs in either hand,
    /// System gear, and serial robe pieces worn in vanilla armor slots; other
    /// classes get none.
    pub(super) fn spell_gear(&self, player: &Player) -> SpellGear {
        let Some((class, rank, worn)) = self.with_session_mut(player.gameprofile.id(), |session| {
            (
                session.class.class,
                session.stats.rank(),
                session.gear.clone(),
            )
        }) else {
            return SpellGear::default();
        };
        if class != Some(HunterClass::Mage) {
            return SpellGear::default();
        }
        let (main, off): (ItemStack, ItemStack) = {
            let inventory = player.inventory.lock();
            (
                inventory
                    .get_item_in_hand(InteractionHand::MainHand)
                    .clone(),
                inventory.get_item_in_hand(InteractionHand::OffHand).clone(),
            )
        };
        let hands = [main, off];
        let artifact_staffs = hands.iter().filter(|stack| {
            artifact::held(stack).is_some_and(|held| held.category == Category::Weapon)
        });
        let serial_staffs = hands
            .iter()
            .filter(|stack| serial::held(stack).is_some_and(|serial| serial.spell_gear.is_some()));
        let physical_robes = [
            EquipmentSlot::Head,
            EquipmentSlot::Chest,
            EquipmentSlot::Legs,
            EquipmentSlot::Feet,
        ]
        .into_iter()
        .filter_map(|slot| {
            let mut stack = ItemStack::empty();
            player.with_equipment_slot(slot, &mut |item| stack = item.clone());
            serial::of(&stack)
        })
        .filter(|id| id.line() == Some(crate::item::artifact::ArmorLine::Robe));
        let artifact_gear = worn
            .iter()
            .filter(|stack| gear::wearable(stack, class))
            .filter(|stack| artifact::usable_by(stack, rank))
            .filter_map(|stack| artifact::of(stack)?.spell_gear());
        let artifact_staff_spell_gear = artifact_staffs
            .filter(|stack| artifact::usable_by(stack, rank))
            .filter_map(|stack| artifact::of(stack)?.spell_gear());
        let serial_staff_spell_gear = serial_staffs
            .filter(|stack| artifact::usable_by(stack, rank))
            .filter_map(|stack| serial::of(stack)?.spell_gear());
        artifact_staff_spell_gear
            .chain(serial_staff_spell_gear)
            .chain(artifact_gear)
            .chain(physical_robes.filter_map(|id| id.spell_gear()))
            .fold(
                self.enchant_spell_gear(player.gameprofile.id()),
                SpellGear::combine,
            )
    }

    /// Returns whether `entity` is the boss of a run in `world`.
    pub(super) fn is_boss(&self, world: &World, entity: i32) -> bool {
        self.dungeon_runtime.lock().boss_entity(world) == Some(entity)
    }

    /// Returns where the boss of a run in `world` stands.
    fn boss_position(&self, world: &World) -> Option<DVec3> {
        let boss = self.dungeon_runtime.lock().boss_entity(world)?;
        world.get_entity_by_id(boss).map(|entity| entity.position())
    }

    /// Adjusts damage for the spell effects on the attacker and the victim.
    ///
    /// Called from the class damage hook after class bonuses.
    pub(super) fn spell_damage(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        let now = now_millis();
        let mut damage = amount;
        match source.causing_entity() {
            Some(attacker) if attacker.as_player().is_some() => {
                let uuid = attacker.as_player().map(|player| player.gameprofile.id());
                let blessed = uuid
                    .and_then(|uuid| {
                        self.with_session_mut(uuid, |session| session.magic.blessed_until > now)
                    })
                    .unwrap_or(false);
                if blessed {
                    damage *= 1.0 + BLESSING_BONUS;
                }
            }
            Some(attacker) => {
                let weakened = self
                    .spell_runtime
                    .lock()
                    .weakened
                    .get(&attacker.id())
                    .is_some_and(|until| *until > now);
                if weakened {
                    damage *= WEAKEN_FACTOR;
                }
            }
            None => {}
        }
        if let Some(player) = victim.as_player() {
            damage = self
                .with_session_mut(player.gameprofile.id(), |session| {
                    let magic = &mut session.magic;
                    magic.last_hurt = now;
                    magic.meditation = None;
                    let mut damage = damage;
                    if magic.ward_until > now {
                        damage *= WARD_FACTOR;
                    }
                    if magic.mana_shield_until > now
                        && let Some(mana) = magic.mana.as_mut()
                    {
                        let absorbed = damage.min(*mana / SHIELD_MANA_PER_HEALTH);
                        *mana -= absorbed * SHIELD_MANA_PER_HEALTH;
                        damage -= absorbed;
                    }
                    damage
                })
                .unwrap_or(damage);
        }
        damage
    }

    /// Keeps a hunter under Last Stand alive at one health.
    pub fn spell_prevent_death(&self, victim: &dyn LivingEntity) -> bool {
        let Some(player) = victim.as_player() else {
            return false;
        };
        let now = now_millis();
        let standing = self
            .with_session_mut(player.gameprofile.id(), |session| {
                session.magic.last_stand_until > now
            })
            .unwrap_or(false);
        if standing {
            player.set_health(1.0);
        }
        standing
    }
}

/// What one mana tick asks the player to show.
enum ManaTick {
    /// The hunter stopped being a Mage: reset the experience bar.
    Cleared,
    /// A Mage's mana and hints.
    Mage {
        mana: f32,
        maximum: f32,
        refresh: bool,
        grimoire: bool,
        learned: bool,
        note: Option<(&'static str, Color)>,
        hint: Option<(SpellId, u64, f32)>,
    },
}

/// Returns the mana a cast of `spell` costs with the caster's artifacts.
fn cast_cost(gear: SpellGear, spell: SpellId, wrath: bool) -> f32 {
    let cost = gear.cost_of(spell);
    if wrath { cost * WRATH_MANA } else { cost }
}

/// Returns whether `player` holds the grimoire in either hand.
fn holds_grimoire(player: &Player) -> bool {
    let inventory = player.inventory.lock();
    [InteractionHand::MainHand, InteractionHand::OffHand]
        .into_iter()
        .any(|hand| grimoire::is_grimoire(inventory.get_item_in_hand(hand)))
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::{cast_cost, compass};
    use crate::spell::{SpellGear, SpellId};

    #[test]
    fn wrath_halves_the_cost_and_compass_points_the_right_way() {
        let gear = SpellGear::default();
        assert!((cast_cost(gear, SpellId::Fireball, true) - 12.5).abs() < f32::EPSILON);
        assert!((cast_cost(gear, SpellId::Fireball, false) - 25.0).abs() < f32::EPSILON);
        assert_eq!(compass(DVec3::ZERO, DVec3::new(0.0, 0.0, -10.0)), "север");
        assert_eq!(compass(DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0)), "восток");
        assert_eq!(compass(DVec3::ZERO, DVec3::new(0.0, 0.0, 10.0)), "юг");
    }
}
