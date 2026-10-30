//! Hunter classes applied to live players.

use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity};
use steel_core::player::Player;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use uuid::Uuid;

use super::SoloLevelingState;
use crate::abilities;
use crate::class::{self, ClassError, HunterClass};

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
        let (price, choice) = self
            .with_session_mut(uuid, |session| {
                if session.economic_busy {
                    return Err(ClassError::Storage);
                }
                let rank = session.stats.rank();
                let price = session.class.choose(class, rank, now, &mut session.won)?;
                Ok((price, session.class))
            })
            .ok_or(ClassError::Storage)??;
        choice.apply_to(&mut player.attributes().lock());
        if let Err(error) = self.save_session(uuid) {
            log::warn!("sololeveling: failed to persist class of {uuid}: {error}");
        }
        Ok(price)
    }

    /// Re-applies class attribute modifiers after join or respawn.
    pub fn apply_class(&self, player: &Player) {
        let choice = self
            .with_session_mut(player.gameprofile.id(), |session| session.class)
            .unwrap_or_default();
        choice.apply_to(&mut player.attributes().lock());
    }

    /// Adjusts damage for the classes of the attacker and the victim.
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
            let bonus = match self.class_of(attacker.gameprofile.id()) {
                Some(HunterClass::Fighter) if source.is_direct() => class::FIGHTER_MELEE_BONUS,
                Some(HunterClass::Assassin)
                    if source.is_direct()
                        && class::is_behind(
                            victim.position(),
                            victim.look_angle(),
                            attacker.position(),
                        ) =>
                {
                    class::ASSASSIN_BACKSTAB_BONUS
                }
                Some(HunterClass::Ranger) if source.is(&DamageTypeTag::IS_PROJECTILE) => {
                    class::RANGER_PROJECTILE_BONUS
                }
                Some(HunterClass::Mage) if is_magic(source) => class::MAGE_MAGIC_BONUS,
                _ => 0.0,
            };
            damage *= 1.0 + bonus;
        }
        if let Some(player) = victim.as_player()
            && self.class_of(player.gameprofile.id()) == Some(HunterClass::Tanker)
        {
            damage *= 1.0 - class::TANKER_DAMAGE_REDUCTION;
        }
        damage
    }

    /// Runs the Healer pulse for one hunter.
    pub fn tick_class(&self, player: &Player) {
        let uuid = player.gameprofile.id();
        let now = abilities::now_millis();
        let due = self
            .with_session_mut(uuid, |session| {
                if session.class.class != Some(HunterClass::Healer) || now < session.class_pulse_at
                {
                    return false;
                }
                session.class_pulse_at = now + class::HEALER_PULSE_MILLIS;
                true
            })
            .unwrap_or(false);
        if !due || player.is_dead_or_dying() {
            return;
        }
        let members = self
            .party_of(uuid)
            .map_or_else(|| vec![uuid], |party| party.members().to_vec());
        let world = player.get_world();
        let center = player.position();
        let radius = class::HEALER_RADIUS * class::HEALER_RADIUS;
        for member in members {
            let Some(target) = world.players.get_by_uuid(&member) else {
                continue;
            };
            if target.is_dead_or_dying() || target.position().distance_squared(center) > radius {
                continue;
            }
            let health =
                (target.get_health() + class::HEALER_PULSE_HEALTH).min(target.get_max_health());
            if health > target.get_health() {
                target.set_health(health);
            }
        }
    }
}
