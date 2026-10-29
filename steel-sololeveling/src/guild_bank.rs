//! Persistent chest menu for authorized guild-bank operators.

use std::sync::Arc;

use steel_core::inventory::container::{Container as _, SimpleContainer};
use steel_core::inventory::menu::kinds::chest_with_kind;
use steel_core::inventory::menu::{MenuBehavior, MenuKind};
use steel_core::inventory::prelude::Shared;
use steel_core::player::Player;
use steel_utils::{DowncastType, DowncastTypeKey, locks::IntoShared as _};
use text_components::TextComponent;

use crate::guild::GuildError;
use crate::state::SoloLevelingState;

/// Number of inventory slots in every guild bank.
pub const GUILD_BANK_SLOTS: usize = 27;

/// Opens a guild bank after reserving its single editable inventory.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) -> Result<(), GuildError> {
    let player_uuid = player.gameprofile.id();
    let (key, items) = state.open_guild_bank(player_uuid, GUILD_BANK_SLOTS)?;
    let container = SimpleContainer::from_items(items).into_shared();
    let inventory = Arc::clone(&player.inventory);
    player.open_menu(
        TextComponent::plain("Банк гильдии"),
        move |context| {
            chest_with_kind(
                inventory,
                context.container_id,
                Arc::clone(&container),
                GUILD_BANK_SLOTS / 9,
                GuildBankMenu {
                    container,
                    player: player_uuid,
                    guild_key: key,
                    state,
                },
            )
        },
    );
    Ok(())
}

/// Saves the bank only after the menu has released its inventory locks.
struct GuildBankMenu {
    container: Shared<SimpleContainer>,
    player: uuid::Uuid,
    guild_key: String,
    state: Arc<SoloLevelingState>,
}

// SAFETY: The Solo Leveling-owned key uniquely identifies this menu kind.
unsafe impl DowncastType for GuildBankMenu {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:sololeveling/guild_bank_menu");
}

impl MenuKind for GuildBankMenu {
    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        let items = self.container.lock().items().to_vec();
        self.state
            .close_guild_bank(self.player, &self.guild_key, &items);
    }
}
