//! The `NumberFormat` value used by scoreboard score/objective packets to control how a
//! score number is rendered on the client (plain integer, hidden, styled, or replaced by
//! arbitrary text).
//!
//! Matches vanilla `net.minecraft.network.chat.numbers.NumberFormat` and its three
//! implementations (`BlankFormat`, `StyledFormat`, `FixedFormat`). On the wire this is a
//! registry-dispatched value: a `VarInt` registry ID for `minecraft:number_format_type`
//! followed by the variant's own payload. `number_format_type` is a built-in (non synced)
//! registry, so its IDs are fixed by vanilla's bootstrap order rather than by a datapack
//! sync packet: `blank` = 0, `styled` = 1, `fixed` = 2 (see
//! `NumberFormatTypes.bootstrap` / `BuiltInRegistries.NUMBER_FORMAT_TYPE`).

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;
use text_components::TextComponent;
use text_components::format::{Color, Format};

/// Encodes the style fields of a [`Format`] as the NBT compound vanilla's
/// `Style.Serializer.MAP_CODEC` produces.
///
/// `text_components::TextComponent`'s own NBT encoding always includes a text-content key
/// (`text`/`translate`/...), which `Style`'s compound never has, so this is encoded
/// directly from the style fields instead of going through `TextComponent::to_codec_nbt`.
/// Click/hover/insertion interactivity is intentionally not included: vanilla's `Style`
/// carries them too, but they are meaningless for a scoreboard number and `Format` (the
/// subset of `Style` this crate's `text_components` exposes) does not carry them anyway.
fn style_to_nbt(format: &Format) -> NbtTag {
    let mut entries = Vec::new();

    if let Some(color) = &format.color {
        let value = match color {
            Color::Black => "black".to_string(),
            Color::DarkBlue => "dark_blue".to_string(),
            Color::DarkGreen => "dark_green".to_string(),
            Color::DarkAqua => "dark_aqua".to_string(),
            Color::DarkRed => "dark_red".to_string(),
            Color::DarkPurple => "dark_purple".to_string(),
            Color::Gold => "gold".to_string(),
            Color::Gray => "gray".to_string(),
            Color::DarkGray => "dark_gray".to_string(),
            Color::Blue => "blue".to_string(),
            Color::Green => "green".to_string(),
            Color::Aqua => "aqua".to_string(),
            Color::Red => "red".to_string(),
            Color::LightPurple => "light_purple".to_string(),
            Color::Yellow => "yellow".to_string(),
            Color::White => "white".to_string(),
            Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        };
        entries.push(("color".into(), NbtTag::String(value.into())));
    }
    if let Some(font) = &format.font {
        entries.push(("font".into(), NbtTag::String(font.as_ref().into())));
    }
    if let Some(value) = format.bold {
        entries.push(("bold".into(), NbtTag::Byte(i8::from(value))));
    }
    if let Some(value) = format.italic {
        entries.push(("italic".into(), NbtTag::Byte(i8::from(value))));
    }
    if let Some(value) = format.underlined {
        entries.push(("underlined".into(), NbtTag::Byte(i8::from(value))));
    }
    if let Some(value) = format.strikethrough {
        entries.push(("strikethrough".into(), NbtTag::Byte(i8::from(value))));
    }
    if let Some(value) = format.obfuscated {
        entries.push(("obfuscated".into(), NbtTag::Byte(i8::from(value))));
    }
    if let Some(color) = format.shadow_color {
        entries.push(("shadow_color".into(), NbtTag::Int(color)));
    }

    NbtTag::Compound(NbtCompound::from_values(entries))
}

/// How a scoreboard score number should be rendered on the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberFormat {
    /// Renders nothing in place of the number.
    Blank,
    /// Renders the number as plain text styled with the given [`Format`].
    Styled(Format),
    /// Replaces the number entirely with the given text.
    ///
    /// Boxed because `TextComponent` is much larger than the other variants; without it
    /// every `NumberFormat` would pay the size of the biggest variant.
    Fixed(Box<TextComponent>),
}

impl NumberFormat {
    const fn registry_id(&self) -> i32 {
        match self {
            Self::Blank => 0,
            Self::Styled(_) => 1,
            Self::Fixed(_) => 2,
        }
    }
}

impl WriteTo for NumberFormat {
    fn write(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        VarInt(self.registry_id()).write(writer)?;
        match self {
            Self::Blank => Ok(()),
            Self::Styled(format) => WriteTo::write(&style_to_nbt(format), writer),
            Self::Fixed(component) => component.as_ref().write(writer),
        }
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::{NbtCompound, NbtTag};
    use text_components::{Modifier as _, TextComponent, format::Color};

    use super::NumberFormat;

    #[test]
    fn blank_writes_only_the_registry_id() {
        let mut encoded = Vec::new();
        let Ok(()) = steel_utils::serial::WriteTo::write(&NumberFormat::Blank, &mut encoded) else {
            panic!("blank number format should encode");
        };

        assert_eq!(encoded, vec![0]);
    }

    #[test]
    fn styled_writes_registry_id_then_style_only_nbt_compound() {
        let format = TextComponent::plain("").color(Color::Red).bold(true).format;
        let number_format = NumberFormat::Styled(format);

        let mut encoded = Vec::new();
        let Ok(()) = steel_utils::serial::WriteTo::write(&number_format, &mut encoded) else {
            panic!("styled number format should encode");
        };

        let mut expected = vec![1]; // registry id for `styled`
        let style_nbt = NbtTag::Compound(NbtCompound::from_values(vec![
            ("color".into(), NbtTag::String("red".into())),
            ("bold".into(), NbtTag::Byte(1)),
        ]));
        style_nbt.write(&mut expected);

        assert_eq!(encoded, expected);
    }

    #[test]
    fn fixed_writes_registry_id_then_full_component_nbt() {
        let component = TextComponent::plain("42");
        let number_format = NumberFormat::Fixed(Box::new(component.clone()));

        let mut encoded = Vec::new();
        let Ok(()) = steel_utils::serial::WriteTo::write(&number_format, &mut encoded) else {
            panic!("fixed number format should encode");
        };

        let mut expected = vec![2]; // registry id for `fixed`
        component.to_codec_nbt().write(&mut expected);

        assert_eq!(encoded, expected);
    }
}
