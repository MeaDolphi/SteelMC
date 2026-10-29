//! The Solo Leveling [`GameplayHooks`] implementation.
//!
//! Wires vanilla server lifecycle events into Solo Leveling's own
//! player-data storage, kept fully separate from `PersistentPlayerData`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{future::Future, pin::Pin};

use steel_core::behavior::InteractionResult;
use steel_core::entity::Entity;
use steel_core::entity::LivingEntity;
use steel_core::entity::SharedEntity;
use steel_core::entity::damage::DamageSource;
use steel_core::gameplay_hooks::GameplayHooks;
use steel_core::player::Player;
use steel_core::player::player_inventory::Prediction;
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_utils::BlockPos;
use steel_utils::types::{BlockStateId, InteractionHand};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::commands;
use crate::dungeon::instance;
use crate::dungeon::is_instance_world;
use crate::dungeon::runtime::TEST_GATE_ON_JOIN;
use crate::item;
use crate::item::chest::ChestKind;
use crate::main_menu;
use crate::organization::Event;
use crate::organization_menu;
use crate::quest::{PENALTY_DELAY_SECONDS, SystemQuest};
use crate::session::HunterSession;
use crate::session::QuestUpdate;
use crate::sidebar;
use crate::state::SoloLevelingState;

/// Solo Leveling's [`GameplayHooks`] implementation.
///
/// Holds the storage used to persist each player's Solo Leveling data as a
/// separate JSON file, keyed by `Uuid`, plus the live in-memory session for
/// each online player (so hooks like `on_player_tick` do not re-read the
/// JSON file every tick).
#[derive(Debug)]
pub struct SoloLevelingHooks {
    state: Arc<SoloLevelingState>,
}

impl SoloLevelingHooks {
    /// Creates hooks backed by the state shared with Solo Leveling commands.
    #[must_use]
    pub const fn new(state: Arc<SoloLevelingState>) -> Self {
        Self { state }
    }
}

