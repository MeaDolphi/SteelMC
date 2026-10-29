//! NBT encoding for stacks that live outside a player inventory.
//!
//! The guild bank and the auction ledger both store stacks on disk, so both
//! need the same lossless byte form: components, custom name, lore and the Solo
//! Leveling tags all have to survive the round trip.

use std::io::{self, Cursor, ErrorKind};

use simdnbt::{ToNbtTag, borrow::read_compound as read_borrowed_compound, owned::NbtTag};
use steel_registry::item_stack::ItemStack;

/// Encodes `stack` as one self-contained NBT compound.
pub fn encode(stack: &ItemStack) -> io::Result<Vec<u8>> {
    let NbtTag::Compound(compound) = stack.clone().to_nbt_tag() else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "item stack did not serialize to a compound",
        ));
    };
    let mut bytes = Vec::new();
    compound.write(&mut bytes);
    Ok(bytes)
}

/// Decodes a stack written by [`encode`].
pub fn decode(bytes: &[u8]) -> io::Result<ItemStack> {
    let nbt = read_borrowed_compound(&mut Cursor::new(bytes)).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("failed to parse stored item NBT: {error}"),
        )
    })?;
    let compound = simdnbt::borrow::NbtCompound::from(&nbt);
    ItemStack::from_borrowed_compound(&compound)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "invalid stored item"))
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::*;
    use crate::item::binding;

    #[test]
    fn a_personal_stack_keeps_its_owner_through_the_round_trip() {
        init_vanilla_registry();
        let owner = uuid::Uuid::new_v4();
        let mut stack = ItemStack::new(&vanilla_items::DIAMOND);
        stack.count = 3;
        binding::bind(&mut stack, owner);

        let restored = decode(&encode(&stack).expect("encode")).expect("decode");
        assert_eq!(restored.item.key, stack.item.key);
        assert_eq!(restored.count, 3);
        assert_eq!(binding::bound_owner(&restored), Some(owner));
    }

    #[test]
    fn garbage_bytes_are_rejected_instead_of_becoming_an_empty_stack() {
        assert!(decode(&[0xff, 0x00]).is_err());
    }
}
