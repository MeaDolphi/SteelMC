//! Rank badges drawn by the resource pack's `sololeveling:rank` font.
//!
//! Each rank maps to one private-use character that the pack's bitmap font
//! renders as a coloured plate with the rank letter. The code points must
//! match `RANK_GLYPHS` in `resourcepack/tools/build.py`.

use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};

use crate::rank::HunterRank;

/// The font that draws rank badges.
pub const FONT: &str = "sololeveling:rank";

/// Returns the private-use character of a rank's badge.
#[must_use]
pub const fn glyph(rank: HunterRank) -> char {
    match rank {
        HunterRank::E => '\u{E101}',
        HunterRank::D => '\u{E102}',
        HunterRank::C => '\u{E103}',
        HunterRank::B => '\u{E104}',
        HunterRank::A => '\u{E105}',
        HunterRank::S => '\u{E106}',
    }
}

/// Returns the badge of `rank` alone.
///
/// Text colour tints a bitmap glyph, so the badge is white to keep its own
/// colours; callers put the rank letter next to it for clients without the
/// pack.
#[must_use]
pub fn badge(rank: HunterRank) -> TextComponent {
    TextComponent::plain(glyph(rank).to_string())
        .font(FONT)
        .color(Color::White)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::glyph;
    use crate::rank::HunterRank;

    #[test]
    fn every_rank_badge_exists_in_the_resource_pack() {
        let font = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resourcepack/pack/assets/sololeveling/font/rank.json");
        let font: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(font)
                .expect("resource pack has the rank font; run resourcepack/tools/build.py"),
        )
        .expect("rank font is JSON");
        let chars: Vec<String> = font["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .filter_map(|provider| provider["chars"][0].as_str().map(ToOwned::to_owned))
            .collect();
        for rank in [
            HunterRank::E,
            HunterRank::D,
            HunterRank::C,
            HunterRank::B,
            HunterRank::A,
            HunterRank::S,
        ] {
            assert!(
                chars.contains(&glyph(rank).to_string()),
                "rank font lacks the badge of {rank:?}"
            );
        }
    }
}
