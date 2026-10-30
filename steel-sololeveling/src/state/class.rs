//! Hunter classes applied to live players.

use std::mem;

use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity};
use steel_core::player::Player;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use uuid::Uuid;

use super::SoloLevelingState;
use crate::abilities;
use crate::class::{self, ClassError, HunterClass};
use crate::gear;
use crate::quest_book::Goal;
use crate::rank::HunterRank;

/// Returns whether `source` is magic: artifact storms and potion-style damage.
fn is_magic(source: &DamageSource) -> bool {
    matches!(
        source.damage_type.key.path.as_ref(),
        "magic" | "indirect_magic"
    )
}

impl SoloLevelingState {
    /// Returns the class a live hunter has chosen.
    pub fn class_of(&self, uuid: Uuid) -> Option<HunterClass> {
        self.with_session_mut(uuid, |session| session.class.class)
            .flatten()
    }

    /// Picks or changes a hunter's class and persists it with the Won debit.
    ///
    /// Class and balance live in the same account document, so one save
    /// carries both.
    pub fn choose_class(&self, player: &Player, class: HunterClass) -> Result<u64, ClassError> {
        let uuid = player.gameprofile.id();
        if !self.persistence.lock().accepts_writes(uuid) {
            return Err(ClassError::Storage);
        }
        let now = abilities::now_millis() / 1_000;
        let (price, choice, rank) = self
            .with_session_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(ClassError::Storage);
                }
                let rank = session.stats.rank();
                let price = session.class.choose(class, rank, now, &mut session.won)?;
                session.class_rank = Some(rank);
                if class == HunterClass::Mage {
                    session.spells.learn_starting();
                    session.magic.grimoire_due = true;
                }
                Ok((price, session.class, rank))
            })
            .ok_or(ClassError::Storage)??;
        choice.apply_to(&mut player.attributes().lock(), rank);
        self.record_goal(uuid, Goal::ClassChosen, 1);
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist class of {uuid}: {error}");
        }
        Ok(price)
    }

    /// Re-applies class attribute modifiers after join or respawn.
    pub fn apply_class(&self, player: &Player) {
        let Some((choice, rank)) = self.with_session_mut(player.gameprofile.id(), |session| {
            let rank = session.stats.rank();
            session.class_rank = Some(rank);
            (session.class, rank)
        }) else {
            return;
        };
        choice.apply_to(&mut player.attributes().lock(), rank);
    }

    /// Adjusts damage for the classes of the attacker and the victim.
    ///
    /// Bonuses add up before they multiply the hit. The Fighter's lifesteal
    /// is owed here and paid on the next player tick, outside the damage
    /// callback.
    pub fn class_damage(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        let mut damage = amount;
        if let Some(attacker) = source
            .causing_entity()
            .and_then(|entity| entity.as_player())
        {
            let uuid = attacker.gameprofile.id();
            let (class, rank, gear_magic) = self
                .with_session_mut(uuid, |session| {
                    (
                        session.class.class,
                        session.stats.rank(),
                        gear::magic_bonus(
                            &session.gear,
                            session.class.class,
                            session.stats.strength,
                        ),
                    )
                })
                .unwrap_or((None, HunterRank::E, 0.0));
            let scale = class::rank_scale(rank);
            let melee = source.is_direct();
            let magic = is_magic(source);
            // Only magic hits read the held items for magic bonuses.
            let magic_bonus = if magic {
                gear_magic
                    + self.magic_focus(attacker)
                    + self.spell_gear(attacker).magic
                    + if class == Some(HunterClass::Mage) {
                        class::MAGE_MAGIC_BONUS * scale
                    } else {
                        0.0
                    }
            } else {
                0.0
            };
            let class_bonus = match class {
                Some(HunterClass::Fighter) if melee => class::FIGHTER_MELEE_BONUS * scale,
                Some(HunterClass::Assassin) if melee => {
                    let behind = class::is_behind(
                        victim.position(),
                        victim.look_angle(),
                        attacker.position(),
                    );
                    class::ASSASSIN_MELEE_BONUS * scale
                        + if behind {
                            class::ASSASSIN_BACKSTAB_BONUS * scale
                        } else {
                            0.0
                        }
                }
                Some(HunterClass::Ranger) if source.is(&DamageTypeTag::IS_PROJECTILE) => {
                    class::RANGER_PROJECTILE_BONUS * scale
                }
                _ => 0.0,
            };
            damage *= 1.0 + class_bonus + magic_bonus;
            if class == Some(HunterClass::Fighter) && melee {
                let heal = damage * class::FIGHTER_LIFESTEAL * scale;
                self.with_session_mut(uuid, |session| session.class_heal += heal);
            }
        }
        damage = self.spell_damage(victim, source, damage);
        damage = self.enchant_damage(victim, source, damage);
        if let Some(player) = victim.as_player() {
            let tank_rank = self
                .with_session_mut(player.gameprofile.id(), |session| {
                    (session.class.class == Some(HunterClass::Tanker)).then(|| session.stats.rank())
                })
                .flatten();
            if let Some(rank) = tank_rank {
                damage *= 1.0 - class::tanker_reduction(rank);
            }
        }
        damage
    }

    /// Pays owed lifesteal and keeps the class bonuses in step with the rank.
    pub fn tick_class(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let Some((choice, rank, rank_changed, heal)) = self.with_session_mut(uuid, |session| {
            let rank = session.stats.rank();
            let rank_changed = session.class_rank != Some(rank);
            session.class_rank = Some(rank);
            let heal = mem::take(&mut session.class_heal);
            (session.class, rank, rank_changed, heal)
        }) else {
            return;
        };
        if rank_changed {
            choice.apply_to(&mut player.attributes().lock(), rank);
        }
        if heal > 0.0 {
            let _ = self.heal_hunter(player, heal);
        }
        self.tick_mana(player);
        self.tick_enchantments(player);
        self.tick_quests(player);
    }
}
