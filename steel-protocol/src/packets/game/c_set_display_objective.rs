//! Clientbound set display objective packet - assigns an objective to a display slot
//! (sidebar, tab list, below-name, or one of the per-team sidebar slots).

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_DISPLAY_OBJECTIVE;

/// Where a scoreboard objective is displayed on the client.
///
/// Matches vanilla `DisplaySlot` ordinal order. The `Team*` variants show the objective
/// in the sidebar only for players on the given team color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DisplaySlot {
    List = 0,
    Sidebar = 1,
    BelowName = 2,
    TeamBlack = 3,
    TeamDarkBlue = 4,
    TeamDarkGreen = 5,
    TeamDarkAqua = 6,
    TeamDarkRed = 7,
    TeamDarkPurple = 8,
    TeamGold = 9,
    TeamGray = 10,
    TeamDarkGray = 11,
    TeamBlue = 12,
    TeamGreen = 13,
    TeamAqua = 14,
    TeamRed = 15,
    TeamLightPurple = 16,
    TeamYellow = 17,
    TeamWhite = 18,
}

impl steel_utils::serial::WriteTo for DisplaySlot {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        steel_utils::codec::VarInt(*self as i32).write(writer)
    }
}

/// Assigns an objective to a display slot, or clears the slot when `objective_name` is
/// empty.
#[derive(Debug, Clone, ClientPacket, WriteTo)]
#[packet_id(Play = C_SET_DISPLAY_OBJECTIVE)]
pub struct CSetDisplayObjective {
    pub slot: DisplaySlot,
    #[write(as = Prefixed(VarInt))]
    pub objective_name: String,
}

impl CSetDisplayObjective {
    /// Creates a packet assigning `objective_name` to `slot`.
    #[must_use]
    pub fn new(slot: DisplaySlot, objective_name: impl Into<String>) -> Self {
        Self {
            slot,
            objective_name: objective_name.into(),
        }
    }

    /// Creates a packet that clears `slot` (no objective displayed).
    #[must_use]
    pub const fn clear(slot: DisplaySlot) -> Self {
        Self {
            slot,
            objective_name: String::new(),
        }
    }
}
