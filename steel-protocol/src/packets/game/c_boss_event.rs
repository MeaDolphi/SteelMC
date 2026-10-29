//! Clientbound boss event packet - adds, updates, or removes a boss bar on the client.

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_BOSS_EVENT;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;
use text_components::TextComponent;
use uuid::Uuid;

/// Boss bar color, matches vanilla `BossEvent.BossBarColor` ordinal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BossBarColor {
    Pink = 0,
    Blue = 1,
    Red = 2,
    Green = 3,
    Yellow = 4,
    Purple = 5,
    White = 6,
}

impl WriteTo for BossBarColor {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        VarInt(*self as i32).write(writer)
    }
}

/// Boss bar overlay (segment style), matches vanilla `BossEvent.BossBarOverlay` ordinal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BossBarOverlay {
    Progress = 0,
    Notched6 = 1,
    Notched10 = 2,
    Notched12 = 3,
    Notched20 = 4,
}

impl WriteTo for BossBarOverlay {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        VarInt(*self as i32).write(writer)
    }
}

/// Extra boss bar flags, matches vanilla `ClientboundBossEventPacket` flag bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BossBarProperties {
    pub darken_screen: bool,
    pub play_music: bool,
    pub create_world_fog: bool,
}

impl WriteTo for BossBarProperties {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        let mut flags: u8 = 0;
        if self.darken_screen {
            flags |= 1;
        }
        if self.play_music {
            flags |= 2;
        }
        if self.create_world_fog {
            flags |= 4;
        }
        flags.write(writer)
    }
}

/// The operation carried by a [`CBossEvent`] packet.
///
/// Matches the operations in vanilla `ClientboundBossEventPacket`: each operation type
/// has its own `VarInt` discriminant (the `OperationType` enum ordinal) followed by its
/// own payload.
#[derive(Debug, Clone)]
pub enum BossEventOperation {
    Add {
        name: TextComponent,
        progress: f32,
        color: BossBarColor,
        overlay: BossBarOverlay,
        properties: BossBarProperties,
    },
    Remove,
    UpdateProgress {
        progress: f32,
    },
    UpdateName {
        name: TextComponent,
    },
    UpdateStyle {
        color: BossBarColor,
        overlay: BossBarOverlay,
    },
    UpdateProperties {
        properties: BossBarProperties,
    },
}

impl BossEventOperation {
    const fn discriminant(&self) -> i32 {
        match self {
            Self::Add { .. } => 0,
            Self::Remove => 1,
            Self::UpdateProgress { .. } => 2,
            Self::UpdateName { .. } => 3,
            Self::UpdateStyle { .. } => 4,
            Self::UpdateProperties { .. } => 5,
        }
    }
}

impl WriteTo for BossEventOperation {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        VarInt(self.discriminant()).write(writer)?;
        match self {
            Self::Add {
                name,
                progress,
                color,
                overlay,
                properties,
            } => {
                name.write(writer)?;
                progress.write(writer)?;
                color.write(writer)?;
                overlay.write(writer)?;
                properties.write(writer)
            }
            Self::Remove => Ok(()),
            Self::UpdateProgress { progress } => progress.write(writer),
            Self::UpdateName { name } => name.write(writer),
            Self::UpdateStyle { color, overlay } => {
                color.write(writer)?;
                overlay.write(writer)
            }
            Self::UpdateProperties { properties } => properties.write(writer),
        }
    }
}

/// Adds, updates, or removes a boss bar on the client.
///
/// `id` identifies the boss bar instance; the client matches it across add/update/remove
/// packets. Used here for gate/dungeon timers displayed via the vanilla boss bar HUD.
#[derive(Debug, Clone, ClientPacket)]
#[packet_id(Play = C_BOSS_EVENT)]
pub struct CBossEvent {
    pub id: Uuid,
    pub operation: BossEventOperation,
}

impl WriteTo for CBossEvent {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        self.id.write(writer)?;
        self.operation.write(writer)
    }
}

