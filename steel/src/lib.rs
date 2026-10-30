//! # Steel
//!
//! The main library for the Steel Minecraft server.

// Wiring the gameplay layer into the server asks the compiler to prove
// `Send`/`Sync` through nested lock and chunk-map types, which needs more
// trait-evaluation depth than the default recursion limit allows.
#![recursion_limit = "256"]

use std::sync::atomic::Ordering;
use std::{
    error::Error,
    fmt, io,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, OnceLock},
};

use std::path::Path;

use steel_core::config::{StorageSelection, WorldEntryConfig, WorldsConfig};
use steel_core::gameplay_hooks::GameplayHooks;
use steel_core::{
    GIT_HASH_SHORT, command::CommandRegistry, permission::PermissionGroupManager, server::Server,
    worldgen::WorldGeneratorRegistry,
};
use steel_login::{JavaTcpClient, ServerConnectionSession};
use steel_sololeveling::{
    city, commands, hooks::SoloLevelingHooks, island, state::SoloLevelingState,
};
use tokio::{net::TcpListener, runtime::Runtime, select};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use toml::{Value, map::Map};

/// Server configuration module.
pub mod config;
/// A module for logging utilities.
pub mod logger;

/// Static access to the server
pub static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

/// The main server struct.
pub struct SteelServer {
    /// The TCP listener for incoming connections.
    pub tcp_listener: TcpListener,
    /// The cancellation token for graceful shutdown.
    pub cancel_token: CancellationToken,
    /// The next client ID to be assigned.
    pub client_id: u64,
    /// The shared server state.
    pub server: Arc<Server>,
    /// Session id UUID state
    pub connection_session: Arc<ServerConnectionSession>,
}

/// Startup error for expected operational failures.
#[derive(Debug)]
pub enum SteelServerError {
    /// Core server startup failed.
    Core(String),
    /// TCP listener could not bind.
    Bind {
        /// Server port that failed to bind.
        port: u16,
        /// Underlying IO error.
        source: io::Error,
    },
}

impl fmt::Display for SteelServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => f.write_str(error),
            Self::Bind { port, source } => {
                write!(f, "failed to bind to server port {port}: {source}")
            }
        }
    }
}

impl Error for SteelServerError {}

impl SteelServer {
    /// Creates a new Steel server with the Solo Leveling gameplay layer attached.
    ///
    /// Solo Leveling player data is stored under a `sololeveling_playerdata`
    /// directory next to the vanilla world save directory
    /// (`steel_config.worlds.save_path`).
    pub async fn new(
        chunk_runtime: Arc<Runtime>,
        cancel_token: CancellationToken,
        steel_config: config::SteelConfig,
    ) -> Result<Self, SteelServerError> {
        let sololeveling_dir = Path::new(&steel_config.worlds.save_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sololeveling_playerdata");
        let sololeveling_state =
            Arc::new(SoloLevelingState::new(sololeveling_dir).map_err(|source| {
                SteelServerError::Core(format!(
                    "failed to initialize Solo Leveling player data storage: {source}"
                ))
            })?);
        let gameplay_hooks: Arc<dyn GameplayHooks> =
            Arc::new(SoloLevelingHooks::new(Arc::clone(&sololeveling_state)));
        let mut command_registry = CommandRegistry::new();
        commands::register(&mut command_registry, sololeveling_state).map_err(|error| {
            SteelServerError::Core(format!(
                "failed to register Solo Leveling commands: {error}"
            ))
        })?;

        Self::new_with_commands(
            chunk_runtime,
            cancel_token,
            steel_config,
            command_registry,
            gameplay_hooks,
        )
        .await
    }

    /// Creates a new Steel server with additional commands registered atomically at startup.
    pub async fn new_with_commands(
        chunk_runtime: Arc<Runtime>,
        cancel_token: CancellationToken,
        mut steel_config: config::SteelConfig,
        command_registry: CommandRegistry,
        gameplay_hooks: Arc<dyn GameplayHooks>,
    ) -> Result<Self, SteelServerError> {
        ensure_sololeveling_city_world(&mut steel_config.worlds)?;
        ensure_sololeveling_instance_world(&mut steel_config)?;
        ensure_sololeveling_island_world(&mut steel_config.worlds)?;
        log::info!("Starting Steel Server ({GIT_HASH_SHORT})");

        let permission_group_store = steel_config.permission_group_store();
        let server_port = steel_config.server.server_port;
        let worlds_config = steel_config.worlds;
        let permission_groups =
            PermissionGroupManager::new(steel_config.groups, permission_group_store).map_err(
                |error| {
                    SteelServerError::Core(format!("failed to validate groups config: {error}"))
                },
            )?;
        let player_idle_timeout = steel_config.server.player_idle_timeout;
        let runtime_config = steel_config.server.into_runtime_config();

        let mut generator_registry = WorldGeneratorRegistry::new_with_builtins()
            .map_err(|error| SteelServerError::Core(error))?;
        city::register_world_generator(&mut generator_registry).map_err(SteelServerError::Core)?;
        island::register_world_generator(&mut generator_registry)
            .map_err(SteelServerError::Core)?;
        let server = Server::new_with_commands_and_world_generators(
            chunk_runtime,
            cancel_token.clone(),
            runtime_config,
            worlds_config,
            permission_groups,
            command_registry,
            gameplay_hooks,
            generator_registry,
        )
        .await
        .map_err(SteelServerError::Core)?;

        server
            .player_idle_timeout
            .store(player_idle_timeout, Ordering::Relaxed);

        let tcp_listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, server_port))
            .await
            .map_err(|source| SteelServerError::Bind {
                port: server_port,
                source,
            })?;

