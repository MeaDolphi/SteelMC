//! Held shield blocking with a renewable guard resource, not item durability.

use std::sync::Arc;

use glam::DVec3;
use steel_core::behavior::InteractionResult;
use steel_core::entity::damage::DamageSource;
use steel_core::entity::{Entity as _, LivingEntity};
use steel_core::player::Player;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::{sound_events, vanilla_items};
use steel_utils::types::InteractionHand;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::abilities::now_millis;
use crate::item::serial::{self, SerialId};
use crate::sound::play_at;
use crate::state::SoloLevelingState;

const MAX_GUARD: f32 = 100.0;
const MIN_START_GUARD: f32 = 25.0;
const DRAIN_PER_SECOND: f32 = 25.0;
const REGEN_PER_SECOND: f32 = 20.0;
const RELEASE_COOLDOWN_MILLIS: u64 = 1_000;
const DAMAGE_FACTOR: f32 = 0.30;
const KNOCKBACK_FACTOR: f64 = 0.25;

/// Live shield-guard state of one hunter; never persisted with the account.
#[derive(Debug, Clone)]
pub struct ShieldGuard {
    stamina: f32,
    last_tick: u64,
    recharge_at: u64,
    exhausted: bool,
    blocking: bool,
}

impl ShieldGuard {
    /// A full guard of a newly joined hunter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stamina: MAX_GUARD,
            last_tick: 0,
            recharge_at: 0,
            exhausted: false,
            blocking: false,
        }
    }
}

impl Default for ShieldGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardTick {
    Idle,
    Blocking,
    Exhausted,
}

impl ShieldGuard {
    fn can_start(&self, now: u64) -> bool {
        !self.blocking
            && !self.exhausted
            && self.stamina >= MIN_START_GUARD
            && now >= self.recharge_at
    }

    fn start(&mut self, now: u64) {
        self.last_tick = now;
        self.blocking = true;
    }

    fn tick(&mut self, active: bool, now: u64) -> GuardTick {
        let elapsed = now.saturating_sub(self.last_tick).min(1_000) as f32 / 1_000.0;
        self.last_tick = now;
        if active && !self.exhausted {
            self.blocking = true;
            self.stamina = (self.stamina - DRAIN_PER_SECOND * elapsed).max(0.0);
            if self.stamina <= 0.0 {
                self.exhausted = true;
                self.blocking = false;
                self.recharge_at = now;
                return GuardTick::Exhausted;
            }
            return GuardTick::Blocking;
        }
        if self.blocking {
            self.blocking = false;
            self.recharge_at = now + RELEASE_COOLDOWN_MILLIS;
        }
        if now >= self.recharge_at {
            self.stamina = (self.stamina + REGEN_PER_SECOND * elapsed).min(MAX_GUARD);
            if self.stamina >= MAX_GUARD {
                self.exhausted = false;
            }
        }
        GuardTick::Idle
    }

    fn is_blocking(&self) -> bool {
        self.blocking && self.stamina > 0.0 && !self.exhausted
    }

    fn stamina(&self) -> f32 {
        self.stamina
    }
}

pub(crate) fn is_shield(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::SHIELD) || serial::of(stack) == Some(SerialId::HunterShield)
}

fn active_shield(player: &Player) -> bool {
    let Some(hand) = player.active_item_use_hand() else {
        return false;
    };
    let inventory = player.inventory.lock();
    is_shield(inventory.get_item_in_hand(hand))
}

impl SoloLevelingState {
    /// Starts a held shield block, if the guard has enough charge.
    pub fn use_shield(&self, player: &Arc<Player>, hand: InteractionHand) -> InteractionResult {
        if player.active_item_use_hand().is_some() {
            return InteractionResult::Success;
        }
        let now = now_millis();
        let Some((allowed, exhausted, ready_at)) =
            self.with_session_mut(player.gameprofile.id(), |session| {
                let guard = &session.shield;
                (guard.can_start(now), guard.exhausted, guard.recharge_at)
            })
        else {
            return InteractionResult::Fail;
        };
        if !allowed {
            let text = if exhausted {
                "Прочность щита истощена: нужно полное восстановление.".to_owned()
            } else if ready_at > now {
                format!(
                    "Щит восстанавливается: {:.1} с.",
                    (ready_at - now) as f64 / 1_000.0
                )
            } else {
                format!("Для поднятия щита нужно {MIN_START_GUARD:.0} прочности.")
            };
            player.send_action_bar(TextComponent::plain(text).color(Color::Red));
            return InteractionResult::Fail;
        }
        player.start_using_item(hand);
        if player.active_item_use_hand() != Some(hand) {
            return InteractionResult::Fail;
        }
        self.with_session_mut(player.gameprofile.id(), |session| session.shield.start(now));
        player.send_action_bar(TextComponent::plain("Щит поднят").color(Color::Aqua));
        InteractionResult::Success
    }

