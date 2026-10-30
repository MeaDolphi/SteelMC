//! Serving the Solo Leveling resource pack to vanilla clients.
//!
//! The pack zip lives next to the Solo Leveling data as `resourcepack.zip`
//! (built by `resourcepack/tools/build.py`). A tiny HTTP listener serves it,
//! and every hunter is offered it on join with its SHA-1, so clients cache
//! the download until the pack changes. Without the zip nothing is served
//! and items fall back to their vanilla look.
//!
//! `resourcepack.json` next to the zip sets the listener address and the URL
//! clients download from; it is written with local-test defaults when absent.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use steel_core::player::Player;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use uuid::Uuid;

/// Pack zip file name inside the Solo Leveling data directory.
pub const ZIP_FILE: &str = "resourcepack.zip";
/// Settings file name inside the Solo Leveling data directory.
pub const CONFIG_FILE: &str = "resourcepack.json";
/// Stable pack id, so a new offer replaces the previous pack on the client.
const PACK_ID: Uuid = Uuid::from_u128(0x5010_1e7e_11e6_4a9c_9b3a_5e5e_7e5e_0001);

/// Where the pack is served and how it is offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackConfig {
    /// Whether the pack is served and offered at all.
    pub enabled: bool,
    /// Address the HTTP listener binds, e.g. `0.0.0.0:25580`.
    pub bind: String,
    /// URL clients download from; must reach `bind` from the players' side.
    pub public_url: String,
    /// Whether declining the pack disconnects the player.
    pub required: bool,
}

impl Default for PackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: "0.0.0.0:25580".to_owned(),
            public_url: "http://127.0.0.1:25580/sololeveling.zip".to_owned(),
            required: false,
        }
    }
}

/// A loaded pack ready to offer.
#[derive(Debug, Clone)]
pub struct ServedPack {
    url: String,
    hash: String,
    required: bool,
}

/// Returns the lowercase hex SHA-1 the client checks the download against.
#[must_use]
pub fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .fold(String::with_capacity(40), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Reads the settings, writing the defaults when there are none yet.
fn load_config(dir: &Path) -> io::Result<PackConfig> {
    let path = dir.join(CONFIG_FILE);
    if !path.exists() {
        let config = PackConfig::default();
        let text = serde_json::to_string_pretty(&config).map_err(io::Error::other)?;
        fs::write(&path, text)?;
        return Ok(config);
    }
    let text = fs::read_to_string(&path)?;
    serde_json::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Loads the pack and starts serving it, or returns `None` when disabled.
///
/// A missing zip or an unusable address only disables the pack: the server
/// keeps running with vanilla item looks.
pub async fn start(dir: &Path) -> Option<ServedPack> {
    let config = match load_config(dir) {
        Ok(config) => config,
        Err(error) => {
            log::warn!("sololeveling: resource pack settings unreadable: {error}");
            return None;
        }
    };
    if !config.enabled {
        return None;
    }
    let zip = match tokio_fs::read(dir.join(ZIP_FILE)).await {
        Ok(zip) => Arc::new(zip),
        Err(error) => {
            log::info!(
                "sololeveling: no {ZIP_FILE} in {} ({error}); resource pack disabled",
                dir.display()
            );
            return None;
        }
    };
    let listener = match TcpListener::bind(&config.bind).await {
        Ok(listener) => listener,
        Err(error) => {
            log::warn!(
                "sololeveling: resource pack listener {} failed: {error}",
                config.bind
            );
            return None;
        }
    };
    let hash = sha1_hex(&zip);
    log::info!(
        "sololeveling: serving resource pack ({} bytes, sha1 {hash}) at {}",
        zip.len(),
        config.public_url
    );
    tokio::spawn(serve(listener, zip));
    Some(ServedPack {
        url: config.public_url,
        hash,
        required: config.required,
    })
}

/// Answers every HTTP request with the pack zip.
async fn serve(listener: TcpListener, zip: Arc<Vec<u8>>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            continue;
        };
        let zip = Arc::clone(&zip);
        tokio::spawn(async move {
            // The request itself is irrelevant: there is only one file.
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                zip.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&zip).await;
            }
            let _ = socket.shutdown().await;
        });
    }
}

impl ServedPack {
    /// Offers the pack to one hunter.
    pub fn offer(&self, player: &Player) {
        let prompt =
            TextComponent::plain("Ресурс-пак Системы: иконки артефактов, кристаллов и наград.")
                .color(Color::Aqua);
        player.send_resource_pack(PACK_ID, &self.url, &self.hash, self.required, Some(&prompt));
    }
}

#[cfg(test)]
mod tests {
    use super::{PackConfig, sha1_hex};

    #[test]
    fn the_hash_is_lowercase_hex_sha1() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn settings_round_trip_through_json() {
        let config = PackConfig::default();
        let text = serde_json::to_string(&config).expect("encode");
        assert_eq!(
            serde_json::from_str::<PackConfig>(&text).expect("decode"),
            config
        );
    }
}
