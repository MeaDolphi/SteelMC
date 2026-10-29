//! Clientbound set objective packet - adds, updates, or removes a scoreboard objective.

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_SET_OBJECTIVE;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo};
use text_components::TextComponent;

use super::number_format::NumberFormat;

/// Render type for an objective's score display.
///
/// Matches vanilla `ObjectiveCriteria.RenderType` ordinal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectiveRenderType {
    Integer = 0,
    Hearts = 1,
}

impl WriteTo for ObjectiveRenderType {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        VarInt(*self as i32).write(writer)
    }
}

/// The method carried by a [`CSetObjective`] packet.
///
/// Matches vanilla `ClientboundSetObjectivePacket`'s `method` field: `Add`/`Change` (0/2)
/// carry the objective's display name, render type, and number format; `Remove` (1)
/// carries nothing else.
#[derive(Debug, Clone)]
pub enum SetObjectiveMethod {
    Add {
        display_name: TextComponent,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    },
    Remove,
    Change {
        display_name: TextComponent,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    },
}

impl SetObjectiveMethod {
    const fn discriminant(&self) -> u8 {
        match self {
            Self::Add { .. } => 0,
            Self::Remove => 1,
            Self::Change { .. } => 2,
        }
    }
}

impl WriteTo for SetObjectiveMethod {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        self.discriminant().write(writer)?;
        match self {
            Self::Add {
                display_name,
                render_type,
                number_format,
            }
            | Self::Change {
                display_name,
                render_type,
                number_format,
            } => {
                display_name.write(writer)?;
                render_type.write(writer)?;
                number_format.write(writer)
            }
            Self::Remove => Ok(()),
        }
    }
}

/// Adds, updates, or removes a scoreboard objective.
///
/// The objective name is a server-chosen internal identifier; vanilla clients never
/// display it directly, only the `display_name` carried by [`SetObjectiveMethod::Add`]
/// and [`SetObjectiveMethod::Change`].
#[derive(Debug, Clone, ClientPacket)]
#[packet_id(Play = C_SET_OBJECTIVE)]
pub struct CSetObjective {
    pub objective_name: String,
    pub method: SetObjectiveMethod,
}

impl WriteTo for CSetObjective {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        self.objective_name.write_prefixed::<VarInt>(writer)?;
        self.method.write(writer)
    }
}

impl CSetObjective {
    /// Creates a packet that adds a new objective.
    #[must_use]
    pub fn add(
        objective_name: impl Into<String>,
        display_name: impl Into<TextComponent>,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: SetObjectiveMethod::Add {
                display_name: display_name.into(),
                render_type,
                number_format,
            },
        }
    }

    /// Creates a packet that removes an existing objective.
    #[must_use]
    pub fn remove(objective_name: impl Into<String>) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: SetObjectiveMethod::Remove,
        }
    }

    /// Creates a packet that changes an existing objective's display name/render type.
    #[must_use]
    pub fn change(
        objective_name: impl Into<String>,
        display_name: impl Into<TextComponent>,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: SetObjectiveMethod::Change {
                display_name: display_name.into(),
                render_type,
                number_format,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::codec::VarInt;
    use steel_utils::serial::{PrefixedWrite as _, WriteTo as _};
    use text_components::TextComponent;

    use super::{CSetObjective, ObjectiveRenderType};

    #[test]
    fn remove_writes_name_then_method_byte_and_nothing_else() {
        let packet = CSetObjective::remove("sololeveling.level");

        let mut encoded = Vec::new();
        let Ok(()) = packet.write(&mut encoded) else {
            panic!("objective remove packet should encode");
        };

        let mut expected = Vec::new();
        let Ok(()) = "sololeveling.level".write_prefixed::<VarInt>(&mut expected) else {
            panic!("objective name should encode");
        };
        expected.push(1); // METHOD_REMOVE

        assert_eq!(encoded, expected);
    }

    #[test]
    fn add_without_number_format_writes_absent_flag() {
        let packet = CSetObjective::add(
            "sololeveling.level",
            TextComponent::plain("Level"),
            ObjectiveRenderType::Integer,
            None,
        );

        let mut encoded = Vec::new();
        let Ok(()) = packet.write(&mut encoded) else {
            panic!("objective add packet should encode");
        };

        let mut expected = Vec::new();
        let Ok(()) = "sololeveling.level".write_prefixed::<VarInt>(&mut expected) else {
            panic!("objective name should encode");
        };
        expected.push(0); // METHOD_ADD
        let name_nbt = TextComponent::plain("Level").to_codec_nbt();
        name_nbt.write(&mut expected);
        expected.push(0); // RenderType::INTEGER
        expected.push(0); // number_format absent

        assert_eq!(encoded, expected);
    }
}