    /// Advances guard drain/recovery without waiting for async work.
    pub fn tick_shield(&self, player: &Player) {
        let active = active_shield(player);
        let now = now_millis();
        let Some((update, stamina)) = self.with_session_mut(player.gameprofile.id(), |session| {
            let update = session.shield.tick(active, now);
            (update, session.shield.stamina())
        }) else {
            return;
        };
        if update == GuardTick::Exhausted {
            player.stop_using_item();
            player
                .send_action_bar(TextComponent::plain("Прочность щита истощена").color(Color::Red));
        } else if active && player.tick_count() % 10 == 0 {
            player.send_action_bar(
                TextComponent::plain(format!("Прочность щита {stamina:.0}/100")).color(Color::Aqua),
            );
        }
    }

    /// Resets a stance that must not leak through death into a new body.
    pub fn reset_shield(&self, uuid: uuid::Uuid) {
        self.with_session_mut(uuid, |session| session.shield = ShieldGuard::default());
    }

    /// Reduces a front-facing physical hit while the shield is held.
    pub fn shield_damage(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        let Some(player) = victim.as_player() else {
            return amount;
        };
        if amount <= 0.0 || !self.shield_blocks(player, source) {
            return amount;
        }
        play_at(
            &player.get_world(),
            &sound_events::ITEM_SHIELD_BLOCK,
            player.position(),
            1.0,
            1.0,
        );
        amount * DAMAGE_FACTOR
    }

    /// Returns the knockback multiplier of a hit against a hunter.
    pub fn shield_knockback_multiplier(
        &self,
        victim: &dyn LivingEntity,
        source: &DamageSource,
    ) -> f64 {
        let Some(player) = victim.as_player() else {
            return 1.0;
        };
        if self.shield_blocks(player, source) {
            KNOCKBACK_FACTOR
        } else {
            1.0
        }
    }

    fn shield_blocks(&self, player: &Player, source: &DamageSource) -> bool {
        if !active_shield(player) {
            return false;
        }
        let blocking = self
            .with_session_mut(player.gameprofile.id(), |session| {
                session.shield.is_blocking()
            })
            .unwrap_or(false);
        blocks_source(source, player.position(), player.look_angle(), blocking)
    }
}

