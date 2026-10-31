//! Extension point letting a gameplay layer (e.g. a themed game mode crate)
//! observe core server lifecycle events without steel-core depending on it.
//!
//! A [`Server`] holds a single `Arc<dyn GameplayHooks>`, defaulting to
//! [`NoopGameplayHooks`] when no gameplay layer is attached. Call sites invoke
//! these methods at a few fixed points in the vanilla server flow (player
//! join, player disconnect, player tick, player respawn, and living-entity
//! death); steel-core never inspects or depends on what a hook implementation
//! does.

use std::sync::Arc;
use std::{future::Future, pin::Pin};

use steel_registry::item_stack::ItemStack;
use steel_utils::BlockPos;
use steel_utils::types::{BlockStateId, InteractionHand};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::{LivingEntity, SharedEntity};
use crate::player::Player;
use crate::server::Server;
use crate::world::World;

/// Origin of a block-state mutation submitted through [`World`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMutationCause {
    /// Ordinary block behavior, such as fluid or neighbor updates.
    Gameplay,
    /// A player completing a permitted block break.
    PlayerBreak,
    /// Server-owned setup or cleanup, such as dungeon generation.
    System,
}

/// Observes core server lifecycle events on behalf of an attached gameplay layer.
///
/// All methods have no-op default bodies, so an implementor can override only
/// the events it cares about.
pub trait GameplayHooks: Send + Sync {
    /// Loads gameplay-owned data off-tick before a player is admitted to a world.
    fn prepare_player_join(
        &self,
        _server: Arc<Server>,
        _uuid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async { Ok(()) })
    }

    /// Loads gameplay-owned state once, before any connection is accepted.
    ///
    /// World data and gameplay documents are only readable through a live
    /// [`Server`], so a gameplay layer that must read its own documents at
    /// startup needs this point rather than its constructor.
    fn prepare_server(
        &self,
        _server: Arc<Server>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async { Ok(()) })
    }

    /// Flushes gameplay-owned persistence after ticks and packets have stopped.
    fn flush(
        &self,
        _server: Arc<Server>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async { Ok(()) })
    }

    /// Called once per server tick after worlds have completed their gameplay
    /// tick and before queued world changes are processed.
    fn on_server_tick(&self, _server: &Arc<Server>, _tick_count: u64) {}

    /// Called when a player finishes joining the server, after vanilla join
    /// handling (including sending the player's inventory) has completed.
    fn on_player_join(&self, _server: &Arc<Server>, _player: &Player) {}

    /// Called when a player is about to be detached from the server during
    /// disconnect handling, just before the player's entity state is cleaned
    /// up.
    fn on_player_disconnect(&self, _server: &Server, _player_uuid: Uuid) {}

    /// Called once per game tick for each ticking player.
    fn on_player_tick(&self, _player: &Player) {}

    /// Called after a living entity loses health to an accepted damage event.
    ///
    /// `amount` is the final health reduction after armor, effects, absorption,
    /// and the vanilla damage cooldown. It is zero when the accepted hit did
    /// not reduce health.
    fn on_living_entity_hurt(
        &self,
        _world: &World,
        _entity: &dyn LivingEntity,
        _source: &DamageSource,
        _amount: f32,
    ) {
    }

    /// Called before the living-entity damage pipeline; `false` rejects the hit.
    fn allow_living_entity_damage(
        &self,
        _world: &World,
        _victim: &dyn LivingEntity,
        _source: &DamageSource,
    ) -> bool {
        true
    }

    /// Returns whether a living entity should be selected before ordinary targets.
    fn is_tank_taunt_target(&self, _world: &World, _candidate: &dyn LivingEntity) -> bool {
        false
    }

    /// Returns whether two mobs should be treated as allies for target selection.
    fn are_mobs_allied(&self, _world: &World, _attacker_id: i32, _candidate_id: i32) -> bool {
        false
    }

    /// Whether a mob should retarget to a new attacker while its hurt-by goal runs.
    fn should_retarget_hurt_mob(&self, _world: &World, _mob_id: i32, _attacker_id: i32) -> bool {
        false
    }

    /// Called after vanilla rolls a living entity's loot but before it spawns.
    ///
    /// Implementations may drain or replace `drops` to route a special
    /// encounter's rewards themselves.
    fn on_living_entity_loot(
        &self,
        _world: &World,
        _entity_id: i32,
        _source: &DamageSource,
        _drops: &mut Vec<ItemStack>,
    ) {
    }

    /// Returns whether items spawned directly by a mob should be suppressed.
    fn suppress_mob_drops(&self, _world: &World, _entity_id: i32) -> bool {
        false
    }

    /// Called once after successful vanilla death processing for every living entity.
    ///
    /// It is not emitted for unloading, despawning, or other entity removal.
    fn on_living_entity_death(
        &self,
        _world: &World,
        _entity: &dyn LivingEntity,
        _source: &DamageSource,
    ) {
    }

    /// Called when a living entity took lethal damage and no vanilla totem saved it.
    ///
    /// Returning `true` cancels the death; the implementation must then have
    /// restored the entity's health itself, the way a totem sets it to 1.
    fn prevent_death(
        &self,
        _world: &World,
        _entity: &dyn LivingEntity,
        _source: &DamageSource,
    ) -> bool {
        false
    }

    /// Scales incoming damage before it is applied to a living entity.
    ///
    /// Vanilla health, armor, effects, and the damage cooldown still run on the
    /// returned value, so this only adjusts the input of that pipeline. The
    /// default returns `amount` unchanged.
    fn modify_damage(
        &self,
        _world: &World,
        _victim: &dyn LivingEntity,
        _source: &DamageSource,
        amount: f32,
    ) -> f32 {
        amount
    }

    /// Called after a block's loot was rolled but before the drops spawn.
    ///
    /// Implementations may drain or replace `drops` to route a themed
    /// resource themselves, the same way [`GameplayHooks::on_living_entity_loot`]
    /// works for entities.
    fn on_block_drops(
        &self,
        _world: &World,
        _player: &Player,
        _pos: BlockPos,
        _state: BlockStateId,
        _drops: &mut Vec<ItemStack>,
    ) {
    }

    /// Called before a player breaks a block; `false` refuses the break.
    ///
    /// Runs after the vanilla game-mode and adventure restrictions, so a
    /// refusal here cannot grant an otherwise forbidden break.
    fn allow_block_break(&self, _world: &World, _player: &Player, _pos: BlockPos) -> bool {
        true
    }

    /// Called before a player places a block; `false` refuses the placement.
    fn allow_block_place(&self, _world: &World, _player: &Player, _pos: BlockPos) -> bool {
        true
    }

    /// Called before a block state changes through the world mutation APIs.
    fn allow_block_mutation(
        &self,
        _world: &World,
        _pos: BlockPos,
        _old_state: BlockStateId,
        _new_state: BlockStateId,
        _cause: BlockMutationCause,
    ) -> bool {
        true
    }

    /// Called when a player uses an item, before the item's vanilla behavior.
    ///
    /// Returning `Some` replaces the vanilla result, which is how a gameplay
    /// layer can consume its own themed item types.
    fn on_player_use_item(
        &self,
        _player: &Arc<Player>,
        _world: &Arc<World>,
        _stack: &ItemStack,
        _hand: InteractionHand,
    ) -> Option<InteractionResult> {
        None
    }

    /// Called when a player uses a block, before the block's vanilla behavior.
    ///
    /// Returning `Some` replaces the vanilla interaction, which is how a
    /// gameplay layer can turn a placed block into its own interactive object.
    /// Spectators never reach this hook.
    fn on_player_use_block(
        &self,
        _player: &Arc<Player>,
        _world: &Arc<World>,
        _pos: BlockPos,
        _hand: InteractionHand,
    ) -> Option<InteractionResult> {
        None
    }

    /// Called when a player right-clicks an entity, before vanilla handles it;
    /// `Some` consumes the interaction with that result.
    fn on_player_interact_entity(
        &self,
        _player: &Arc<Player>,
        _world: &Arc<World>,
        _target: &SharedEntity,
        _hand: InteractionHand,
    ) -> Option<InteractionResult> {
        None
    }

    /// Called for every stack a dying player would drop; `true` keeps the
    /// stack in its slot, so it survives the respawn.
    fn keep_item_on_death(&self, _player: &Player, _stack: &ItemStack) -> bool {
        false
    }

    /// Called before a player throws or drops an item, including death drops;
    /// `false` destroys the stack instead of spawning it in the world.
    fn allow_item_drop(&self, _player: &Player, _stack: &ItemStack) -> bool {
        true
    }

    /// Called before a player moves an item into storage other than their own
    /// inventory, including containers, crafting slots, and guild banks.
    ///
    /// `false` cancels the transfer but still lets the player move the item
    /// around within their own inventory.
    fn allow_item_storage(&self, _player: &Player, _stack: &ItemStack) -> bool {
        true
    }

    /// Called before a player retrieves an item from storage outside their
    /// inventory, including containers, crafting slots, and guild banks.
    fn allow_item_retrieve(&self, _player: &Player, _stack: &ItemStack) -> bool {
        true
    }

    /// Called when a player fires a bow or crossbow, after the vanilla ammo
    /// consumption rules (creative `hasInfiniteMaterials`, bow INFINITY with
    /// plain arrows) already allowed keeping the ammunition.
    ///
    /// Returning `true` keeps `ammo` in the player's inventory instead of
    /// consuming it; the default preserves vanilla consumption.
    fn keep_projectile_ammo(
        &self,
        _player: &Player,
        _weapon: &ItemStack,
        _ammo: &ItemStack,
    ) -> bool {
        false
    }

    /// Called when a player dies, after vanilla death handling has run.
    ///
    /// Non-player deaths have no reachable [`Server`] handle at the vanilla
    /// call site (`World` holds no back-reference to its `Server`), so this
    /// hook currently only fires for player deaths.
    fn on_player_death(&self, _server: &Arc<Server>, _player: &Player) {}

    /// Called after a player's respawn replacement is installed and announced.
    ///
    /// Vanilla respawn builds a new player entity, so anything a gameplay
    /// layer applied to the previous entity (attribute modifiers, client-side
    /// UI other than the scoreboard) has to be applied again here.
    fn on_player_respawn(&self, _server: &Arc<Server>, _player: &Player) {}
}

/// A [`GameplayHooks`] implementation that does nothing.
///
/// Used as the default when no gameplay layer is attached to the server.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopGameplayHooks;

impl GameplayHooks for NoopGameplayHooks {}
