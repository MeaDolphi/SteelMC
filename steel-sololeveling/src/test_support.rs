use std::sync::{Arc, Weak};

use steel_core::config::RuntimeConfig;
use steel_core::entity::damage::DamageHistory;
use steel_core::gameplay_hooks::GameplayHooks;
use steel_core::level_data::{GameTimeSource, WorldGenerationSettings};
use steel_core::player::connection::NetworkConnection;
use steel_core::player::{ClientInformation, GameProfile, Player, PlayerConnection, PlayerSession};
use steel_core::world::{World, WorldConfig, WorldStorageConfig};
use steel_core::worldgen::{ChunkGeneratorType, EmptyChunkGenerator};
use steel_protocol::packet_traits::{CompressionInfo, EncodedPacket};
use steel_registry::{init_vanilla_registry, vanilla_dimension_types};
use steel_utils::Identifier;
use steel_utils::types::{Difficulty, GameType};
use text_components::TextComponent;
use tokio::runtime::Runtime;
use toml::{Value, map::Map};
use uuid::Uuid;

pub(crate) struct TestWorld {
    pub(crate) world: Arc<World>,
    _history: Arc<DamageHistory>,
    _runtime: Arc<Runtime>,
}

pub(crate) fn world(gameplay_hooks: Arc<dyn GameplayHooks>) -> TestWorld {
    init_vanilla_registry();
    let dimension = &vanilla_dimension_types::OVERWORLD;
    let generation_settings = WorldGenerationSettings::from_generator_config(
        Identifier::vanilla_static("empty"),
        &Value::Table(Map::new()),
        dimension.key.clone(),
        dimension.min_y,
        dimension.height,
    );
    let history = Arc::new(DamageHistory::default());
    let runtime = Arc::new(Runtime::new().expect("test runtime"));
    let generation_pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("test generation pool"),
    );
    let world = runtime
        .block_on(World::new_with_config(
            Arc::clone(&runtime),
            Identifier::vanilla(format!("spell_test_{}", Uuid::new_v4())),
            dimension,
            0,
            WorldConfig {
                damage_history: Arc::clone(&history),
                gameplay_hooks,
                game_time_source: GameTimeSource::Primary,
                storage: WorldStorageConfig::RamOnly,
                level_data_path: None,
                generator: Arc::new(ChunkGeneratorType::Empty(EmptyChunkGenerator::new())),
                generation_settings,
                view_distance: 2,
                simulation_distance: 2,
                max_chained_neighbor_updates: 1_000,
                compression: None,
                is_flat: true,
                sea_level: 64,
                default_gamemode: GameType::Survival,
                difficulty: Difficulty::Normal,
            },
            generation_pool,
        ))
        .expect("test world");
    TestWorld {
        world,
        _history: history,
        _runtime: runtime,
    }
}

pub(crate) fn player(world: &Arc<World>, name: &str, id: i32) -> Arc<Player> {
    let config = Arc::new(RuntimeConfig {
        max_players: 1,
        view_distance: 2,
        simulation_distance: 2,
        online_mode: false,
        encryption: false,
        use_favicon: false,
        favicon: String::new(),
        compression: None,
        packet_workers: Some(1),
        chunk_generation_threads: Some(1),
        chunk_encoding_threads: Some(1),
        ..RuntimeConfig::default()
    });
    let profile = GameProfile::new(Uuid::new_v4(), name.to_owned(), Vec::new())
        .expect("valid test player name");
    let session = Arc::new(PlayerSession::new(
        config.chat_spam_threshold_seconds,
        config.command_spam_threshold_seconds,
    ));
    let player = Arc::new(Player::new(
        profile,
        Arc::new(PlayerConnection::Other(Box::<TestConnection>::default())),
        Arc::clone(&session),
        Arc::clone(world),
        Weak::new(),
        config,
        id,
        ClientInformation::default(),
    ));
    assert!(session.bind_initial_player(&player));
    assert!(world.players.insert(Arc::clone(&player)));
    player
}

#[derive(Default)]
struct TestConnection;

impl NetworkConnection for TestConnection {
    fn compression(&self) -> Option<CompressionInfo> {
        None
    }

    fn send_encoded(&self, _packet: EncodedPacket) {}

    fn send_encoded_bundle(&self, _packets: Vec<EncodedPacket>) {}

    fn disconnect_with_reason(&self, _reason: TextComponent) {}

    fn tick(&self) {}

    fn latency(&self) -> i32 {
        0
    }

    fn close(&self) {}

    fn closed(&self) -> bool {
        false
    }
}