        let server = Arc::new(server);
        // Runs before the accept loop starts, so no player can join while the
        // gameplay layer is still reading its own stored documents.
        server
            .gameplay_hooks
            .prepare_server(Arc::clone(&server))
            .await
            .map_err(SteelServerError::Core)?;

        Ok(Self {
            tcp_listener,
            cancel_token,
            client_id: 0,
            server,
            connection_session: Arc::new(ServerConnectionSession::default()),
        })
    }

    /// Starts the server and begins accepting connections.
    pub async fn start(&mut self, task_tracker: TaskTracker) {
        log::info!("Started Steel Server");

        let server = Arc::clone(&self.server);
        let token = self.cancel_token.clone();
        let server_handle = tokio::spawn(async move {
            server.run(token).await;
        });

        loop {
            select! {
                () = self.cancel_token.cancelled() => {
                    break;
                }
                accept_result = self.tcp_listener.accept() => {
                    let Ok((connection, address)) = accept_result else {
                        continue;
                    };
                    if let Err(e) = connection.set_nodelay(true) {
                        log::warn!("Failed to set TCP_NODELAY: {e}");
                    }
                    let (java_client, sender_recv, net_reader) = JavaTcpClient::new(
                        connection,
                        address,
                        self.client_id,
                        self.cancel_token.child_token(),
                        Arc::clone(&self.server),
                        Arc::clone(&self.connection_session),
                        task_tracker.clone(),
                    );
                    self.client_id = self.client_id.wrapping_add(1);
                    log::info!("Accepted connection from Java Edition: {address} (id {})", self.client_id);

                    let java_client = Arc::new(java_client);
                    java_client.start_outgoing_packet_task(sender_recv);
                    java_client.start_incoming_packet_task(net_reader);
                    // Java_client won't drop until the incoming and outcoming task close
                    // So we dont need to care about them here anymore
                }
            }
        }
        let _ = server_handle.await;
    }
}

fn ensure_sololeveling_city_world(
    worlds_config: &mut WorldsConfig,
) -> Result<(), SteelServerError> {
    let Some(domain) = worlds_config
        .domains
        .values_mut()
        .find(|domain| domain.default)
    else {
        return Err(SteelServerError::Core(
            "worlds.toml must declare a default domain before Solo Leveling can add its city world"
                .to_owned(),
        ));
    };

    let city_generator = steel_utils::Identifier::new("sololeveling", "city");
    if let Some(city) = domain
        .worlds
        .iter_mut()
        .find(|world| world.name == city::WORLD_NAME)
    {
        if city.generator != city_generator {
            return Err(SteelServerError::Core(format!(
                "world {} is reserved for generator {city_generator}, found {}",
                city::WORLD_NAME,
                city.generator
            )));
        }
        city.default = true;
        city.config = Some(Value::Table(Map::new()));
    } else {
        domain.worlds.push(WorldEntryConfig {
            name: city::WORLD_NAME.to_owned(),
            generator: city_generator,
            default: true,
            seed: None,
            default_gamemode: None,
            difficulty: None,
            storage: None,
            nether_portal_target: None,
            end_portal_target: None,
            config: Some(Value::Table(Map::new())),
        });
    }

    let has_nether = domain.worlds.iter().any(|world| world.name == "the_nether");
    let has_end = domain.worlds.iter().any(|world| world.name == "the_end");
    for world in &mut domain.worlds {
        world.default = world.name == city::WORLD_NAME;
        if world.name == city::WORLD_NAME {
            if has_nether {
                world.nether_portal_target = Some("the_nether".to_owned());
            }
            if has_end {
                world.end_portal_target = Some("the_end".to_owned());
            }
        } else if world.name == "the_nether" {
            world.nether_portal_target = Some(city::WORLD_NAME.to_owned());
        }
    }

    Ok(())
}

