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

use steel_core::config::{StorageSelection, WorldEntryConfig};
use steel_core::gameplay_hooks::GameplayHooks;
use steel_core::{
    GIT_HASH_SHORT, command::CommandRegistry, permission::PermissionGroupManager, server::Server,
};
use steel_login::{JavaTcpClient, ServerConnectionSession};
use steel_sololeveling::{commands, hooks::SoloLevelingHooks, state::SoloLevelingState};
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
        mut steel_config: config::SteelConfig,
    ) -> Result<Self, SteelServerError> {
        ensure_sololeveling_instance_world(&mut steel_config)?;
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
        steel_config: config::SteelConfig,
        command_registry: CommandRegistry,
        gameplay_hooks: Arc<dyn GameplayHooks>,
    ) -> Result<Self, SteelServerError> {
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

        let server = Server::new_with_commands(
            chunk_runtime,
            cancel_token.clone(),
            runtime_config,
            worlds_config,
            permission_groups,
            command_registry,
            gameplay_hooks,
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
