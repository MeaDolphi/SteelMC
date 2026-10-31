//! Arrow projectile entity (`Arrow`).
//!
//! Mirrors vanilla `Arrow` on the Steel
//! `Projectile → AbstractArrow` trait stack.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components::{POTION_CONTENTS, POTION_DURATION_SCALE};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_data::ArrowEntityData;
use steel_registry::{vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use super::SpectralArrowEntity;
use crate::behavior::MOB_EFFECT_BEHAVIORS;
use crate::entity::potion_contents::to_runtime_instance;
use crate::entity::projectile::{
    AbstractArrow, AbstractArrowBase, PICKUP_ALLOWED, Projectile, ProjectileBase, ProjectileHit,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity, SharedEntity,
    next_entity_id,
};
use crate::player::Player;
use crate::world::World;

/// A shot arrow.
#[entity_behavior(class = "Arrow")]
pub struct ArrowEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<ArrowEntityData>,
    projectile_base: ProjectileBase,
    abstract_arrow_base: AbstractArrowBase,
}

// SAFETY: The owner-scoped type key uniquely identifies ArrowEntity.
unsafe impl DowncastType for ArrowEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/arrow");
}

impl ArrowEntity {
    /// Creates a new arrow with no owner and default data.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
        }
    }

    /// Creates an arrow from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            abstract_arrow_base: AbstractArrowBase::new(),
        }
    }

    /// Spawns an arrow shot by `owner` using `weapon` and `ammo`.
    ///
    /// Mirrors vanilla projectile selection (`ProjectileWeaponItem` /
    /// crossbow `createProjectile`): spectral ammunition spawns a
    /// [`SpectralArrowEntity`]; any other arrow item spawns an `ArrowEntity`
    /// carrying the ammunition (including tipped-arrow potion contents) as
    /// its pickup stack.
    pub fn spawn(
        world: &Arc<World>,
        owner: &dyn Entity,
        weapon: Option<&ItemStack>,
        ammo: &ItemStack,
        power: f32,
        spread: f32,
    ) -> Option<SharedEntity> {
        let position = owner.position();
        let spawn_pos = DVec3::new(position.x, owner.get_eye_y() - 0.1, position.z);

        let arrow: SharedEntity = if ammo.is(&vanilla_items::SPECTRAL_ARROW) {
            let arrow = SpectralArrowEntity::new(
                &vanilla_entities::SPECTRAL_ARROW,
                next_entity_id(),
                spawn_pos,
                Arc::downgrade(world),
            );
            init_shot_arrow(&arrow, owner, weapon, power, spread);
            arrow.set_pickup_item(ammo.copy_with_count(1));
            Arc::new(arrow)
        } else {
            let arrow = ArrowEntity::new(
                &vanilla_entities::ARROW,
                next_entity_id(),
                spawn_pos,
                Arc::downgrade(world),
            );
            init_shot_arrow(&arrow, owner, weapon, power, spread);
            arrow.set_pickup_item(ammo.copy_with_count(1));
            arrow.update_potion_color();
            Arc::new(arrow)
        };
        world.try_add_entity(Arc::clone(&arrow)).ok()?;
        Some(arrow)
    }

    /// Vanilla `Arrow.updateColor`: exposes the potion color through the
    /// synced `ID_EFFECT_COLOR` field (-1 when the pickup stack has no
    /// potion contents).
    fn update_potion_color(&self) {
        let contents = self
            .pickup_item()
            .get_or_default(POTION_CONTENTS, PotionContents::empty());
        let color = if contents.has_effects() {
            contents.get_color()
        } else {
            -1
        };
        self.entity_data.lock().id_effect_color.set(color);
    }
}

/// Applies the shared vanilla setup for a freshly shot arrow: owner (which
/// also enables pickup for player-shot arrows), fired weapon, and launch
/// direction/speed.
fn init_shot_arrow<A: AbstractArrow + ?Sized>(
    arrow: &A,
    owner: &dyn Entity,
    weapon: Option<&ItemStack>,
    power: f32,
    spread: f32,
) {
    arrow.set_owner_uuid(Some(owner.uuid()));
    // Vanilla `AbstractArrow.setOwner`: player-shot arrows become pickup-allowed.
    if owner.as_player().is_some() {
        arrow.set_pickup(PICKUP_ALLOWED);
    }
    if let Some(weapon) = weapon {
        arrow.set_weapon_item(Some(weapon.clone()));
    }

    let (yaw, pitch) = owner.rotation();
    arrow.shoot_from_rotation(owner, pitch, yaw, 0.0, power, spread);
}

