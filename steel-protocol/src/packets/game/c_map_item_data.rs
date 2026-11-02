//! Clientbound map item data packet - updates map colors and decorations.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::data_components::components::MapId;
use steel_registry::packets::play::C_MAP_ITEM_DATA;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;
use text_components::TextComponent;

/// One icon drawn over a map.
#[derive(Debug, Clone)]
pub struct MapDecoration {
    pub type_id: i32,
    pub x: i8,
    pub y: i8,
    pub rotation: i8,
    pub name: Option<TextComponent>,
}

impl MapDecoration {
    #[must_use]
    pub const fn new(
        type_id: i32,
        x: i8,
        y: i8,
        rotation: i8,
        name: Option<TextComponent>,
    ) -> Self {
        Self {
            type_id,
            x,
            y,
            rotation: rotation & 15,
            name,
        }
    }
}

impl WriteTo for MapDecoration {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.type_id).write(writer)?;
        self.x.write(writer)?;
        self.y.write(writer)?;
        self.rotation.write(writer)?;
        self.name.write(writer)
    }
}

/// A rectangular update of the 128×128 map color canvas.
#[derive(Debug, Clone)]
pub struct MapPatch {
    pub start_x: u8,
    pub start_y: u8,
    pub width: u8,
    pub height: u8,
    pub colors: Vec<u8>,
}

impl MapPatch {
    #[must_use]
    pub const fn full(colors: Vec<u8>) -> Self {
        Self {
            start_x: 0,
            start_y: 0,
            width: 128,
            height: 128,
            colors,
        }
    }
}

/// Updates one client-side map. The color patch has no optional boolean:
/// vanilla uses a zero width for absence, exactly like an empty patch.
#[derive(Debug, Clone, ClientPacket)]
#[packet_id(Play = C_MAP_ITEM_DATA)]
pub struct CMapItemData {
    pub map_id: MapId,
    pub scale: i8,
    pub locked: bool,
    pub decorations: Option<Vec<MapDecoration>>,
    pub color_patch: Option<MapPatch>,
}

impl WriteTo for CMapItemData {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.map_id.write(writer)?;
        self.scale.write(writer)?;
        self.locked.write(writer)?;
        self.decorations.write(writer)?;
        let Some(patch) = &self.color_patch else {
            return 0_u8.write(writer);
        };
        patch.width.write(writer)?;
        if patch.width == 0 {
            return Ok(());
        }
        patch.height.write(writer)?;
        patch.start_x.write(writer)?;
        patch.start_y.write(writer)?;
        patch.colors.write(writer)
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::components::MapId;
    use steel_utils::serial::WriteTo;
    use text_components::TextComponent;

    use super::{CMapItemData, MapDecoration, MapPatch};

    #[test]
    fn writes_fields_in_vanilla_stream_codec_order() {
        let packet = CMapItemData {
            map_id: MapId::new(7),
            scale: 2,
            locked: true,
            decorations: Some(vec![MapDecoration::new(
                3,
                -4,
                120,
                19,
                Some(TextComponent::plain("Exit")),
            )]),
            color_patch: Some(MapPatch {
                start_x: 5,
                start_y: 6,
                width: 2,
                height: 1,
                colors: vec![44, 45],
            }),
        };
        let mut encoded = Vec::new();
        packet
            .write(&mut encoded)
            .expect("map packet should encode");

        let mut expected = vec![7, 2, 1, 1, 1, 3, 252, 120, 3, 1];
        let name = TextComponent::plain("Exit").to_codec_nbt();
        let mut name_bytes = Vec::new();
        name.write(&mut name_bytes);
        expected.extend_from_slice(&name_bytes);
        expected.extend_from_slice(&[2, 1, 5, 6, 2, 44, 45]);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn absent_decorations_and_patch_write_zero_markers() {
        let packet = CMapItemData {
            map_id: MapId::new(1),
            scale: 0,
            locked: false,
            decorations: None,
            color_patch: None,
        };
        let mut encoded = Vec::new();
        packet
            .write(&mut encoded)
            .expect("map packet should encode");
        assert_eq!(encoded, [1, 0, 0, 0, 0]);
    }
}
