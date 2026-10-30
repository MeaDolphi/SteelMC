//! Artifact abilities applied to live entities.
//!
//! Only a successful direct melee hit by a player triggers anything; storm
//! damage is magic caused by the owner but not dealt directly by them, so it
//! can never trigger another ability.

use std::mem;
use std::sync::Arc;

use glam::DVec3;
use steel_core::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{LivingEntity, SharedEntity};
use steel_core::inventory::equipment::EquipmentSlot;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_core::server::Server;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::{vanilla_attributes, vanilla_damage_types, vanilla_particle_types};
use steel_utils::Identifier;
use steel_utils::geometry::WorldAabb;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::SoloLevelingState;
use crate::abilities::{self, AbilityEffect};
use crate::dungeon::runtime::find_player;
use crate::gear;
use crate::identifier::from_sololeveling;
use crate::item::artifact::{self, Ability, Category};
use crate::item::enchantment::{self, Effect};

/// Milliseconds between two storm damage pulses.
const STORM_PULSE_MILLIS: u64 = 1_000;
/// Armor slots whose artifacts are checked against the wearer's rank.
const ARMOR_SLOTS: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];

/// A running Demon King's Longsword storm.
#[derive(Debug)]
pub(super) struct Storm {
    owner: Uuid,
    world: Identifier,
    center: DVec3,
    radius: f64,
    damage: f32,
    until: u64,
    next_pulse: u64,
}

impl Storm {
    /// Starts a storm of `owner` at `center` that pulses at once.
    pub(super) const fn new(
        owner: Uuid,
        world: Identifier,
        center: DVec3,
        radius: f64,
        damage: f32,
        until: u64,
    ) -> Self {
        Self {
            owner,
            world,
            center,
            radius,
            damage,
            until,
            next_pulse: 0,
        }
    }
}

fn equipped(entity: &dyn LivingEntity, slot: EquipmentSlot) -> ItemStack {
    let mut stack = ItemStack::empty();
    entity.with_equipment_slot(slot, &mut |item| stack = item.clone());
    stack
}

fn speed_id() -> Identifier {
    from_sololeveling("artifact_speed_on_hit")
}

fn shield_id(source: &str) -> Identifier {
    from_sololeveling(format!("artifact_shield_{source}"))
}

/// Returns the player who dealt `source` with their own hand, if any.
fn melee_player(source: &DamageSource) -> Option<&Player> {
    if !source.is_direct() {
        return None;
    }
    source.causing_entity()?.as_player()
}

