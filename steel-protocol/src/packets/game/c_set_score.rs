//! Clientbound set score packet - sets a player's score for a scoreboard objective.

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_SET_SCORE;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo};
use text_components::TextComponent;

use super::number_format::NumberFormat;

/// Sets the score of `owner` (a player name or entity UUID string) on `objective_name`.
#[derive(Debug, Clone, ClientPacket)]
#[packet_id(Play = C_SET_SCORE)]
pub struct CSetScore {
    pub owner: String,
    pub objective_name: String,
    pub score: i32,
    pub display: Option<TextComponent>,
    pub number_format: Option<NumberFormat>,
}

impl WriteTo for CSetScore {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        self.owner.write_prefixed::<VarInt>(writer)?;
        self.objective_name.write_prefixed::<VarInt>(writer)?;
        VarInt(self.score).write(writer)?;
        self.display.write(writer)?;
        self.number_format.write(writer)
    }
}

impl CSetScore {
    /// Creates a packet setting a plain integer score with no custom display or format.
    #[must_use]
    pub fn new(owner: impl Into<String>, objective_name: impl Into<String>, score: i32) -> Self {
        Self {
            owner: owner.into(),
            objective_name: objective_name.into(),
            score,
            display: None,
            number_format: None,
        }
    }

    /// Creates a packet setting a score with a custom display name and/or number format.
    #[must_use]
    pub fn with_display(
        owner: impl Into<String>,
        objective_name: impl Into<String>,
        score: i32,
        display: Option<TextComponent>,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            owner: owner.into(),
            objective_name: objective_name.into(),
            score,
            display,
            number_format,
        }
    }
}
