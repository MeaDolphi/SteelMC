use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RESOURCE_PACK_PUSH;
use text_components::TextComponent;
use uuid::Uuid;

/// Asks the client to download and apply a server resource pack.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RESOURCE_PACK_PUSH)]
pub struct CResourcePackPush {
    /// Identity of the pack, so a later push with the same id replaces it.
    pub id: Uuid,
    /// HTTP(S) address the client downloads the zip from.
    #[write(as = Prefixed(VarInt))]
    pub url: String,
    /// Lowercase hex SHA-1 of the zip; the client re-downloads when it changes.
    #[write(as = Prefixed(VarInt))]
    pub hash: String,
    /// Whether declining the pack disconnects the player.
    pub required: bool,
    /// Text shown in the client's download prompt.
    pub prompt: Option<TextComponent>,
}