impl CBossEvent {
    /// Creates a packet that adds a new boss bar.
    #[must_use]
    pub const fn add(
        id: Uuid,
        name: TextComponent,
        progress: f32,
        color: BossBarColor,
        overlay: BossBarOverlay,
        properties: BossBarProperties,
    ) -> Self {
        Self {
            id,
            operation: BossEventOperation::Add {
                name,
                progress,
                color,
                overlay,
                properties,
            },
        }
    }

    /// Creates a packet that removes an existing boss bar.
    #[must_use]
    pub const fn remove(id: Uuid) -> Self {
        Self {
            id,
            operation: BossEventOperation::Remove,
        }
    }

    /// Creates a packet that updates the progress (fill fraction, 0.0-1.0) of a boss bar.
    #[must_use]
    pub const fn update_progress(id: Uuid, progress: f32) -> Self {
        Self {
            id,
            operation: BossEventOperation::UpdateProgress { progress },
        }
    }

    /// Creates a packet that updates the name of a boss bar.
    #[must_use]
    pub const fn update_name(id: Uuid, name: TextComponent) -> Self {
        Self {
            id,
            operation: BossEventOperation::UpdateName { name },
        }
    }

    /// Creates a packet that updates the color and overlay style of a boss bar.
    #[must_use]
    pub const fn update_style(id: Uuid, color: BossBarColor, overlay: BossBarOverlay) -> Self {
        Self {
            id,
            operation: BossEventOperation::UpdateStyle { color, overlay },
        }
    }

    /// Creates a packet that updates the darken-screen/music/fog flags of a boss bar.
    #[must_use]
    pub const fn update_properties(id: Uuid, properties: BossBarProperties) -> Self {
        Self {
            id,
            operation: BossEventOperation::UpdateProperties { properties },
        }
    }
}

#[cfg(test)]
mod tests {
    use text_components::TextComponent;
    use uuid::Uuid;

    use super::{BossBarColor, BossBarOverlay, BossBarProperties, CBossEvent};

    #[test]
    fn add_writes_id_then_discriminant_then_fields_in_vanilla_order() {
        let id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let packet = CBossEvent::add(
            id,
            TextComponent::plain("Dungeon timer"),
            0.5,
            BossBarColor::Red,
            BossBarOverlay::Notched10,
            BossBarProperties {
                darken_screen: true,
                play_music: false,
                create_world_fog: true,
            },
        );

        let mut encoded = Vec::new();
        let Ok(()) = steel_utils::serial::WriteTo::write(&packet, &mut encoded) else {
            panic!("boss event add packet should encode");
        };

        let mut expected = Vec::new();
        expected.extend_from_slice(&id.as_u64_pair().0.to_be_bytes());
        expected.extend_from_slice(&id.as_u64_pair().1.to_be_bytes());
        expected.push(0); // OperationType::ADD ordinal
        let name = TextComponent::plain("Dungeon timer").to_codec_nbt();
        let mut name_bytes = Vec::new();
        name.write(&mut name_bytes);
        expected.extend_from_slice(&name_bytes);
        expected.extend_from_slice(&0.5_f32.to_be_bytes());
        expected.push(BossBarColor::Red as u8); // VarInt(2)
        expected.push(BossBarOverlay::Notched10 as u8); // VarInt(2)
        expected.push(1 | 4); // darken_screen | create_world_fog

        assert_eq!(encoded, expected);
    }

    #[test]
    fn remove_writes_id_then_discriminant_and_nothing_else() {
        let id = Uuid::from_u128(1);
        let packet = CBossEvent::remove(id);

        let mut encoded = Vec::new();
        let Ok(()) = steel_utils::serial::WriteTo::write(&packet, &mut encoded) else {
            panic!("boss event remove packet should encode");
        };

        let mut expected = Vec::new();
        expected.extend_from_slice(&id.as_u64_pair().0.to_be_bytes());
        expected.extend_from_slice(&id.as_u64_pair().1.to_be_bytes());
        expected.push(1); // OperationType::REMOVE ordinal

        assert_eq!(encoded, expected);
    }
}