/// Adds the shared island world, generated by `sololeveling:island` and kept
/// on disk like the city.
fn ensure_sololeveling_island_world(
    worlds_config: &mut WorldsConfig,
) -> Result<(), SteelServerError> {
    let Some(domain) = worlds_config
        .domains
        .values_mut()
        .find(|domain| domain.default)
    else {
        return Err(SteelServerError::Core(
            "worlds.toml must declare a default domain before Solo Leveling can add its island"
                .to_owned(),
        ));
    };
    let generator = steel_utils::Identifier::new("sololeveling", "island");
    if let Some(world) = domain
        .worlds
        .iter_mut()
        .find(|world| world.name == island::WORLD_NAME)
    {
        if world.generator != generator {
            return Err(SteelServerError::Core(format!(
                "world {} is reserved for generator {generator}, found {}",
                island::WORLD_NAME,
                world.generator
            )));
        }
        world.config = Some(Value::Table(Map::new()));
        return Ok(());
    }
    domain.worlds.push(WorldEntryConfig {
        name: island::WORLD_NAME.to_owned(),
        generator,
        default: false,
        seed: None,
        default_gamemode: None,
        difficulty: None,
        storage: None,
        nether_portal_target: None,
        end_portal_target: None,
        config: Some(Value::Table(Map::new())),
    });
    Ok(())
}

fn ensure_sololeveling_instance_world(
    steel_config: &mut config::SteelConfig,
) -> Result<(), SteelServerError> {
    let Some(domain) = steel_config
        .worlds
        .domains
        .values_mut()
        .find(|domain| domain.default)
    else {
        return Err(SteelServerError::Core(
            "worlds.toml must declare a default domain before Solo Leveling can add its instance world"
                .to_owned(),
        ));
    };
    if domain
        .worlds
        .iter()
        .any(|world| world.name == "sololeveling_instances")
    {
        return Ok(());
    }

    domain.worlds.push(WorldEntryConfig {
        name: "sololeveling_instances".to_owned(),
        generator: steel_utils::Identifier::from_steel("empty"),
        default: false,
        seed: None,
        default_gamemode: None,
        difficulty: None,
        storage: Some(StorageSelection {
            kind: steel_utils::Identifier::from_steel("ram"),
            config: None,
        }),
        nether_portal_target: None,
        end_portal_target: None,
        config: Some(empty_world_generator_config("overworld")),
    });
    Ok(())
}

/// Builds the generator config `steel:empty` requires: only a dimension type,
/// which fixes the build height and sky of the instance world.
fn empty_world_generator_config(dimension_type: &str) -> Value {
    Value::Table(Map::from_iter([(
        "dimension_type".to_owned(),
        Value::String(format!("minecraft:{dimension_type}")),
    )]))
}

#[cfg(test)]
mod tests {
    use super::{city, ensure_sololeveling_city_world};
    use steel_core::bootstrap::init_globals;
    use steel_core::chunk_saver::registry::WorldStorageRegistry;
    use steel_core::config::WorldsConfig;
    use steel_core::worldgen::WorldGeneratorRegistry;
    use steel_utils::Identifier;

    #[test]
    fn city_becomes_default_without_removing_the_existing_overworld() {
        let mut worlds: WorldsConfig = toml::from_str(
            r#"
save_path = "saves"

[domains.minecraft]
default = true

[[domains.minecraft.worlds]]
name = "overworld"
generator = "minecraft:overworld"
default = true

[[domains.minecraft.worlds]]
name = "the_nether"
generator = "minecraft:the_nether"

[[domains.minecraft.worlds]]
name = "the_end"
generator = "minecraft:the_end"
"#,
        )
        .expect("world config should parse");

        ensure_sololeveling_city_world(&mut worlds).expect("city world should be configured");

        let domain = worlds.domains.get("minecraft").expect("minecraft domain");
        let city = domain
            .worlds
            .iter()
            .find(|world| world.name == city::WORLD_NAME)
            .expect("city world entry");
        assert!(city.default);
        assert_eq!(city.generator, Identifier::new("sololeveling", "city"));
        assert_eq!(city.nether_portal_target.as_deref(), Some("the_nether"));
        assert_eq!(city.end_portal_target.as_deref(), Some("the_end"));

        let overworld = domain
            .worlds
            .iter()
            .find(|world| world.name == "overworld")
            .expect("the original world is retained");
        assert!(!overworld.default);
        assert_eq!(overworld.generator, Identifier::vanilla_static("overworld"));
        assert_eq!(
            domain
                .worlds
                .iter()
                .find(|world| world.name == "the_nether")
                .and_then(|world| world.nether_portal_target.as_deref()),
            Some(city::WORLD_NAME)
        );

        init_globals();
        let mut generators =
            WorldGeneratorRegistry::new_with_builtins().expect("built-in generators");
        city::register_world_generator(&mut generators).expect("city generator");
        let storage = WorldStorageRegistry::new_with_builtins().expect("storage registry");
        let resolved = worlds
            .validate_and_resolve(&generators, &storage)
            .expect("city world configuration");
        assert_eq!(
            resolved.domains[0].default_world,
            Identifier::new("minecraft", city::WORLD_NAME)
        );
    }
}
