//! Vanilla `AbstractArrow` shared behavior.
//!
//! Mirrors `net.minecraft.world.entity.projectile.AbstractArrow` as a trait +
//! base struct layered above `Projectile`.
#![allow(
    missing_docs,
    reason = "getters/setters directly mirror vanilla fields"
)]

use std::sync::Arc;

use glam::DVec3;
use simdnbt::ToNbtTag as _;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEvent;
use steel_registry::{sound_events, vanilla_damage_types};
use steel_utils::locks::SyncMutex;

use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::damage::DamageSource;
use crate::entity::{LivingEntity, Projectile, ProjectileHit, RemovalReason, SharedEntity};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::world::ClipHitResult;
use crate::world::World;

/// Default gravity for arrows (`AbstractArrow.getDefaultGravity`).
const DEFAULT_GRAVITY: f64 = 0.05;
/// Air drag multiplier for arrows (`AbstractArrow` airborne inertia).
const DEFAULT_AIR_DRAG: f32 = 0.99;
/// Water inertia for arrows (`AbstractArrow.applyInertia`).
const WATER_INERTIA: f64 = 0.6;
/// Ticks an arrow stays in a block before despawning.
const IN_GROUND_LIFETIME: i32 = 1200;
/// Initial `shakeTime` value on block impact.
const DEFAULT_SHAKE_TIME: i32 = 7;

/// Vanilla `AbstractArrow.Pickup.DISALLOWED`: the arrow can never be picked up.
pub const PICKUP_DISALLOWED: i8 = 0;
/// Vanilla `AbstractArrow.Pickup.ALLOWED`: any player may pick the arrow up.
pub const PICKUP_ALLOWED: i8 = 1;
/// Vanilla `AbstractArrow.Pickup.CREATIVE_ONLY`: only creative players may pick the arrow up.
pub const PICKUP_CREATIVE_ONLY: i8 = 2;

/// Runtime fields shared by vanilla `AbstractArrow` subclasses.
struct AbstractArrowState {
    base_damage: f64,
    knockback: i32,
    crit: bool,
    pierce_level: i8,
    in_ground: bool,
    shake_time: i32,
    life: i32,
    pickup: i8,
    sound_event: &'static SoundEvent,
    weapon_item: Option<ItemStack>,
    pickup_item: Option<ItemStack>,
}

impl AbstractArrowState {
    const fn new() -> Self {
        Self {
            base_damage: 2.0,
            knockback: 0,
            crit: false,
            pierce_level: 0,
            in_ground: false,
            shake_time: 0,
            life: 0,
            pickup: 0,
            sound_event: &sound_events::ENTITY_ARROW_HIT,
            weapon_item: None,
            pickup_item: None,
        }
    }
}

/// Shared `AbstractArrow` runtime state.
pub struct AbstractArrowBase {
    state: SyncMutex<AbstractArrowState>,
}

impl AbstractArrowBase {
    /// Creates default abstract-arrow runtime state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: SyncMutex::new(AbstractArrowState::new()),
        }
    }

    #[must_use]
    pub fn base_damage(&self) -> f64 {
        self.state.lock().base_damage
    }

    pub fn set_base_damage(&self, value: f64) {
        self.state.lock().base_damage = value;
    }

    #[must_use]
    pub fn knockback(&self) -> i32 {
        self.state.lock().knockback
    }

    pub fn set_knockback(&self, value: i32) {
        self.state.lock().knockback = value;
    }

    #[must_use]
    pub fn is_crit(&self) -> bool {
        self.state.lock().crit
    }

    pub fn set_crit(&self, value: bool) {
        self.state.lock().crit = value;
    }

    #[must_use]
    pub fn pierce_level(&self) -> i8 {
        self.state.lock().pierce_level
    }

    pub fn set_pierce_level(&self, value: i8) {
        self.state.lock().pierce_level = value;
    }

    #[must_use]
    pub fn in_ground(&self) -> bool {
        self.state.lock().in_ground
    }

    pub fn set_in_ground(&self, value: bool) {
        self.state.lock().in_ground = value;
    }

    #[must_use]
    pub fn shake_time(&self) -> i32 {
        self.state.lock().shake_time
    }

    pub fn set_shake_time(&self, value: i32) {
        self.state.lock().shake_time = value;
    }

    #[must_use]
    pub fn life(&self) -> i32 {
        self.state.lock().life
    }

    pub fn set_life(&self, value: i32) {
        self.state.lock().life = value;
    }

    #[must_use]
    pub fn pickup(&self) -> i8 {
        self.state.lock().pickup
    }

    pub fn set_pickup(&self, value: i8) {
        self.state.lock().pickup = value;
    }

    #[must_use]
    pub fn sound_event(&self) -> &'static SoundEvent {
        self.state.lock().sound_event
    }

    pub fn set_sound_event(&self, value: &'static SoundEvent) {
        self.state.lock().sound_event = value;
    }

    #[must_use]
    pub fn weapon_item(&self) -> Option<ItemStack> {
        self.state.lock().weapon_item.clone()
    }

    pub fn set_weapon_item(&self, value: Option<ItemStack>) {
        self.state.lock().weapon_item = value;
    }

    #[must_use]
    pub fn pickup_item(&self) -> Option<ItemStack> {
        self.state.lock().pickup_item.clone()
    }

    pub fn set_pickup_item(&self, value: Option<ItemStack>) {
        self.state.lock().pickup_item = value;
    }
}