fn blocks_source(source: &DamageSource, position: DVec3, look: DVec3, active: bool) -> bool {
    if !active
        || source.causing_entity().is_none()
        || (!source.is_direct() && !source.is(&DamageTypeTag::IS_PROJECTILE))
    {
        return false;
    }
    let Some(origin) = source
        .source_position()
        .or_else(|| source.causing_entity().map(|entity| entity.position()))
    else {
        return false;
    };
    let direction = (origin - position).normalize_or_zero();
    direction.length_squared() > 0.0 && direction.dot(look.normalize_or_zero()) >= 0.5
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_core::entity::damage::DamageSource;
    use steel_core::entity::entities::ZombieEntity;
    use steel_registry::{init_vanilla_registry, vanilla_damage_types, vanilla_entities};

    use super::{GuardTick, ShieldGuard, blocks_source};
    use std::sync::{Arc, Weak};

    #[test]
    fn guard_drains_in_four_seconds_and_requires_full_recovery() {
        let mut guard = ShieldGuard::default();
        guard.start(0);
        for (now, stamina) in [(1_000, 75.0), (2_000, 50.0), (3_000, 25.0)] {
            assert_eq!(guard.tick(true, now), GuardTick::Blocking);
            assert_eq!(guard.stamina(), stamina);
        }
        assert_eq!(guard.tick(true, 4_000), GuardTick::Exhausted);
        assert_eq!(guard.stamina(), 0.0);
        assert!(!guard.can_start(4_000));
        for now in [5_000, 6_000, 7_000, 8_000] {
            guard.tick(false, now);
            assert!(!guard.can_start(now));
        }
        guard.tick(false, 9_000);
        assert_eq!(guard.stamina(), 100.0);
        assert!(guard.can_start(9_000));
    }

    #[test]
    fn a_released_guard_keeps_charge_and_has_a_restart_cooldown() {
        let mut guard = ShieldGuard::default();
        guard.start(0);
        assert!(!guard.can_start(0));
        guard.tick(true, 1_000);
        guard.tick(false, 1_000);
        assert_eq!(guard.stamina(), 75.0);
        assert!(!guard.can_start(1_999));
        assert!(guard.can_start(2_000));
        guard.tick(false, 2_000);
        assert_eq!(guard.stamina(), 95.0);
        guard.stamina = 24.0;
        assert!(!guard.can_start(2_000));
        guard.stamina = 25.0;
        assert!(guard.can_start(2_000));
    }

    #[test]
    fn only_frontal_physical_hits_are_blocked() {
        init_vanilla_registry();
        let attacker: steel_core::entity::SharedEntity = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            1,
            DVec3::new(0.0, 0.0, 5.0),
            Weak::new(),
        ));
        let front = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(&attacker));
        assert!(blocks_source(&front, DVec3::ZERO, DVec3::Z, true));
        let projectile =
            DamageSource::environment(&vanilla_damage_types::ARROW).with_causing_entity(attacker);
        assert!(blocks_source(&projectile, DVec3::ZERO, DVec3::Z, true));
        assert!(!blocks_source(&front, DVec3::ZERO, DVec3::Z, false));
        assert!(!blocks_source(&front, DVec3::ZERO, DVec3::NEG_Z, true));
        assert!(!blocks_source(&front, DVec3::ZERO, DVec3::X, true));
        let magic = DamageSource::environment(&vanilla_damage_types::MAGIC);
        assert!(!blocks_source(&magic, DVec3::ZERO, DVec3::Z, true));
    }

    #[test]
    fn a_held_shield_reduces_real_front_damage_and_knockback() {
        use steel_core::entity::{Entity as _, LivingEntity};
        use steel_core::inventory::container::Container as _;
        use steel_registry::{item_stack::ItemStack, vanilla_items};
        use steel_utils::types::InteractionHand;

        use crate::hooks::SoloLevelingHooks;
        use crate::player_data::SoloLevelingPlayerData;
        use crate::state::SoloLevelingState;

        init_vanilla_registry();
        steel_core::behavior::init_behaviors();
        let directory = std::env::temp_dir().join(format!("shield-test-{}", uuid::Uuid::new_v4()));
        let state = Arc::new(SoloLevelingState::new(directory.clone()).expect("shield state"));
        let world =
            crate::test_support::world(Arc::new(SoloLevelingHooks::new(Arc::clone(&state))));
        let player = crate::test_support::player(&world.world, "Guardian", 1);
        let uuid = player.gameprofile.id();
        state.begin_session(uuid, SoloLevelingPlayerData::new(uuid));
        player
            .inventory
            .lock()
            .set_item(40, ItemStack::new(&vanilla_items::SHIELD));
        player.set_rotation((0.0, 0.0));
        assert_eq!(
            state.use_shield(&player, InteractionHand::OffHand),
            steel_core::behavior::InteractionResult::Success
        );
        assert_eq!(
            player.active_item_use_hand(),
            Some(InteractionHand::OffHand)
        );

        let attacker = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            2,
            DVec3::new(0.0, 0.0, 5.0),
            Weak::new(),
        ));
        let source = DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, attacker);
        assert!(player.hurt(&world.world, &source, 10.0));
        assert!((player.get_health() - 17.0).abs() < 1e-6);
        assert!((player.velocity().z + 0.1).abs() < 1e-6);

        player.stop_using_item();
        assert_eq!(player.active_item_use_hand(), None);
        assert!((state.shield_damage(player.as_ref(), &source, 10.0) - 10.0).abs() < 1e-6);
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}
