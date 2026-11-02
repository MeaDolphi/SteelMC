//! Live 128×128 maps shared by the members of one instance run.

pub(crate) mod castle;
pub(crate) mod gate;

use rustc_hash::FxHashMap;
use steel_core::player::Player;
use steel_protocol::packets::game::{CMapItemData, MapDecoration, MapPatch};
use steel_registry::data_components::components::MapId;
use steel_registry::map_color::{Brightness, MapColor};
use steel_utils::Identifier;
use text_components::TextComponent;
use uuid::Uuid;

pub(crate) const SIZE: usize = 128;
const MAP_ID_BASE: u64 = 1_000_000;

pub(crate) const PLAYER: i32 = 0;
pub(crate) const FRAME: i32 = 1;
pub(crate) const RED_MARKER: i32 = 2;
pub(crate) const BANNER_LIGHT_BLUE: i32 = 13;
pub(crate) const BANNER_PURPLE: i32 = 20;
pub(crate) const RED_X: i32 = 26;

/// Display state of one castle district or ordinary-dungeon room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZoneStatus {
    Unexplored,
    Combat,
    Cleared,
    Arrival,
    Throne,
    ThroneCleared,
}

impl ZoneStatus {
    pub(crate) const fn color(self) -> u8 {
        let material = match self {
            Self::Unexplored => MapColor::COLOR_RED,
            Self::Combat => MapColor::COLOR_ORANGE,
            Self::Cleared => MapColor::COLOR_LIGHT_GREEN,
            Self::Arrival => MapColor::COLOR_LIGHT_BLUE,
            Self::Throne => MapColor::COLOR_PURPLE,
            Self::ThroneCleared => MapColor::GOLD,
        };
        material.packed_id(Brightness::Normal)
    }
}

pub(crate) const WALL: u8 = MapColor::COLOR_BLACK.packed_id(Brightness::High);
pub(crate) const CORRIDOR: u8 = MapColor::COLOR_GRAY.packed_id(Brightness::Normal);

/// Canvas and per-player delivery state of one run's map.
pub(crate) struct DungeonMap {
    id: MapId,
    pub(crate) canvas: [u8; SIZE * SIZE],
    revision: u64,
    sent: FxHashMap<Uuid, (Identifier, u64)>,
}

impl DungeonMap {
    #[must_use]
    pub(crate) fn new(run_id: u64) -> Self {
        Self {
            id: MapId::new((MAP_ID_BASE + run_id) as i32),
            canvas: [WALL; SIZE * SIZE],
            revision: 1,
            sent: FxHashMap::default(),
        }
    }

    #[must_use]
    pub(crate) const fn id(&self) -> MapId {
        self.id
    }

    pub(crate) fn replace_canvas(&mut self, canvas: &[u8; SIZE * SIZE]) {
        if self.canvas != *canvas {
            self.canvas = *canvas;
            self.revision += 1;
        }
    }

    /// Sends a full canvas after a change or world switch, and otherwise only
    /// the moving decorations. Vanilla keeps map data per client level.
    pub(crate) fn sync(&mut self, player: &Player, decorations: Vec<MapDecoration>) {
        let uuid = player.gameprofile.id();
        let world = player.get_world().key.clone();
        let full = self.sent.get(&uuid).is_none_or(|(sent_world, revision)| {
            *sent_world != world || *revision != self.revision
        });
        if full {
            self.sent.insert(uuid, (world, self.revision));
        }
        player.send_packet(CMapItemData {
            map_id: self.id,
            scale: 0,
            locked: true,
            decorations: Some(decorations),
            color_patch: full.then(|| MapPatch::full(self.canvas.to_vec())),
        });
    }
}

/// Converts a canvas position to vanilla's signed map-decoration coordinates.
#[must_use]
pub(crate) fn map_coordinate(value: f64) -> i8 {
    (value.clamp(0.0, 127.0) * 2.0 - 128.0).round() as i8
}

#[must_use]
pub(crate) fn decoration(
    kind: i32,
    x: f64,
    y: f64,
    rotation: i8,
    name: Option<&str>,
) -> MapDecoration {
    MapDecoration::new(
        kind,
        map_coordinate(x),
        map_coordinate(y),
        rotation,
        name.map(|name| TextComponent::plain(name.to_owned())),
    )
}

/// Vanilla quantizes yaw to sixteen map-arrow directions.
#[must_use]
pub(crate) fn player_rotation(yaw: f32) -> i8 {
    (f64::from(yaw) * 16.0 / 360.0 + 0.5).floor() as i8 & 15
}

#[cfg(test)]
mod tests {
    use super::{map_coordinate, player_rotation};

    #[test]
    fn map_coordinates_and_rotation_follow_vanilla_quantization() {
        assert_eq!(map_coordinate(0.0), -128);
        assert_eq!(map_coordinate(64.0), 0);
        assert_eq!(map_coordinate(127.0), 126);
        assert_eq!(player_rotation(0.0), 0);
        assert_eq!(player_rotation(90.0), 4);
        assert_eq!(player_rotation(-90.0), 12);
    }
}