impl Default for AbstractArrowBase {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla-shaped behavior shared by entities that extend `AbstractArrow`.
pub trait AbstractArrow: Projectile {
    /// Returns shared abstract-arrow runtime state.
    fn abstract_arrow_base(&self) -> &AbstractArrowBase;

    /// Returns the entity's `AbstractArrow` default gravity.
    fn abstract_arrow_default_gravity(&self) -> f64 {
        DEFAULT_GRAVITY
    }

    /// Returns the entity's air drag multiplier.
    fn abstract_arrow_air_drag(&self) -> f32 {
        DEFAULT_AIR_DRAG
    }

    /// Returns the entity's water inertia multiplier.
    fn abstract_arrow_water_inertia(&self) -> f64 {
        WATER_INERTIA
    }

    fn base_damage(&self) -> f64 {
        self.abstract_arrow_base().base_damage()
    }

    fn set_base_damage(&self, value: f64) {
        self.abstract_arrow_base().set_base_damage(value);
    }

    fn knockback(&self) -> i32 {
        self.abstract_arrow_base().knockback()
    }

    fn set_knockback(&self, value: i32) {
        self.abstract_arrow_base().set_knockback(value);
    }

    fn is_crit(&self) -> bool {
        self.abstract_arrow_base().is_crit()
    }

    fn set_crit(&self, value: bool) {
        self.abstract_arrow_base().set_crit(value);
    }

    fn pierce_level(&self) -> i8 {
        self.abstract_arrow_base().pierce_level()
    }

    fn set_pierce_level(&self, value: i8) {
        self.abstract_arrow_base().set_pierce_level(value);
    }

    fn in_ground(&self) -> bool {
        self.abstract_arrow_base().in_ground()
    }

    fn set_in_ground(&self, value: bool) {
        self.abstract_arrow_base().set_in_ground(value);
    }

    fn shake_time(&self) -> i32 {
        self.abstract_arrow_base().shake_time()
    }

    fn set_shake_time(&self, value: i32) {
        self.abstract_arrow_base().set_shake_time(value);
    }

    fn life(&self) -> i32 {
        self.abstract_arrow_base().life()
    }

    fn set_life(&self, value: i32) {
        self.abstract_arrow_base().set_life(value);
    }

    fn pickup(&self) -> i8 {
        self.abstract_arrow_base().pickup()
    }

    fn set_pickup(&self, value: i8) {
        self.abstract_arrow_base().set_pickup(value);
    }

    fn sound_event(&self) -> &'static SoundEvent {
        self.abstract_arrow_base().sound_event()
    }

    fn set_sound_event(&self, value: &'static SoundEvent) {
        self.abstract_arrow_base().set_sound_event(value);
    }

    fn weapon_item(&self) -> Option<ItemStack> {
        self.abstract_arrow_base().weapon_item()
    }

    fn set_weapon_item(&self, value: Option<ItemStack>) {
        self.abstract_arrow_base().set_weapon_item(value);
    }

    /// Vanilla `AbstractArrow.getDefaultPickupItem`: the stack returned when no
    /// pickup stack was stored.
    fn default_pickup_item(&self) -> ItemStack;

    /// Vanilla `AbstractArrow.getPickupItem`.
    fn pickup_item(&self) -> ItemStack {
        self.abstract_arrow_base()
            .pickup_item()
            .unwrap_or_else(|| self.default_pickup_item())
    }

    /// Vanilla `AbstractArrow.setPickupItemStack`; an empty stack resets to the
    /// default pickup item.
    fn set_pickup_item(&self, item: ItemStack) {
        let stored = if item.is_empty() { None } else { Some(item) };
        self.abstract_arrow_base().set_pickup_item(stored);
    }

    /// Vanilla `AbstractArrow.tryPickup`.
    fn try_pickup(&self, player: &Arc<Player>) -> bool {
        match self.pickup() {
            PICKUP_ALLOWED => {
                let mut item = self.pickup_item();
                player.inventory.lock().add(&mut item)
            }
            PICKUP_CREATIVE_ONLY => player.has_infinite_materials(),
            _ => false,
        }
    }