impl SoloLevelingState {
    /// Returns the usable abilities of the artifact in `slot`.
    ///
    /// A main-hand slot only counts weapons, so armor held in the hand gives
    /// nothing; an artifact above the wearer's rank gives nothing either.
    fn slot_abilities(&self, entity: &Player, slot: EquipmentSlot) -> &'static [Ability] {
        let stack = equipped(entity, slot);
        let Some(artifact) = artifact::held(&stack) else {
            return &[];
        };
        let weapon_slot = slot == EquipmentSlot::MainHand;
        if weapon_slot != (artifact.category == Category::Weapon) {
            return &[];
        }
        self.slot_abilities_of(entity, &stack)
    }

    /// Returns the abilities of a held `stack` its holder may use.
    fn slot_abilities_of(&self, entity: &Player, stack: &ItemStack) -> &'static [Ability] {
        let Some(artifact) = artifact::held(stack) else {
            return &[];
        };
        let stack = stack.clone();
        let usable = self
            .hunter_rank(entity.gameprofile.id())
            .is_some_and(|rank| artifact::usable_by(&stack, rank))
            && gear::wearable(&stack, self.class_of(entity.gameprofile.id()));
        if usable { artifact.abilities } else { &[] }
    }

    /// Returns the outgoing magic-damage bonus of the held weapon and the
    /// System gear of `player`, as a fraction.
    ///
    /// A Mage staff also counts from the off hand, where it sits while the
    /// grimoire is in the main hand.
    pub(super) fn magic_focus(&self, player: &Player) -> f32 {
        let off_hand = equipped(player, EquipmentSlot::OffHand);
        let off_hand_staff = if artifact::of(&off_hand).is_some_and(|id| id.spell_gear().is_some())
        {
            self.slot_abilities_of(player, &off_hand)
        } else {
            &[]
        };
        self.slot_abilities(player, EquipmentSlot::MainHand)
            .iter()
            .chain(off_hand_staff)
            .copied()
            .chain(self.gear_abilities(player))
            .map(|ability| match ability {
                Ability::MagicFocus { percent } => percent,
                _ => 0.0,
            })
            .sum()
    }

    /// Returns the abilities of the System gear `player` can use.
    pub(super) fn gear_abilities(&self, player: &Player) -> Vec<Ability> {
        self.with_session_mut(player.gameprofile.id(), |session| {
            gear::abilities(&session.gear, session.stats.rank(), session.class.class)
        })
        .unwrap_or_default()
    }

    /// Forgets the bonuses applied to a player entity that was replaced, as
    /// after a respawn, so the next tick applies them to the new one.
    pub fn reset_gear_bonuses(&self, uuid: Uuid) {
        self.with_session_mut(uuid, |session| {
            session.gear_applied.clear();
            session.gear_dirty = true;
        });
    }

    /// Applies the attribute bonuses of the System gear, replacing the last
    /// ones; runs when the gear changed and now and then for rank changes.
    fn refresh_gear(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let periodic = player.tick_count() % 100 == 0;
        let Some((items, rank, class, strength, previous)) = self
            .with_session_mut(uuid, |session| {
                if !session.gear_dirty && !periodic {
                    return None;
                }
                session.gear_dirty = false;
                Some((
                    session.gear.clone(),
                    session.stats.rank(),
                    session.class.class,
                    session.stats.strength,
                    mem::take(&mut session.gear_applied),
                ))
            })
            .flatten()
        else {
            return;
        };
        let bonuses = gear::modifiers(&items, rank, class, strength);
        let applied = bonuses
            .iter()
            .map(|(attribute, modifier)| (*attribute, modifier.id.clone()))
            .collect::<Vec<_>>();
        {
            let mut attributes = player.attributes().lock();
            for (attribute, id) in &previous {
                if !applied
                    .iter()
                    .any(|(kept, kept_id)| kept.key == attribute.key && kept_id == id)
                {
                    attributes.remove_modifier(attribute, id);
                }
            }
            for (attribute, modifier) in bonuses {
                attributes.set_modifier(attribute, modifier, false);
            }
        }
        // Lower maximum health must not leave health above it.
        if player.get_health() > player.get_max_health() {
            player.set_health(player.get_max_health());
        }
        self.with_session_mut(uuid, |session| session.gear_applied = applied);
    }

    /// Adjusts damage before vanilla armor runs, for both sides of a hit.
    pub fn artifact_damage(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        let mut damage = amount;
        if let Some(attacker) = melee_player(source) {
            let weapon = equipped(attacker, EquipmentSlot::MainHand);
            let unusable = artifact::of(&weapon).is_some()
                && !self
                    .hunter_rank(attacker.gameprofile.id())
                    .is_some_and(|rank| artifact::usable_by(&weapon, rank));
            if unusable {
                // A weapon above the wielder's rank strikes like a bare hand.
                damage = damage.min(1.0);
            } else {
                let strength = self
                    .with_session_mut(attacker.gameprofile.id(), |session| session.stats.strength)
                    .unwrap_or(0);
                let armored = victim
                    .attributes()
                    .lock()
                    .get_value(vanilla_attributes::ARMOR)
                    .is_some_and(|armor| armor > 0.0);
                // Rings and amulets add their damage abilities to the weapon's.
                let mut held = self
                    .slot_abilities(attacker, EquipmentSlot::MainHand)
                    .to_vec();
                held.extend(self.gear_abilities(attacker));
                damage = abilities::outgoing_damage(&held, strength, armored, damage);
            }
        }
        if let Some(player) = victim.as_player() {
            let gear = self.gear_abilities(player);
            if !gear.is_empty() {
                let fraction = victim.get_health() / victim.get_max_health().max(1.0);
                let now = abilities::now_millis();
                damage = self
                    .with_session_mut(player.gameprofile.id(), |session| {
                        abilities::incoming_damage(
                            &gear,
                            &mut session.cooldowns,
                            &mut session.abilities,
                            fraction,
                            now,
                            damage,
                        )
                    })
                    .unwrap_or(damage);
            }
        }
        damage
    }

    /// Triggers on-hit abilities after a melee hit that took health.
    pub fn artifact_hit(&self, victim: &dyn LivingEntity, source: &DamageSource, amount: f32) {
        let Some(attacker) = melee_player(source) else {
            return;
        };
        self.enchantment_hit(attacker, amount);
        let uuid = attacker.gameprofile.id();
        let mut held = self
            .slot_abilities(attacker, EquipmentSlot::MainHand)
            .to_vec();
        held.extend(self.gear_abilities(attacker));
        if held.is_empty() {
            return;
        }
        let now = abilities::now_millis();
        let Some(effects) = self.with_session_mut(uuid, |session| {
            let before = session.cooldowns.clone();
            let result =
                abilities::on_hit(&held, &mut session.cooldowns, &mut session.abilities, now);
            session
                .cooldowns
                .shorten_started(&before, now, session.enchant.summary.focus);
            result
        }) else {
            return;
        };
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            self.apply_effect(attacker, victim, effect, now);
        }
        self.queue_ability_save(uuid);
    }

    /// Applies the on-hit enchantments of the wielded weapon.
    ///
    /// Recoil is applied straight to health rather than as damage, so it can
    /// never kill, never trigger another hook, and never count as a hit.
    fn enchantment_hit(&self, attacker: &Player, amount: f32) {
        let weapon = equipped(attacker, EquipmentSlot::MainHand);
        if artifact::held(&weapon).is_none_or(|artifact| artifact.category != Category::Weapon)
            || !self
                .hunter_rank(attacker.gameprofile.id())
                .is_some_and(|rank| artifact::usable_by(&weapon, rank))
        {
            return;
        }
        let summary = self.enchant_summary(attacker.gameprofile.id());
        let mut health = attacker.get_health();
        for enchantment in artifact::enchantments_of(&weapon) {
            match enchantment.effect() {
                Effect::Lifesteal => {
                    health = (health
                        + summary.healed(amount.max(0.0) * enchantment::LIFESTEAL_FRACTION))
                    .min(attacker.get_max_health());
                }
                Effect::Recoil => {
                    health = (health - enchantment::RECOIL_DAMAGE).max(1.0);
                }
                Effect::Attribute { .. } | Effect::Mechanic => {}
            }
        }
        if (health - attacker.get_health()).abs() > f32::EPSILON {
            attacker.set_health(health);
        }
    }

    /// Survives a lethal hit with the Red Knight's Helmet.
    pub fn artifact_prevent_death(&self, victim: &dyn LivingEntity, source: &DamageSource) -> bool {
        if source.bypasses_invulnerability() {
            return false;
        }
        let Some(player) = victim.as_player() else {
            return false;
        };
        let gear = self.gear_abilities(player);
        if gear.is_empty() {
            return false;
        }
        let uuid = player.gameprofile.id();
        let now = abilities::now_millis();
        let Some((health, shield)) = self
            .with_session_mut(uuid, |session| {
                let before = session.cooldowns.clone();
                let result = abilities::on_lethal(
                    &gear,
                    &mut session.cooldowns,
                    &mut session.abilities,
                    now,
                );
                session
                    .cooldowns
                    .shorten_started(&before, now, session.enchant.summary.focus);
                result
            })
            .flatten()
        else {
            return false;
        };
        player.set_health(health);
        self.apply_effect(player, victim, shield, now);
        player.send_message(
            &TextComponent::plain("Шлем красного рыцаря спас вас от смерти.").color(Color::Red),
        );
        self.queue_ability_save(uuid);
        true
    }

    fn apply_effect(
        &self,
        holder: &Player,
        target: &dyn LivingEntity,
        effect: AbilityEffect,
        now: u64,
    ) {
        match effect {
            AbilityEffect::Speed { percent, .. } => {
                holder.attributes().lock().set_modifier(
                    vanilla_attributes::MOVEMENT_SPEED,
                    AttributeModifier {
                        id: speed_id(),
                        amount: percent,
                        operation: AttributeModifierOperation::AddMultipliedTotal,
                    },
                    false,
                );
            }
            AbilityEffect::Shield { source, health, .. } => {
                holder.attributes().lock().set_modifier(
                    vanilla_attributes::MAX_ABSORPTION,
                    AttributeModifier {
                        id: shield_id(source),
                        amount: f64::from(health),
                        operation: AttributeModifierOperation::AddValue,
                    },
                    false,
                );
                holder.set_absorption_amount(holder.get_absorption_amount() + health);
            }
            AbilityEffect::Storm {
                radius,
                damage_per_second,
                until,
            } => {
                self.storms.lock().push(Storm {
                    owner: holder.gameprofile.id(),
                    world: holder.get_world().key.clone(),
                    center: target.position(),
                    radius,
                    damage: damage_per_second,
                    until,
                    next_pulse: now,
                });
            }
        }
    }

    /// Ends expired effects, applies the System gear and unequips armor
    /// above the wearer's rank.
    pub fn tick_artifacts(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        self.refresh_gear(player);
        let now = abilities::now_millis();
        if let Some((speed, shields)) =
            self.with_session_mut(uuid, |session| session.abilities.expire(now))
        {
            let mut attributes = player.attributes().lock();
            if speed {
                attributes.remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &speed_id());
            }
            for source in shields {
                attributes.remove_modifier(vanilla_attributes::MAX_ABSORPTION, &shield_id(source));
            }
        }

        let Some(rank) = self.hunter_rank(uuid) else {
            return;
        };
        // A weapon or tool above the holder's rank does nothing at all, which
        // looks like a bug unless the holder is told why.
        if player.tick_count() % 20 == 0 {
            let held = equipped(player, EquipmentSlot::MainHand);
            if let Some(required) = artifact::required_rank(&held)
                && rank < required
            {
                player.send_action_bar(
                    TextComponent::plain(format!(
                        "Артефакт не действует: нужен ранг {}, ваш ранг {}",
                        required.name(),
                        rank.name()
                    ))
                    .color(Color::Red),
                );
            }
        }
        for slot in ARMOR_SLOTS {
            let mut removed = None;
            player.with_equipment_slot_mut(slot, &mut |stack| {
                if artifact::of(stack).is_some() && !artifact::usable_by(stack, rank) {
                    removed = Some(mem::replace(stack, ItemStack::empty()));
                }
            });
            if let Some(stack) = removed {
                player.send_message(
                    &TextComponent::plain(format!(
                        "Ваш ранг {} слишком низок для этого артефакта (нужен {}): он снят.",
                        rank.name(),
                        artifact::required_rank(&stack).map_or("?", |required| required.name())
                    ))
                    .color(Color::Red),
                );
                player.add_item_or_drop(stack, Prediction::ServerOnly);
            }
        }
    }

    /// Deals storm damage once a second to every mob inside a running storm.
    ///
    /// Players and summoned shadows are never hit, and a storm whose owner left
    /// ends at once.
    pub fn tick_storms(&self, server: &Server) {
        let now = abilities::now_millis();
        let mut storms = self.storms.lock();
        storms.retain(|storm| storm.until > now);
        let due: Vec<(Uuid, Identifier, DVec3, f64, f32)> = storms
            .iter_mut()
            .filter(|storm| storm.next_pulse <= now)
            .map(|storm| {
                storm.next_pulse = storm.next_pulse.saturating_add(STORM_PULSE_MILLIS);
                (
                    storm.owner,
                    storm.world.clone(),
                    storm.center,
                    storm.radius,
                    storm.damage,
                )
            })
            .collect();
        drop(storms);

        for (owner, world_key, center, radius, damage) in due {
            let Some(player) = find_player(server, owner) else {
                continue;
            };
            let Some(world) = server.worlds.get(&world_key) else {
                continue;
            };
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::SWEEP_ATTACK),
                center,
                12,
                DVec3::new(radius, 0.5, radius),
                0.0,
            );
            let area = WorldAabb::of_size(center, radius * 2.0, radius * 2.0, radius * 2.0);
            let targets = world.get_entities_in_aabb_matching(&area, |entity| {
                entity.as_player().is_none()
                    && entity
                        .as_living_entity()
                        .is_some_and(|living| !living.is_dead_or_dying())
                    && entity
                        .as_mob()
                        .is_none_or(|mob| mob.allied_owner().is_none())
                    && entity.position().distance_squared(center) <= radius * radius
            });
            let cause: SharedEntity = Arc::<Player>::clone(&player);
            let source =
                DamageSource::environment(&vanilla_damage_types::MAGIC).with_causing_entity(cause);
            for target in targets {
                if let Some(living) = target.as_living_entity() {
                    living.hurt(world, &source, damage);
                }
            }
        }
    }

    /// Returns the extra crystals the held pickaxe adds to one mined ore.
    pub fn crystal_bonus(&self, player: &Player) -> i32 {
        abilities::extra_crystals(
            self.slot_abilities(player, EquipmentSlot::MainHand),
            rand::random_range(0..100),
        )
    }

    fn queue_ability_save(&self, uuid: Uuid) {
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist ability cooldowns of {uuid}: {error}");
        }
    }
}