impl GameplayHooks for SoloLevelingHooks {
    fn prepare_server(
        &self,
        server: Arc<Server>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move { state.load_auction(&server).await })
    }

    fn prepare_player_join(
        &self,
        _server: Arc<Server>,
        uuid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move { state.prepare_join(uuid).await })
    }

    fn flush(
        &self,
        server: Arc<Server>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let state = Arc::clone(&self.state);
        Box::pin(async move { state.flush_persistence(server).await })
    }

    fn on_server_tick(&self, server: &Arc<Server>, tick_count: u64) {
        self.state.tick_persistence(server);
        self.state.tick_auction(server, tick_count);
        self.state.tick_dungeons(server, tick_count);
        self.state.tick_organization(server);
        self.state.tick_storms(server);
    }

    fn on_player_join(&self, _server: &Arc<Server>, player: &Player) {
        let player_uuid = player.gameprofile.id();
        match self.state.take_prepared_player(player_uuid) {
            Some(data) => {
                data.stats.apply_to(&mut player.attributes().lock());
                self.state.begin_session(player_uuid, data);
                if let Some((_, view)) = self.state.sidebar_update(player_uuid) {
                    sidebar::show(player, &view);
                }
                let owed = self.state.collect_guild_payout(player_uuid);
                if owed > 0 {
                    player.send_message(
                        &TextComponent::plain(format!(
                            "Организация выплатила {owed} Won за гильдийные задания."
                        ))
                        .color(Color::Gold),
                    );
                }
                if self.state.has_pending_penalty(player_uuid) {
                    player.send_message(
                        &TextComponent::plain(format!(
                            "Система: невыполненное ежедневное задание. Штрафная зона через {} мин.",
                            PENALTY_DELAY_SECONDS / 60
                        ))
                        .color(Color::Red),
                    );
                }
                if let Err(error) = self.state.save_session(player_uuid) {
                    log::error!("sololeveling: could not queue initial player save: {error}");
                }
                if self.state.has_pending_delivery(player_uuid) {
                    self.state.request_auction_delivery(player_uuid);
                }
                if TEST_GATE_ON_JOIN {
                    self.state.request_test_gate(player_uuid);
                }
            }
            None => {
                player.disconnect(TextComponent::plain(
                    "Gameplay data was not prepared for this connection.",
                ));
            }
        }
    }

    fn on_player_disconnect(&self, server: &Server, player_uuid: Uuid) {
        self.state.on_party_disconnect(player_uuid);
        self.state.recall_shadows(player_uuid);
        self.state.end_player_run(server, player_uuid);
        self.state.retire_session(player_uuid);
    }

    fn on_player_tick(&self, player: &Player) {
        let player_uuid = player.gameprofile.id();
        let unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        self.state.tick_artifacts(player);
        item::menu_book::keep_in_place(player);
        if self.state.take_crystal_sale(player_uuid) {
            commands::crystal::sell(&self.state, player);
        }
        let window = SystemQuest::window_at(unix_seconds);
        let update = self.state.observe_quest(
            player_uuid,
            window,
            player.position(),
            player.last_client_input(),
        );
        if let Some(update) = update {
            announce_quest(player, update);
        }

        let due = self
            .state
            .with_session_mut(player_uuid, HunterSession::tick_sidebar_throttle);
        if due != Some(true) {
            return;
        }
        if let Some((previous, view)) = self.state.sidebar_update(player_uuid) {
            sidebar::refresh(player, previous.as_ref(), &view);
        }
    }

    fn on_player_death(&self, _server: &Arc<Server>, player: &Player) {
        self.state.recall_shadows(player.gameprofile.id());
        self.state.observe_dungeon_player_death(player);
    }

    fn on_player_respawn(&self, _server: &Arc<Server>, player: &Player) {
        let player_uuid = player.gameprofile.id();
        let Some(stats) = self
            .state
            .with_session_mut(player_uuid, |session| session.stats)
        else {
            return;
        };
        stats.apply_to(&mut player.attributes().lock());
        // Vanilla respawns at full health; vitality modifiers are re-applied
        // after that, so the replacement is topped up to its new maximum.
        player.set_health(player.get_max_health());
        if let Some(view) = self.state.sidebar_view(player_uuid) {
            let _ = self.state.sidebar_update(player_uuid);
            sidebar::restore(player, &view);
        }
    }

    fn allow_block_break(&self, world: &World, _player: &Player, pos: BlockPos) -> bool {
        // A board is removed with `/organization board remove`, never by hand.
        if self.state.is_organization_board(&world.key, pos) {
            return false;
        }
        // Dungeons are sealed rooms: a hunter may only extract the crystals built
        // into their walls, so the layout and the boss cannot be bypassed.
        if !is_instance_world(world) {
            return true;
        }
        instance::is_crystal_ore(world.get_block_state(pos))
    }

    fn allow_block_place(&self, world: &World, _player: &Player, _pos: BlockPos) -> bool {
        // Nothing may be built inside a dungeon, including the walls a hunter
        // cannot break.
        !is_instance_world(world)
    }

    fn on_block_drops(
        &self,
        world: &World,
        player: &Player,
        _pos: BlockPos,
        state: BlockStateId,
        drops: &mut Vec<ItemStack>,
    ) {
        if state != instance::CRYSTAL_ORE.default_state() {
            return;
        }
        let Some(rank) = self.state.instance_rank(world) else {
            return;
        };
        drops.clear();
        let mut crystals = item::crystal::stack(rank);
        crystals.set_count(1 + self.state.crystal_bonus(player));
        drops.push(crystals);
    }

    fn on_player_use_item(
        &self,
        player: &Arc<Player>,
        _world: &Arc<World>,
        stack: &ItemStack,
        hand: InteractionHand,
    ) -> Option<InteractionResult> {
        if item::menu_book::is_menu_book(stack) {
            main_menu::open(player, Arc::clone(&self.state));
            return Some(InteractionResult::Success);
        }
        item::consume::use_item(&self.state, player, stack, hand)
    }

    fn allow_item_drop(&self, _player: &Player, stack: &ItemStack) -> bool {
        // The System book is permanent; it is put back in its slot instead.
        !item::menu_book::is_menu_book(stack)
    }

    fn on_player_use_block(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        hand: InteractionHand,
    ) -> Option<InteractionResult> {
        if !self.state.is_organization_board(&world.key, pos) {
            return None;
        }
        // The client offers the off hand only when the main hand passed, so the
        // board opens once per click.
        if hand == InteractionHand::MainHand {
            organization_menu::open(player, Arc::clone(&self.state));
        }
        Some(InteractionResult::Success)
    }

    fn on_living_entity_hurt(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) {
        self.state.artifact_hit(entity, source);
        self.state.refresh_mob_label(world, entity);
        let Some(killer) = source.causing_entity() else {
            return;
        };
        let hunter = credited_hunter(killer);
        if let Some(hunter) = hunter {
            self.state
                .observe_raid_boss_damage(world, entity.id(), hunter, amount);
        }
    }

    fn modify_damage(
        &self,
        _world: &World,
        victim: &dyn LivingEntity,
        source: &DamageSource,
        amount: f32,
    ) -> f32 {
        self.state.artifact_damage(victim, source, amount)
    }

    fn prevent_death(
        &self,
        _world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) -> bool {
        self.state.artifact_prevent_death(entity, source)
    }

    fn on_living_entity_loot(
        &self,
        world: &World,
        entity_id: i32,
        _source: &DamageSource,
        drops: &mut Vec<ItemStack>,
    ) {
        self.state.capture_raid_loot(world, entity_id, drops);
    }

    fn on_living_entity_death(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) {
        self.state.observe_shadow_death(entity.uuid());
        let Some(killer) = source.causing_entity() else {
            let _ = self
                .state
                .observe_dungeon_mob_death(world, entity.id(), None);
            return;
        };
        let hunter = credited_hunter(killer);
        // Players and summoned shadows are not mobs the Organization pays for.
        if let Some(hunter) = hunter
            && entity.as_player().is_none()
            && entity
                .as_mob()
                .is_none_or(|mob| mob.allied_owner().is_none())
        {
            let zombie = entity.entity_type() == &vanilla_entities::ZOMBIE;
            self.state
                .record_organization_event(vec![hunter], Event::Kill { zombie });
        }
        let dungeon_mob = self
            .state
            .observe_dungeon_mob_death(world, entity.id(), hunter);
        if dungeon_mob
            && let Some(player) = killer.as_player()
            && let Some(shadow) = self.state.arise_from_player_kill(player, world, entity)
        {
            player.send_message(&TextComponent::plain(format!(
                "{} arose from the shadows.",
                shadow.name
            )));
        }
        let _ = self.state.credit_shadow_kill(killer.uuid(), entity, world);
    }
}

/// Returns the hunter credited for a hit or a kill: the attacker itself, or the
/// owner of the shadow that attacked for it.
fn credited_hunter(entity: &SharedEntity) -> Option<Uuid> {
    if let Some(player) = entity.as_player() {
        return Some(player.gameprofile.id());
    }
    entity.as_mob()?.allied_owner()
}

/// Hands out the daily reward and reports a failed day.
fn announce_quest(player: &Player, update: QuestUpdate) {
    if update.completed {
        player.add_item_or_drop(ChestKind::Normal.stack(), Prediction::ServerOnly);
        player.send_message(
            &TextComponent::plain("Ежедневное задание выполнено: получен обычный сундук.")
                .color(Color::Gold),
        );
    }
    if update.penalty_pending {
        player.send_message(
            &TextComponent::plain("Система: ежедневное задание провалено. Штрафная зона.")
                .color(Color::Red),
        );
    }
}