    /// Vanilla `AbstractArrow.playerTouch` for a stuck arrow.
    fn abstract_arrow_player_touch(self: Arc<Self>, player: &Arc<Player>)
    where
        Self: Sized,
    {
        if !self.in_ground() || self.shake_time() > 0 {
            return;
        }
        if self.try_pickup(player) {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    /// Vanilla `AbstractArrow.tick` movement and hit handling.
    fn abstract_arrow_tick(self: Arc<Self>) {
        if self.in_ground() {
            self.tick_in_ground();
            return;
        }

        self.set_old_position_to_current();
        self.base().set_old_rotation_to_current();

        self.apply_gravity();

        let inertia = if self.is_in_water() {
            self.abstract_arrow_water_inertia()
        } else {
            f64::from(self.abstract_arrow_air_drag())
        };
        self.set_velocity(self.velocity() * inertia);

        let hit = self.get_hit_result_on_move_vector();
        let new_position = match &hit {
            Some(result) => result.location(),
            None => self.position() + self.velocity(),
        };

        if let Err(error) = self.try_set_position(new_position) {
            log::debug!("failed to advance arrow {}: {error}", self.id());
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        self.update_rotation();
        self.projectile_base_tick();

        if let Some(result) = hit
            && self.is_alive()
            && !self.is_world_change_pending()
        {
            self.hit_target_or_deflect_self(&result);
        }
    }

    fn tick_in_ground(&self) {
        let mut life = self.life();
        life += 1;
        self.set_life(life);
        if life >= IN_GROUND_LIFETIME {
            self.set_removed(RemovalReason::Discarded);
        }

        let shake = self.shake_time();
        if shake > 0 {
            self.set_shake_time(shake - 1);
        }
    }

    /// Vanilla `AbstractArrow.onHitEntity`.
    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3)
    where
        Self: Sized,
    {
        let Some(world) = self.level() else {
            return;
        };

        let mut damage = self.base_damage() * self.velocity().length();
        if self.is_crit() {
            damage += damage / 2.0 + rand::random::<f64>() * damage / 2.0;
        }

        let direct = Arc::clone(&self);
        let direct: SharedEntity = direct;
        let mut source =
            DamageSource::environment(&vanilla_damage_types::ARROW).with_direct_entity(direct);
        if let Some(owner) = self.get_owner() {
            source = source.with_causing_entity(owner);
        }

        if entity.hurt(&world, &source, damage as f32) {
            if let Some(living) = entity.as_living_entity() {
                self.do_post_hurt_effects(&world, living);
            }
            // Vanilla applies knockback and enchantment effects here.
            if self.is_crit() {
                // VANILLA CLIENT-LOCAL: critical hit particles.
            }
        }

        self.set_removed(RemovalReason::Discarded);
    }

    /// Vanilla `AbstractArrow.doPostHurtEffects`, overridden by arrow
    /// subclasses to apply on-hit effects (tipped-arrow potion contents,
    /// spectral-arrow glowing).
    fn do_post_hurt_effects(self: &Arc<Self>, _world: &World, _entity: &dyn LivingEntity) {}

    /// Vanilla `AbstractArrow.onHitBlock`.
    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult)
    where
        Self: Sized,
    {
        self.projectile_on_hit_block(hit);

        let state = {
            let Some(world) = self.level() else {
                return;
            };
            world.get_block_state(hit.block_pos)
        };

        self.set_velocity(DVec3::ZERO);
        self.set_in_ground(true);
        self.set_shake_time(DEFAULT_SHAKE_TIME);
        self.set_life(0);

        if let Some(world) = self.level() {
            let sound = self.sound_event();
            world.play_sound_at(sound, self.sound_source(), self.position(), 1.0, 1.0, None);
            if !state.is_air() {
                let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
                behavior.on_projectile_hit(state, &world, hit, self.as_projectile_event_source());
            }
        }
    }

    /// Vanilla `AbstractArrow.onHit` dispatch.
    fn on_hit(self: Arc<Self>, hit: &ProjectileHit)
    where
        Self: Sized,
    {
        self.projectile_on_hit(hit);
    }

    /// Saves vanilla `AbstractArrow` fields to NBT.
    fn save_abstract_arrow(&self, nbt: &mut NbtCompound) {
        nbt.insert("damage", self.base_damage());
        nbt.insert("crit", i8::from(self.is_crit()));
        nbt.insert("inGround", i8::from(self.in_ground()));
        nbt.insert("shake", self.shake_time());
        nbt.insert("life", self.life());
        nbt.insert("PierceLevel", self.pierce_level());
        nbt.insert("pickup", self.pickup());
        nbt.insert("item", self.pickup_item().to_nbt_tag());
    }

    /// Loads vanilla `AbstractArrow` fields from NBT.
    fn load_abstract_arrow(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_base_damage(nbt.double("damage").unwrap_or(2.0));
        self.set_crit(nbt.byte("crit").is_some_and(|b| b != 0));
        self.set_in_ground(nbt.byte("inGround").is_some_and(|b| b != 0));
        self.set_shake_time(i32::from(nbt.short("shake").unwrap_or(0)));
        self.set_life(i32::from(nbt.short("life").unwrap_or(0)));
        self.set_pierce_level(nbt.byte("PierceLevel").unwrap_or(0));
        self.set_pickup(nbt.byte("pickup").unwrap_or(0));
        if let Some(item_tag) = nbt.compound("item")
            && let Some(item) = ItemStack::from_borrowed_compound(&item_tag)
        {
            self.set_pickup_item(item);
        }
    }
}
