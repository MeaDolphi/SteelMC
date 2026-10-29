//! Personal ownership of a reward item.
//!
//! A personal System-shop purchase is marked with its owner's `Uuid`, and that
//! mark travels with the item: vanilla only merges stacks whose components are
//! identical, so a bound and an unbound copy of the same item never combine and
//! can never silently lose (or gain) a binding. Reading the mark is the single
//! check every transfer route uses.

use simdnbt::owned::NbtCompound;
use steel_registry::item_stack::ItemStack;
use uuid::Uuid;

use super::{read_tag, record, tag_string, write_tag};

/// Tag field holding the owner of a personal item.
const BOUND_TO: &str = "bound_to";
/// Kind stored in the record of an item that only carries a binding.
const BOUND_KIND: &str = "bound";

/// Binds `stack` to `owner`, keeping an existing binding unchanged.
///
/// An upgrade or rename rebuilds the stack; it must pass the original owner
/// here so the personal flag survives.
pub fn bind(stack: &mut ItemStack, owner: Uuid) {
    if bound_owner(stack).is_some() {
        return;
    }
    let mut fields = binding_record(stack);
    fields.insert(BOUND_TO, owner.hyphenated().to_string());
    write_tag(stack, fields);
}

/// Returns `stack` bound to `owner`, keeping an existing binding.
///
/// Building a personal stack from scratch is a two-step operation everywhere
/// else, so the single-owner rule lives here instead of in each caller.
#[must_use]
pub fn bound(mut stack: ItemStack, owner: Uuid) -> ItemStack {
    bind(&mut stack, owner);
    stack
}

/// Returns the owner a personal item belongs to.
#[must_use]
pub fn bound_owner(stack: &ItemStack) -> Option<Uuid> {
    let owner = tag_string(&read_tag(stack)?, BOUND_TO)?;
    Uuid::try_parse(&owner).ok()
}

/// Returns whether `stack` is personal and belongs to someone else.
#[must_use]
pub fn bound_to_other(stack: &ItemStack, player: Uuid) -> bool {
    bound_owner(stack).is_some_and(|owner| owner != player)
}

/// Returns whether a stack may be deposited into storage outside its owner's inventory.
#[must_use]
pub fn may_store(stack: &ItemStack) -> bool {
    bound_owner(stack).is_none()
}

/// Returns whether `player` may retrieve a stack from external storage.
#[must_use]
pub fn may_retrieve(stack: &ItemStack, player: Uuid) -> bool {
    bound_owner(stack).is_none_or(|owner| owner == player)
}

/// Returns whether a stack may be dropped into the world.
#[must_use]
pub fn may_drop(stack: &ItemStack) -> bool {
    bound_owner(stack).is_none()
}

/// Rebuilds the Solo Leveling record of `stack` after its kind fields changed,
/// so binding it does not drop the fields that describe the item.
fn binding_record(stack: &ItemStack) -> NbtCompound {
    read_tag(stack).unwrap_or_else(|| record(BOUND_KIND))
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};
    use uuid::Uuid;

    use super::{bind, may_drop, may_retrieve, may_store};

    #[test]
    fn a_personal_item_stays_with_its_owner_across_transfer_routes() {
        init_vanilla_registry();
        let owner = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let mut item = ItemStack::new(&vanilla_items::DIAMOND);
        bind(&mut item, owner);

        assert!(!may_store(&item));
        assert!(!may_drop(&item));
        assert!(may_retrieve(&item, owner));
        assert!(!may_retrieve(&item, stranger));
        assert!(may_store(&ItemStack::new(&vanilla_items::DIAMOND)));
    }
}