impl Entity for ArrowEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.abstract_arrow_tick();
    }

    fn get_default_gravity(&self) -> f64 {
        self.abstract_arrow_default_gravity()
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        self.abstract_arrow_player_touch(player);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_abstract_arrow(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_abstract_arrow(nbt);
        self.update_potion_color();
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}
}

impl Projectile for ArrowEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, location: DVec3) {
        AbstractArrow::on_hit_entity(self, entity, location);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        AbstractArrow::on_hit(self, hit);
    }
}

impl AbstractArrow for ArrowEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.abstract_arrow_base
    }

    fn default_pickup_item(&self) -> ItemStack {
        ItemStack::new(&vanilla_items::ARROW)
    }

    /// Vanilla `Arrow.doPostHurtEffects`: applies the pickup stack's potion
    /// contents to the hit entity, scaled by the arrow item's potion
    /// duration scale.
    fn do_post_hurt_effects(self: &Arc<Self>, world: &World, entity: &dyn LivingEntity) {
        let pickup = self.pickup_item();
        let contents = pickup.get_or_default(POTION_CONTENTS, PotionContents::empty());
        if !contents.has_effects() {
            return;
        }
        let duration_scale = pickup.get_or_default(POTION_DURATION_SCALE, 1.0);
        // Vanilla `Projectile.getEffectSource`: the owner when it is a living
        // entity, attributing instantaneous effects.
        let owner = self.get_owner();
        let effect_source = owner
            .as_ref()
            .filter(|owner| owner.as_living_entity().is_some());

        for effect in contents.all_effects() {
            let behavior = MOB_EFFECT_BEHAVIORS.get_behavior(effect.effect());
            if let Some(instantaneous) = behavior.as_instantaneous() {
                let direct: SharedEntity = Arc::<Self>::clone(self);
                instantaneous.apply_instantaneous(
                    world,
                    entity,
                    effect.amplifier(),
                    Some(&direct),
                    owner.as_ref(),
                    1.0,
                );
                continue;
            }

            let scaled = effect.with_scaled_duration(duration_scale);
            entity.add_mob_effect_with_source(
                to_runtime_instance(&scaled, scaled.duration()),
                effect_source.map(AsRef::as_ref),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::data_components::PotionContents;
    use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        MobEffectInstance as RegistryMobEffectInstance, init_vanilla_registry, vanilla_entities,
        vanilla_items, vanilla_mob_effects,
    };
    use steel_utils::ChunkPos;

    use crate::behavior::init_behaviors;
    use crate::entity::entities::SpectralArrowEntity;
    use crate::entity::projectile::PICKUP_ALLOWED;
    use crate::entity::{Entity, LivingEntity, SharedEntity};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;
    use steel_utils::Downcast as _;

    use super::*;

    fn tipped_arrow() -> ItemStack {
        let mut arrow = ItemStack::new(&vanilla_items::TIPPED_ARROW);
        arrow.set(
            POTION_CONTENTS,
            PotionContents::new(
                None,
                None,
                vec![RegistryMobEffectInstance::simple(
                    vanilla_mob_effects::POISON,
                    800,
                    0,
                )],
                None,
            ),
        );
        arrow
    }

    #[test]
    fn arrow_has_default_gravity_and_damage() {
        init_vanilla_registry();

        let arrow = ArrowEntity::new(
            &vanilla_entities::ARROW,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );

        assert_eq!(arrow.get_default_gravity(), 0.05);
        assert_eq!(arrow.base_damage(), 2.0);
        assert!(!arrow.in_ground());
    }

    #[test]
    fn pickup_item_defaults_to_plain_arrow_and_stores_ammo() {
        init_vanilla_registry();

        let arrow = ArrowEntity::new(
            &vanilla_entities::ARROW,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        assert!(arrow.pickup_item().is(&vanilla_items::ARROW));

        let tipped = tipped_arrow();
        arrow.set_pickup_item(tipped.copy_with_count(1));
        let pickup = arrow.pickup_item();
        assert!(pickup.is(&vanilla_items::TIPPED_ARROW));
        assert!(
            pickup
                .get(POTION_CONTENTS)
                .is_some_and(|contents| contents.has_effects())
        );

        // Vanilla `Arrow.tick`: after the potion decays out of an exposed
        // arrow the pickup stack resets to a plain arrow.
        arrow.set_pickup_item(ItemStack::empty());
        assert!(arrow.pickup_item().is(&vanilla_items::ARROW));
    }

    #[test]
    fn spawn_selects_spectral_entity_for_spectral_ammo() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("arrow_spawn_spectral");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Shooter", 1).build();

        let spectral = ItemStack::new(&vanilla_items::SPECTRAL_ARROW);
        let spawned = ArrowEntity::spawn(world, player.as_ref(), None, &spectral, 3.0, 1.0)
            .expect("spectral arrow spawns");
        let spectral_arrow = spawned
            .as_ref()
            .downcast_ref::<SpectralArrowEntity>()
            .expect("spectral ammo spawns a spectral arrow entity");
        assert!(
            spectral_arrow
                .pickup_item()
                .is(&vanilla_items::SPECTRAL_ARROW)
        );
        assert_eq!(spectral_arrow.pickup(), PICKUP_ALLOWED);
        assert_eq!(
            spectral_arrow.entity_type(),
            &vanilla_entities::SPECTRAL_ARROW
        );
    }

    #[test]
    fn spawn_stores_tipped_ammo_as_pickup_stack() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("arrow_spawn_tipped");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Shooter", 1).build();

        let spawned = ArrowEntity::spawn(world, player.as_ref(), None, &tipped_arrow(), 3.0, 1.0)
            .expect("tipped arrow spawns");
        let arrow = spawned
            .as_ref()
            .downcast_ref::<ArrowEntity>()
            .expect("tipped ammo spawns a regular arrow entity");
        assert!(arrow.pickup_item().is(&vanilla_items::TIPPED_ARROW));
        assert_eq!(arrow.pickup(), PICKUP_ALLOWED);
    }

    fn shot_arrow(world: &Arc<World>, ammo: ItemStack) -> Arc<ArrowEntity> {
        let position = DVec3::new(0.5, 70.0, 0.5);
        let arrow = Arc::new(ArrowEntity::new(
            &vanilla_entities::ARROW,
            1000,
            position,
            Arc::downgrade(world),
        ));
        arrow.set_pickup_item(ammo);
        arrow.set_velocity(DVec3::new(3.0, 0.0, 0.0));
        let shared: SharedEntity = Arc::clone(&arrow) as SharedEntity;
        world
            .try_add_entity(shared)
            .expect("arrow enters the world");
        arrow
    }

    #[test]
    fn tipped_arrow_applies_scaled_potion_effect_on_hit() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("arrow_tipped_hit");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let target = TestPlayerBuilder::new(Arc::clone(world), "Target", 1).build();
        world
            .try_add_entity(Arc::<Player>::clone(&target))
            .expect("target enters the world");

        let arrow = shot_arrow(world, tipped_arrow());
        arrow.do_post_hurt_effects(world, target.as_ref());

        let effect = target
            .mob_effect(vanilla_mob_effects::POISON)
            .expect("tipped arrow applies its potion effect");
        // 800 ticks scaled by the tipped arrow's vanilla 0.125 duration scale.
        assert_eq!(effect.duration(), 100);
        assert_eq!(effect.amplifier(), 0);
    }

    #[test]
    fn plain_arrow_applies_no_effect_on_hit() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("arrow_plain_hit");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let target = TestPlayerBuilder::new(Arc::clone(world), "Target", 1).build();
        world
            .try_add_entity(Arc::<Player>::clone(&target))
            .expect("target enters the world");

        let arrow = shot_arrow(world, ItemStack::new(&vanilla_items::ARROW));
        arrow.do_post_hurt_effects(world, target.as_ref());

        assert!(target.active_mob_effects().is_empty());
    }

    #[test]
    fn stuck_arrow_is_picked_up_as_its_fired_item() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("arrow_pickup");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Collector", 1).build();
        world
            .try_add_entity(Arc::<Player>::clone(&player))
            .expect("player enters the world");

        let arrow = shot_arrow(world, tipped_arrow());
        arrow.set_in_ground(true);
        arrow.set_pickup(crate::entity::projectile::PICKUP_ALLOWED);

        let shared: SharedEntity = Arc::clone(&arrow) as SharedEntity;
        Arc::clone(&shared).player_touch(&player);

        assert!(shared.is_removed());
        assert!(player.inventory.lock().get_items().iter().any(|stack| {
            stack.is(&vanilla_items::TIPPED_ARROW)
                && stack
                    .get(POTION_CONTENTS)
                    .is_some_and(|contents| contents.has_effects())
        }));
    }
}
