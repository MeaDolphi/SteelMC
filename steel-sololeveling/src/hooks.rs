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
use steel_core::gameplay_hooks::{BlockMutationCause, GameplayHooks};
use steel_core::player::Player;
use steel_core::server::Server;
use steel_core::world::World;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_blocks, vanilla_entities, vanilla_game_rules, vanilla_items};
use steel_utils::BlockPos;
use steel_utils::types::{BlockStateId, InteractionHand};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::auction_menu;
use crate::city;
use crate::deferred_inventory_menu;
use crate::dungeon::instance;
use crate::dungeon::is_instance_world;
use crate::dungeon::mobs;
use crate::dungeon::runtime::TEST_GATE_ON_JOIN;
use crate::enchant_menu;
use crate::gate_menu;
use crate::gear;
use crate::island;
use crate::item;
use crate::item::chest::ChestKind;
use crate::item::crystal::CrystalSaleError;
use crate::main_menu;
use crate::merchant_menu;
use crate::npc;
use crate::organization::Event;
use crate::organization_menu;
use crate::quest::{PENALTY_DELAY_SECONDS, PENALTY_MIN_RANK, SystemQuest};
use crate::quest_book::Goal;
use crate::rank::HunterRank;
use crate::scenario;
use crate::session::HunterSession;
use crate::session::QuestUpdate;
use crate::shadow::{BossSource, EliteShadow};
use crate::sidebar;
use crate::sound::SystemSound;
use crate::spell_menu;
use crate::starter_menu;
use crate::state::{EnchantAction, GateMenuAction, SoloLevelingState};
use crate::temple;

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
        Box::pin(async move {
            state.load_guilds(&server).await?;
            state.load_auction(&server).await?;
            state.load_gate_registry(&server).await?;
            state.load_island(&server).await?;
            state.load_parties(&server).await?;
            let Some(city_world) = server.worlds.server_default_world() else {
                return Err("Solo Leveling city world is not loaded".to_owned());
            };
            if !city::is_city_world(city_world) {
                return Err(format!(
                    "expected {}/{} to be the default city world",
                    city_world.key.namespace, city_world.key.path
                ));
            }
            city_world.set_game_rule(&vanilla_game_rules::SPAWN_MOBS, false);
            state.start_resource_pack().await;
            Ok(())
        })
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
        self.state.observe_server(server, tick_count);
        self.state.tick_persistence(server);
        self.state.tick_guild_payouts();
        self.state.tick_auction(server, tick_count);
        self.state.tick_dungeons(server, tick_count);
        self.state.tick_scenarios(server, tick_count);
        self.state.tick_island(server, tick_count);
        self.state.tick_elite_shadow_fights(server, tick_count);
        self.state.tick_organization(server);
        self.state.tick_storms(server);
        self.state.tick_spells(server);
        self.state.tick_enchant_effects(server);
        npc::tick(server, tick_count);
    }

    fn on_player_join(&self, _server: &Arc<Server>, player: &Player) {
        mobs::install_health_line(player);
        let player_uuid = player.gameprofile.id();
        self.state.offer_resource_pack(player);
        match self.state.take_prepared_player(player_uuid) {
            Some(data) => {
                data.stats.apply_to(&mut player.attributes().lock());
                data.class
                    .apply_to(&mut player.attributes().lock(), data.stats.rank());
                self.state.begin_session(player_uuid, data);
                if let Some((_, view)) = self.state.sidebar_update(player_uuid) {
                    sidebar::show(player, &view);
                }
                if self.state.collect_guild_payout(player_uuid) {
                    player.send_message(
                        &TextComponent::plain("Выплата за гильдийные задания обрабатывается.")
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
        scenario::on_disconnect(player_uuid);
        self.state.end_player_run(server, player_uuid);
        self.state.retire_session(player_uuid);
    }

    fn on_player_tick(&self, player: &Player) {
        let player_uuid = player.gameprofile.id();
        let unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        self.state.tick_artifacts(player);
        self.state.tick_class(player);
        item::menu_book::keep_in_place(player);
        if player.tick_count() % 20 == 0 {
            if is_instance_world(&player.get_world()) {
                // The free starter-weapon swap ends with the first run.
                self.state.close_starter_swap(player_uuid);
            } else {
                item::loot::unmark_inventory(player);
            }
        }
        if let Some(notice) = self.state.take_notice(player_uuid) {
            notice.show(player);
        }
        match self.state.take_enchant_action(player_uuid) {
            Some(EnchantAction::OpenMenu) => {
                enchant_menu::open_picker(player, Arc::clone(&self.state));
            }
            Some(EnchantAction::Apply(enchantment, target, completion)) => {
                match self.state.request_enchantment(
                    player,
                    enchantment,
                    target,
                    Arc::clone(&completion),
                ) {
                    Ok(()) => player.send_message(&TextComponent::plain(format!(
                        "Зачарование «{}» обрабатывается.",
                        enchantment.display_name()
                    ))),
                    Err(error) => {
                        *completion.lock() = Some(target);
                        player
                            .send_message(&TextComponent::plain(error.message()).color(Color::Red));
                    }
                }
            }
            None => {}
        }
        if self.state.take_buyback_menu(player_uuid) {
            merchant_menu::open_buyback(player, Arc::clone(&self.state));
        }
        if self.state.take_auction_sell_menu(player_uuid) {
            auction_menu::open_picker(player, Arc::clone(&self.state));
        }
        if self.state.take_gear_menu(player_uuid) {
            gear::open(player, Arc::clone(&self.state));
        }
        if let Some(page) = self.state.take_deferred_menu(player_uuid) {
            deferred_inventory_menu::open(player, Arc::clone(&self.state), page);
        }
        if let Some(action) = self.state.take_gate_menu_action(player_uuid) {
            self.handle_gate_menu_action(player, action);
        }
        if self.state.take_crystal_sale(player_uuid) {
            match self.state.sell_crystals(player) {
                Ok(()) => {
                    player
                        .send_message(&TextComponent::plain("Продажа кристаллов обрабатывается."));
                }
                Err(error) => {
                    let message = match error {
                        CrystalSaleError::NoCrystals => "В инвентаре нет магических кристаллов.",
                        CrystalSaleError::Storage => "Организация не может рассчитаться сейчас.",
                    };
                    player.send_message(&TextComponent::plain(message).color(Color::Red));
                }
            }
        }
        let window = SystemQuest::window_at(unix_seconds);
        let update = self.state.observe_quest(
            player_uuid,
            window,
            player.position(),
            player.last_client_input(),
        );
        if let Some(update) = update {
            announce_quest(&self.state, player, update);
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
        let uuid = player.gameprofile.id();
        self.state.recall_shadows(uuid);
        scenario::on_player_death(uuid);
        self.state.observe_dungeon_player_death(player);
    }

    fn on_player_respawn(&self, _server: &Arc<Server>, player: &Player) {
        let player_uuid = player.gameprofile.id();
        // The respawned entity starts without the gear bonuses.
        self.state.reset_gear_bonuses(player_uuid);
        let Some(stats) = self
            .state
            .with_session_mut(player_uuid, |session| session.stats)
        else {
            return;
        };
        stats.apply_to(&mut player.attributes().lock());
        self.state.apply_class(player);
        // Vanilla respawns at full health; vitality modifiers are re-applied
        // after that, so the replacement is topped up to its new maximum.
        player.set_health(player.get_max_health());
        if let Some(view) = self.state.sidebar_view(player_uuid) {
            let _ = self.state.sidebar_update(player_uuid);
            sidebar::restore(player, &view);
        }
    }

    fn allow_block_break(&self, world: &World, _player: &Player, pos: BlockPos) -> bool {
        // Registered Organization boards are not hand-broken.
        if self.state.is_organization_board(&world.key, pos) {
            return false;
        }
        if city::is_city_world(world) || island::is_island_world(world) {
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
        !is_instance_world(world) && !city::is_city_world(world) && !island::is_island_world(world)
    }

    fn allow_block_mutation(
        &self,
        world: &World,
        _pos: BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
        cause: BlockMutationCause,
    ) -> bool {
        if city::is_city_world(world) || island::is_island_world(world) {
            return allows_protected_block_mutation(old_state, new_state, cause);
        }
        !is_instance_world(world) || allows_instance_block_mutation(old_state, new_state, cause)
    }

    fn on_block_drops(
        &self,
        world: &World,
        player: &Player,
        pos: BlockPos,
        state: BlockStateId,
        drops: &mut Vec<ItemStack>,
    ) {
        if state != instance::CRYSTAL_ORE.default_state() {
            return;
        }
        if self.state.record_mine_crystal(world, pos, 1) {
            drops.clear();
            return;
        }
        let Some(rank) = self.state.instance_rank(world) else {
            return;
        };
        drops.clear();
        let (extra, essence) = self.state.enchant_mining(player);
        let mut crystals = item::crystal::stack(rank);
        let mined = 1 + self.state.crystal_bonus(player) + extra;
        crystals.set_count(mined);
        // Mined crystals appear in the hunter's inventory right away; only
        // what does not fit spills next to the broken block as a normal drop.
        give_or_drop(crystals, |stack| player.add_item_with_sound(stack), drops);
        self.state.record_goal(
            player.gameprofile.id(),
            Goal::Crystal,
            u32::try_from(mined).unwrap_or(1),
        );
        for _ in 0..mined {
            self.state
                .record_organization_event(vec![player.gameprofile.id()], Event::Crystal(rank));
        }
        if essence {
            drops.push(item::material::stack(
                item::material::Material::StableEssence,
                item::material::Quality::for_rank(rank),
                1,
            ));
        }
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
        // The grimoire works from either hand, so a Mage staff can sit in
        // the other one.
        if item::grimoire::is_grimoire(stack) {
            if player.is_crouching() {
                spell_menu::open(player, Arc::clone(&self.state));
            } else {
                self.state.queue_cast(player.gameprofile.id());
            }
            return Some(InteractionResult::Success);
        }
        if temple::is_temple_key(stack) {
            return Some(match self.state.enter_temple(player, hand) {
                Ok(()) => InteractionResult::Success,
                Err(message) => {
                    player.send_message(&TextComponent::plain(message).color(Color::Red));
                    InteractionResult::Fail
                }
            });
        }
        item::consume::use_item(&self.state, player, stack, hand)
    }

    fn allow_item_drop(&self, player: &Player, stack: &ItemStack, off_hand: &ItemStack) -> bool {
        // The drop key on the grimoire, or on a Mage staff held with the
        // grimoire in the off hand, cycles the prepared spells instead.
        // Core passes the off hand, so this never locks the inventory: the
        // hook can run while it is held, and locking it here would invert
        // this crate's lock order against the session-taking tick paths.
        match drop_decision(stack, off_hand) {
            DropDecision::CycleSpell => {
                self.state.cycle_spell(player);
                false
            }
            DropDecision::Destroy => false,
            DropDecision::Allow => true,
        }
    }

    fn keep_item_on_death(&self, player: &Player, stack: &ItemStack) -> bool {
        // Death drops may run under the inventory lock, so this stays
        // lock-free; core hands the drop hook the off hand itself.
        keeps_item_on_death(stack, is_instance_world(&player.get_world()))
    }

    fn allow_item_storage(&self, _player: &Player, stack: &ItemStack) -> bool {
        item::binding::may_store(stack)
    }

    fn allow_item_retrieve(&self, player: &Player, stack: &ItemStack) -> bool {
        item::binding::may_retrieve(stack, player.gameprofile.id())
    }

    fn keep_projectile_ammo(
        &self,
        _player: &Player,
        _weapon: &ItemStack,
        ammo: &ItemStack,
    ) -> bool {
        keeps_projectile_ammo(ammo)
    }

    fn on_player_interact_entity(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        target: &SharedEntity,
        hand: InteractionHand,
    ) -> Option<InteractionResult> {
        let role = npc::role_of(world, target.as_ref())?;
        // The client offers the off hand only when the main hand passed, so the
        // character answers once per click.
        if hand == InteractionHand::MainHand {
            match role {
                npc::NpcRole::OrganizationAgent => {
                    organization_menu::open(player, Arc::clone(&self.state));
                }
                npc::NpcRole::AssociationClerk => {
                    starter_menu::open(player, Arc::clone(&self.state));
                }
                npc::NpcRole::Merchant => {
                    merchant_menu::open(player, Arc::clone(&self.state));
                }
                npc::NpcRole::IslandDispatcher => {
                    self.state
                        .request_island_travel(player.gameprofile.id(), true);
                }
                npc::NpcRole::IslandEvacuation => {
                    self.state
                        .request_island_travel(player.gameprofile.id(), false);
                }
            }
        }
        Some(InteractionResult::Success)
    }

    fn on_player_use_block(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        hand: InteractionHand,
    ) -> Option<InteractionResult> {
        // A shadow-summon item is a soul lantern, so clicking a block would
        // try to place it — and where building is forbidden the client keeps
        // its placement prediction, so the stack visually vanishes. Activating
        // the shadow instead keeps the item working and the inventory in sync.
        let stack = player.inventory.lock().get_item_in_hand(hand).clone();
        if item::shadow::entity(&stack).is_some() {
            return item::consume::use_item(&self.state, player, &stack, hand);
        }
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
        self.state.artifact_hit(entity, source, amount);
        self.order_shadows(world, entity, source);
        self.state.refresh_mob_label(world, entity);
        if island::is_island_world(world) {
            island::refresh_health_line(world, entity);
        }
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
        let amount = self.state.class_damage(victim, source, amount);
        self.state.artifact_damage(victim, source, amount)
    }

    fn allow_living_entity_damage(
        &self,
        world: &World,
        victim: &dyn LivingEntity,
        source: &DamageSource,
    ) -> bool {
        if source
            .causing_entity()
            .and_then(|attacker| attacker.as_living_entity())
            .is_some_and(|attacker| {
                self.state
                    .are_dungeon_mobs_allied(world, attacker.id(), victim.id())
            })
        {
            return false;
        }
        let attacker_is_player = source
            .causing_entity()
            .is_some_and(|attacker| attacker.as_player().is_some());
        allows_player_damage(
            city::is_city_world(world),
            victim.as_player().is_some(),
            attacker_is_player,
        )
    }

    fn is_tank_taunt_target(&self, _world: &World, candidate: &dyn LivingEntity) -> bool {
        is_shadow_tank(candidate)
    }

    fn are_mobs_allied(&self, world: &World, attacker_id: i32, candidate_id: i32) -> bool {
        self.state
            .are_dungeon_mobs_allied(world, attacker_id, candidate_id)
    }

    fn should_retarget_hurt_mob(&self, world: &World, mob_id: i32, attacker_id: i32) -> bool {
        let Some(mob_entity) = world.get_entity_by_id(mob_id) else {
            return false;
        };
        let Some(mob) = mob_entity.as_mob() else {
            return false;
        };
        let Some(attacker_entity) = world.get_entity_by_id(attacker_id) else {
            return false;
        };
        let Some(attacker) = attacker_entity.as_mob() else {
            return false;
        };
        mob.allied_owner().is_none()
            && self.state.dungeon_mob_label(world, mob_id).is_some()
            && attacker.allied_owner().is_some()
    }

    fn prevent_death(
        &self,
        _world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) -> bool {
        self.state.spell_prevent_death(entity)
            || self.state.artifact_prevent_death(entity, source)
            || self.state.shadow_guard_prevent_death(entity)
    }

    fn on_living_entity_loot(
        &self,
        world: &World,
        entity_id: i32,
        _source: &DamageSource,
        drops: &mut Vec<ItemStack>,
    ) {
        // The System pays every reward itself, so vanilla loot tables (rotten
        // flesh and the like) never drop. A mob that just died may already be
        // gone from the entity index, so only a found player keeps its loot.
        let is_player = world
            .get_entity_by_id(entity_id)
            .is_some_and(|entity| entity.as_player().is_some());
        if !is_player {
            drops.clear();
        }
    }

    fn suppress_mob_drops(&self, _world: &World, _entity_id: i32) -> bool {
        true
    }

    fn on_living_entity_death(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) {
        mobs::clear_health_line(world, entity);
        scenario::on_boss_death(world, entity.id());
        self.state.observe_shadow_death(entity.uuid());
        let Some(killer) = source.causing_entity() else {
            if self
                .state
                .observe_island_death(world, entity.id(), None)
                .is_none()
            {
                let _ = self
                    .state
                    .observe_dungeon_mob_death(world, entity.id(), None);
            }
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
            let rank = self.state.instance_rank(world);
            self.state
                .record_organization_event(vec![hunter], Event::Kill { zombie, rank });
        }
        if let Some(kill) = self.state.observe_island_death(world, entity.id(), hunter) {
            if let Some(player) = killer.as_player() {
                if let Some((source, rank)) = island_boss_source(&kill)
                    && let Some(shadow) = self
                        .state
                        .extract_island_boss_shadow(player, entity, source, rank)
                {
                    announce_elite_extraction(player, &shadow);
                }
                if matches!(kill, island::IslandKill::Nest { .. }) {
                    // A nest is a part of the swarm, not a sentient champion;
                    // the ritual only answers the Ant King.
                    player.send_message(
                        &TextComponent::plain(
                            "Гнездо — часть роя, а не разумный враг: поднять его тень нельзя. Ритуал отвечает лишь Королю муравьёв.",
                        )
                        .color(Color::DarkGray),
                    );
                }
            }
            return;
        }
        if let Some(player) = killer.as_player()
            && let Some(BossSource::Dungeon {
                rank,
                archetype,
                red,
            }) = self.state.dungeon_boss_source(world, entity.id())
            && let Some(shadow) = self
                .state
                .extract_dungeon_boss_shadow(player, entity, rank, archetype, red)
        {
            announce_elite_extraction(player, &shadow);
        }
        // The run may end with this death, so the nameplate is read first.
        let label = self.state.dungeon_mob_label(world, entity.id());
        let dungeon_mob = self
            .state
            .observe_dungeon_mob_death(world, entity.id(), hunter);
        if dungeon_mob
            && let Some(player) = killer.as_player()
            && let Some(shadow) =
                self.state
                    .arise_from_player_kill(player, world, entity, label.as_ref())
        {
            SystemSound::Arise.play(player);
            player.send_title(TextComponent::plain("Восстань").color(Color::DarkPurple));
            player.send_title_times(5, 30, 10);
            player.send_message(
                &TextComponent::plain(format!("«{}» восстаёт из тени.", shadow.name))
                    .color(Color::DarkPurple),
            );
        }
        let _ = self.state.credit_shadow_kill(killer.uuid(), entity, world);
    }
}

impl SoloLevelingHooks {
    fn handle_gate_menu_action(&self, player: &Player, action: GateMenuAction) {
        match action {
            GateMenuAction::OpenPage(page) => {
                gate_menu::open(player, Arc::clone(&self.state), page);
            }
            GateMenuAction::Teleport(id) => {
                if is_instance_world(&player.get_world()) {
                    player.send_message(
                        &TextComponent::plain("Навигация по вратам недоступна в подземелье.")
                            .color(Color::Red),
                    );
                    return;
                }
                let Some(gate) = self.state.committed_gate(id) else {
                    player.send_message(
                        &TextComponent::plain("Эти врата уже недоступны.").color(Color::Red),
                    );
                    return;
                };
                let world = player.get_world();
                if gate.world != world.key {
                    player.send_message(
                        &TextComponent::plain(format!(
                            "Врата находятся в мире {}:{}.",
                            gate.world.namespace, gate.world.path
                        ))
                        .color(Color::Yellow),
                    );
                    return;
                }
                let uuid = player.gameprofile.id();
                let Some(rank) = self
                    .state
                    .with_session_mut(uuid, |session| session.stats.rank())
                else {
                    player.send_message(
                        &TextComponent::plain("Сессия охотника не найдена.").color(Color::Red),
                    );
                    return;
                };
                let price = gate_menu::teleport_price(rank);
                if let Err(message) = gate_menu::charge_teleport(&self.state, uuid, price) {
                    player.send_message(&TextComponent::plain(message).color(Color::Red));
                    return;
                }
                player.send_message(
                    &TextComponent::plain(format!("Перенос к вратам. Списано {price} Won."))
                        .color(Color::Gold),
                );
                let destination = glam::DVec3::new(
                    f64::from(gate.entrance.x()) + 0.5,
                    f64::from(gate.entrance.y()) + 1.0,
                    f64::from(gate.entrance.z()) - 2.5,
                );
                if let Err(error) = player.teleport_to(destination) {
                    player.send_message(
                        &TextComponent::plain(format!("Не удалось перенестись к вратам: {error}"))
                            .color(Color::Red),
                    );
                }
            }
        }
    }

    /// Turns a hit involving a hunter into orders for their shadows: they
    /// defend a hunter who is struck and join the fight the hunter starts.
    /// Hunters never send shadows at other players.
    fn order_shadows(&self, world: &World, victim: &dyn LivingEntity, source: &DamageSource) {
        let Some(attacker) = source.causing_entity() else {
            return;
        };
        if let Some(hunter) = victim.as_player() {
            if attacker.as_player().is_none() {
                self.state.focus_shadows(hunter.gameprofile.id(), attacker);
            }
            return;
        }
        if let Some(hunter) = attacker.as_player()
            && let Some(target) = world.get_entity_by_uuid(&victim.uuid())
        {
            self.state.focus_shadows(hunter.gameprofile.id(), &target);
        }
    }
}

fn is_shadow_tank(candidate: &dyn LivingEntity) -> bool {
    candidate
        .as_mob()
        .is_some_and(|mob| mob.allied_owner().is_some() && mob.is_allied_tank())
}

/// Solo Leveling ammunition rule: ordinary arrows are free for their owner,
/// while special ammunition (spectral, tipped) is still consumed.
fn keeps_projectile_ammo(ammo: &ItemStack) -> bool {
    ammo.is(&vanilla_items::ARROW)
}

/// Solo Leveling death rule: the System books, every serial and artifact item
/// and everything personal stay with the hunter; inside a run, anything that
/// is not the run's own marked loot stays as well. Materials and loot drop.
fn keeps_item_on_death(stack: &ItemStack, instance_world: bool) -> bool {
    item::grimoire::is_grimoire(stack)
        || item::menu_book::is_menu_book(stack)
        || item::serial::of(stack).is_some()
        || item::artifact::of(stack).is_some()
        || !item::binding::may_drop(stack)
        || (instance_world && !item::loot::is_marked(stack))
}

/// What dropping `stack` does under the Solo Leveling rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDecision {
    /// The drop key on spell gear cycles the prepared spells instead.
    CycleSpell,
    /// The stack is destroyed instead of spawning in the world.
    Destroy,
    /// The stack drops normally.
    Allow,
}

/// Resolves a drop without touching player state: dropping the grimoire, or a
/// Mage staff while the grimoire sits in the off hand, cycles the prepared
/// spells; the System book and personally bound stacks never leave the hunter.
fn drop_decision(stack: &ItemStack, off_hand: &ItemStack) -> DropDecision {
    let spell_gear = item::artifact::of(stack).is_some_and(|id| id.spell_gear().is_some())
        || item::serial::of(stack).is_some_and(|id| id.spell_gear().is_some());
    if item::grimoire::is_grimoire(stack) || (spell_gear && item::grimoire::is_grimoire(off_hand)) {
        return DropDecision::CycleSpell;
    }
    if item::menu_book::is_menu_book(stack) || !item::binding::may_drop(stack) {
        return DropDecision::Destroy;
    }
    DropDecision::Allow
}

/// Gives a mined stack to the hunter first; only the remainder that does not
/// fit becomes a normal drop next to the broken block.
fn give_or_drop(
    mut stack: ItemStack,
    add: impl FnOnce(&mut ItemStack) -> bool,
    drops: &mut Vec<ItemStack>,
) {
    if !add(&mut stack) {
        drops.push(stack);
    }
}

/// Returns the elite-shadow source of an island boss kill.
///
/// Nests are part of the swarm and raise nothing; only the Ant King answers
/// the extraction ritual.
const fn island_boss_source(kill: &island::IslandKill) -> Option<(BossSource, HunterRank)> {
    match kill {
        island::IslandKill::King { .. } => Some((BossSource::IslandKing, HunterRank::S)),
        island::IslandKill::Nest { .. } | island::IslandKill::Ant(_) => None,
    }
}

/// Tells the hunter whether the boss became an elite shadow.
fn announce_elite_extraction(player: &Player, shadow: &EliteShadow) {
    SystemSound::Arise.play(player);
    player.send_message(
        &TextComponent::plain(format!(
            "Ритуал «Восстань» удался: «{}» стала элитной тенью.",
            shadow.name
        ))
        .color(Color::DarkRed),
    );
}

/// Returns the hunter credited for a hit or a kill: the attacker itself, or the
/// owner of the shadow that attacked for it.
fn credited_hunter(entity: &SharedEntity) -> Option<Uuid> {
    if let Some(player) = entity.as_player() {
        return Some(player.gameprofile.id());
    }
    entity.as_mob()?.allied_owner()
}

fn allows_instance_block_mutation(
    old_state: BlockStateId,
    new_state: BlockStateId,
    cause: BlockMutationCause,
) -> bool {
    allows_instance_mutation(
        old_state == new_state,
        old_state.get_block() == new_state.get_block(),
        instance::is_crystal_ore(old_state) && new_state == vanilla_blocks::AIR.default_state(),
        cause,
    )
}

fn allows_instance_mutation(
    same_state: bool,
    same_block: bool,
    crystal_mined: bool,
    cause: BlockMutationCause,
) -> bool {
    if cause == BlockMutationCause::System {
        return true;
    }
    if same_state || same_block {
        return true;
    }
    cause == BlockMutationCause::PlayerBreak && crystal_mined
}

fn allows_protected_block_mutation(
    old_state: BlockStateId,
    new_state: BlockStateId,
    cause: BlockMutationCause,
) -> bool {
    allows_city_mutation(
        cause == BlockMutationCause::System,
        old_state == new_state,
        old_state.get_block() == new_state.get_block(),
    )
}

const fn allows_city_mutation(system_change: bool, same_state: bool, same_block: bool) -> bool {
    system_change || same_state || same_block
}

const fn allows_player_damage(
    in_city: bool,
    victim_is_player: bool,
    attacker_is_player: bool,
) -> bool {
    !(in_city && victim_is_player && attacker_is_player)
}

/// Hands out the daily reward and reports a failed day.
fn announce_quest(state: &SoloLevelingState, player: &Player, update: QuestUpdate) {
    if update.completed {
        state.request_system_loot(player, ChestKind::Normal.stack());
        SystemSound::Quest.play(player);
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
    if update.missed_without_penalty {
        player.send_message(
            &TextComponent::plain(format!(
                "Система: ежедневное задание провалено. До ранга {} штрафной зоны нет.",
                PENALTY_MIN_RANK.name()
            ))
            .color(Color::Yellow),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DropDecision, allows_city_mutation, allows_instance_mutation, allows_player_damage,
        drop_decision, give_or_drop, is_shadow_tank, keeps_item_on_death, keeps_projectile_ammo,
    };
    use crate::item::{artifact, grimoire, menu_book, serial};
    use glam::DVec3;
    use std::sync::Weak;
    use steel_core::entity::Entity as _;
    use steel_core::entity::entities::ZombieEntity;
    use steel_core::gameplay_hooks::BlockMutationCause;
    use steel_core::inventory::container::Container as _;
    use steel_core::player::player_inventory::PlayerInventory;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use uuid::Uuid;

    #[test]
    fn serial_and_artifact_gear_survives_death_but_materials_and_loot_drop() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();

        // Personal and transferable serial gear stays in the inventory.
        let personal =
            crate::item::serial::stack(crate::item::serial::SerialId::HunterSword, Some(owner));
        assert!(keeps_item_on_death(&personal, false));
        let tradable =
            crate::item::serial::stack(crate::item::serial::SerialId::ImprovedHunterBow, None);
        assert!(keeps_item_on_death(&tradable, false));

        // Artifact equipment stays as well.
        let artifact = crate::item::artifact::stack(
            crate::item::artifact::ArtifactId::AssassinsShoes,
            Some(owner),
        );
        assert!(keeps_item_on_death(&artifact, false));

        // Materials drop on death in the city; inside a run the existing
        // instance rule still keeps whatever is not the run's own loot.
        let materials = material_stack();
        assert!(!keeps_item_on_death(&materials, false));
        assert!(keeps_item_on_death(&materials, true));

        // Run loot drops inside a run.
        let mut loot = material_stack();
        crate::item::loot::mark(&mut loot);
        assert!(!keeps_item_on_death(&loot, true));
    }

    fn material_stack() -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::DIAMOND);
        stack.count = 3;
        stack
    }

    #[test]
    fn ordinary_arrows_are_kept_but_special_ammunition_is_consumed() {
        init_vanilla_registry();

        assert!(keeps_projectile_ammo(&ItemStack::new(
            &vanilla_items::ARROW
        )));
        assert!(!keeps_projectile_ammo(&ItemStack::new(
            &vanilla_items::SPECTRAL_ARROW
        )));
        assert!(!keeps_projectile_ammo(&ItemStack::new(
            &vanilla_items::TIPPED_ARROW
        )));
    }

    #[test]
    fn drop_key_cycles_spells_on_grimoire_and_on_staff_with_grimoire_off_hand() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let grimoire = grimoire::stack(owner);
        let empty = ItemStack::empty();

        // The grimoire itself cycles from either hand.
        assert_eq!(drop_decision(&grimoire, &empty), DropDecision::CycleSpell);

        // A bound Mage staff cycles while the grimoire sits in the off hand;
        // without it the bound stack is destroyed, not dropped.
        let staff = artifact::stack(artifact::ArtifactId::AshStaff, Some(owner));
        assert_eq!(drop_decision(&staff, &grimoire), DropDecision::CycleSpell);
        assert_eq!(drop_decision(&staff, &empty), DropDecision::Destroy);

        // An unbound staff without the grimoire drops normally.
        let loose = artifact::stack(artifact::ArtifactId::AshStaff, None);
        assert_eq!(drop_decision(&loose, &empty), DropDecision::Allow);
    }

    #[test]
    fn system_book_and_bound_stacks_are_destroyed_instead_of_dropped() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let empty = ItemStack::empty();

        // The System book is permanent.
        let book = menu_book::stack();
        assert_eq!(drop_decision(&book, &empty), DropDecision::Destroy);

        // Personal rewards cannot be passed on.
        let bound = serial::stack(serial::SerialId::HunterSword, Some(owner));
        assert_eq!(drop_decision(&bound, &empty), DropDecision::Destroy);

        // Ordinary items drop normally.
        assert_eq!(
            drop_decision(&material_stack(), &empty),
            DropDecision::Allow
        );
    }

    #[test]
    fn dungeon_guard_is_not_a_taunt_target_but_a_tank_shadow_is() {
        init_vanilla_registry();
        let zombie = ZombieEntity::new(&vanilla_entities::ZOMBIE, 1, DVec3::ZERO, Weak::new());
        let mob = zombie.as_mob().expect("zombie has mob behavior");
        mob.set_allied_tank(true);
        assert!(!is_shadow_tank(&zombie));
        mob.set_allied_owner(Some(Uuid::new_v4()));
        assert!(is_shadow_tank(&zombie));
        mob.set_allied_tank(false);
        assert!(!is_shadow_tank(&zombie));
    }

    #[test]
    fn dungeon_block_damage_only_allows_system_changes_and_crystal_mining() {
        assert!(allows_instance_mutation(
            false,
            false,
            false,
            BlockMutationCause::System
        ));
        assert!(allows_instance_mutation(
            false,
            false,
            true,
            BlockMutationCause::PlayerBreak
        ));
        assert!(!allows_instance_mutation(
            false,
            false,
            false,
            BlockMutationCause::PlayerBreak
        ));
        assert!(!allows_instance_mutation(
            false,
            false,
            false,
            BlockMutationCause::Gameplay
        ));
        assert!(allows_instance_mutation(
            false,
            true,
            false,
            BlockMutationCause::Gameplay
        ));
    }

    #[test]
    fn city_protection_allows_only_system_or_same_block_state_changes() {
        assert!(allows_city_mutation(true, false, false));
        assert!(allows_city_mutation(false, true, true));
        assert!(allows_city_mutation(false, false, true));
        assert!(!allows_city_mutation(false, false, false));
    }

    #[test]
    fn player_attacks_are_disabled_only_against_city_players() {
        assert!(!allows_player_damage(true, true, true));
        assert!(allows_player_damage(true, true, false));
        assert!(allows_player_damage(true, false, true));
        assert!(allows_player_damage(false, true, true));
    }

    #[test]
    fn mined_crystals_land_in_inventory_and_only_the_overflow_drops() {
        init_vanilla_registry();
        let stack = |count: i32| {
            let mut stack = ItemStack::new(&vanilla_items::DIAMOND);
            stack.set_count(count);
            stack
        };
        let count_of = |inventory: &PlayerInventory| -> i32 {
            (0..inventory.get_container_size())
                .filter(|slot| inventory.get_item(*slot).is(&vanilla_items::DIAMOND))
                .map(|slot| inventory.get_item(slot).count())
                .sum()
        };

        // A fitting stack goes entirely to the inventory and stays out of drops.
        let mut inventory = PlayerInventory::new();
        let mut drops = Vec::new();
        give_or_drop(stack(5), |s| inventory.add(s), &mut drops);
        assert!(drops.is_empty(), "a fitting stack must not become a drop");
        assert_eq!(count_of(&inventory), 5);

        // A full inventory cannot absorb anything: everything stays a drop.
        let mut inventory = PlayerInventory::new();
        for slot in 0..PlayerInventory::INVENTORY_SIZE {
            inventory.set_item(slot, stack(64));
        }
        let mut drops = Vec::new();
        give_or_drop(stack(3), |s| inventory.add(s), &mut drops);
        assert_eq!(count_of(&inventory), 36 * 64);
        assert_eq!(drops, vec![stack(3)]);
    }

    #[test]
    fn island_extraction_is_offered_only_to_the_ant_king() {
        use crate::island::{IslandKill, Zone};
        use crate::rank::HunterRank;
        use crate::shadow::BossSource;

        let king = super::island_boss_source(&IslandKill::King {
            hunters: Vec::new(),
        });
        assert_eq!(king, Some((BossSource::IslandKing, HunterRank::S)));
        assert!(
            super::island_boss_source(&IslandKill::Nest {
                zone: Zone::Nests,
                left: 3,
                zone_cleared: false,
            })
            .is_none()
        );
        assert!(super::island_boss_source(&IslandKill::Ant(HunterRank::B)).is_none());
    }
}
