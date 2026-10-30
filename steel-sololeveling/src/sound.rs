//! System sounds from the server resource pack.
//!
//! Each sound is an event in the pack's `sounds.json`; a client without the
//! pack simply hears nothing. The ids here must match
//! `resourcepack/tools/build.py`.

use steel_core::player::Player;

use crate::identifier::from_sololeveling;

/// One System sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemSound {
    /// The System window opens.
    Open,
    /// The hunter levels up.
    LevelUp,
    /// The daily quest is complete.
    Quest,
    /// A shadow rises from a kill.
    Arise,
    /// A reward, purchase or enchantment lands.
    Reward,
}

/// Every System sound, as the pack must provide them.
pub const ALL: [SystemSound; 5] = [
    SystemSound::Open,
    SystemSound::LevelUp,
    SystemSound::Quest,
    SystemSound::Arise,
    SystemSound::Reward,
];

impl SystemSound {
    /// Returns the sound event path inside the `sololeveling` namespace.
    #[must_use]
    pub const fn event(self) -> &'static str {
        match self {
            Self::Open => "system.open",
            Self::LevelUp => "system.level_up",
            Self::Quest => "system.quest",
            Self::Arise => "system.arise",
            Self::Reward => "system.reward",
        }
    }

    /// Plays the sound to `player` alone.
    pub fn play(self, player: &Player) {
        player.play_sound_to_self(from_sololeveling(self.event()), 0.8, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::ALL;

    #[test]
    fn every_system_sound_exists_in_the_resource_pack() {
        let assets =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("resourcepack/pack/assets/sololeveling");
        let sounds: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(assets.join("sounds.json"))
                .expect("resource pack has sounds.json; run resourcepack/tools/build.py"),
        )
        .expect("sounds.json is JSON");
        for sound in ALL {
            let entry = &sounds[sound.event()];
            let file = entry["sounds"][0]
                .as_str()
                .unwrap_or_else(|| panic!("sounds.json lacks {}", sound.event()));
            let path = file.trim_start_matches("sololeveling:");
            assert!(
                assets.join(format!("sounds/{path}.ogg")).is_file(),
                "resource pack lacks the audio of {}",
                sound.event()
            );
        }
    }
}
