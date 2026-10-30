//! The System main menu, opened from the System book in the last hotbar slot.
//!
//! The top row shows the hunter's profile; the middle row holds one button per
//! category. Buttons open their own menus where one exists and otherwise write
//! a short summary to chat.

use std::sync::Arc;

use steel_core::inventory::prelude::*;
use steel_registry::items::ItemRef;
use steel_registry::{vanilla_items, vanilla_menu_types};
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use crate::auction_menu;
use crate::class_menu;
use crate::guild_menu;
use crate::hunter_stats_menu;
use crate::item::{named, with_lore};
use crate::organization_menu;
use crate::party_menu;
use crate::pay_menu;
use crate::shadow_menu;
use crate::shop_menu;
use crate::sound::SystemSound;
use crate::state::SoloLevelingState;

const MENU_SLOTS: usize = 27;
const PROFILE_SLOT: usize = 4;

/// One category of the main menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Hunter,
    Shop,
    Contracts,
    Guild,
    Party,
    Shadows,
    Auction,
    Crystals,
    Enchanting,
    Class,
    Pay,
    Gear,
}

/// Categories and the menu slot each one sits in.
const CATEGORIES: [(usize, Category); 12] = [
    (10, Category::Hunter),
    (11, Category::Shop),
    (12, Category::Contracts),
    (13, Category::Guild),
    (14, Category::Party),
    (15, Category::Shadows),
    (16, Category::Auction),
    (22, Category::Crystals),
    (23, Category::Enchanting),
    (21, Category::Class),
    (20, Category::Pay),
    (24, Category::Gear),
];

fn category_at(slot: usize) -> Option<Category> {
    CATEGORIES
        .iter()
        .find_map(|(at, category)| (*at == slot).then_some(*category))
}

impl Category {
    fn icon(self) -> ItemRef {
        match self {
            Self::Hunter => &vanilla_items::NETHER_STAR,
            Self::Shop => &vanilla_items::EMERALD,
            Self::Contracts => &vanilla_items::WRITABLE_BOOK,
            Self::Guild => &vanilla_items::WHITE_BANNER,
            Self::Party => &vanilla_items::BELL,
            Self::Shadows => &vanilla_items::WITHER_SKELETON_SKULL,
            Self::Auction => &vanilla_items::GOLD_INGOT,
            Self::Crystals => &vanilla_items::AMETHYST_SHARD,
            Self::Enchanting => &vanilla_items::ENCHANTING_TABLE,
            Self::Class => &vanilla_items::IRON_SWORD,
            Self::Pay => &vanilla_items::GOLD_NUGGET,
            Self::Gear => &vanilla_items::ARMOR_STAND,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Hunter => "Охотник",
            Self::Shop => "Магазин Системы",
            Self::Contracts => "Мои задания",
            Self::Guild => "Гильдия",
            Self::Party => "Группа",
            Self::Shadows => "Армия теней",
            Self::Auction => "Аукцион",
            Self::Crystals => "Продать кристаллы",
            Self::Enchanting => "Зачарование",
            Self::Class => "Класс",
            Self::Pay => "Перевод Won",
            Self::Gear => "Снаряжение",
        }
    }

    const fn hints(self) -> &'static [&'static str] {
        match self {
            Self::Hunter => &["Распределить очки характеристик."],
            Self::Shop => &["Десять товаров дня за очки Системы."],
            Self::Contracts => &[
                "Взятые задания Организации и прогресс.",
                "Брать и сдавать — у доски Организации.",
            ],
            Self::Guild => &[
                "Основать гильдию, участники и ранги,",
                "банк и взносы в казну.",
            ],
            Self::Party => &["Состав группы, приглашения", "и выход из группы."],
            Self::Shadows => &["Армия, призыв, отзыв", "и прокачка теней."],
            Self::Auction => &["Покупка, ставки и продажа", "предмета из руки."],
            Self::Crystals => &["Продать все магические кристаллы", "Организации за Won."],
            Self::Enchanting => &[
                "Выбрать артефакт и зачаровать его.",
                "Недостатки дают очки, силы стоят Won.",
            ],
            Self::Class => &["Выбрать класс охотника.", "Бонусы классов и правила смены."],
            Self::Pay => &["Перевести Won охотнику онлайн."],
            Self::Gear => &[
                "Шлем, броня, перчатки, кольца, амулет:",
                "бонусы артефактов без надевания.",
            ],
        }
    }

    fn item(self) -> ItemStack {
        with_lore(
            named(
                ItemStack::new(self.icon()),
                TextComponent::plain(self.title()).color(Color::Gold),
            ),
            self.hints()
                .iter()
                .map(|hint| TextComponent::plain(*hint).color(Color::Gray))
                .collect(),
        )
    }
}

/// Opens the main menu for an online hunter.
pub fn open(player: &Player, state: Arc<SoloLevelingState>) {
    SystemSound::Open.play(player);
    player.open_menu(
        TextComponent::plain("Система").color(Color::Aqua),
        move |context| build(context.container_id, context.player, state),
    );
}

fn build(container_id: u8, player: &Player, state: Arc<SoloLevelingState>) -> Menu {
    let items = menu_items(&state, player.gameprofile.id());
    let container = SimpleContainer::from_items(items).into_shared();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let buttons = builder.section_all(Arc::clone(&container));
    builder.player_inventory(&player.inventory);
    builder.build(MainMenuKind { state, buttons })
}

fn menu_items(state: &SoloLevelingState, uuid: Uuid) -> Vec<ItemStack> {
    let mut items = vec![ItemStack::empty(); MENU_SLOTS];
    items[PROFILE_SLOT] = profile_item(state, uuid);
    for (slot, category) in CATEGORIES {
        items[slot] = category.item();
    }
    items
}

fn profile_item(state: &SoloLevelingState, uuid: Uuid) -> ItemStack {
    let lines = state
        .with_session_mut(uuid, |session| {
            vec![
                format!(
                    "Уровень {} · ранг {}",
                    session.stats.level,
                    session.stats.rank().name()
                ),
                format!("Won: ₩{}", session.won.amount()),
                format!("Очки Системы: {}", session.system_points.amount()),
                format!("Очки характеристик: {}", session.stats.unspent_points),
                format!("Теней: {}", session.shadows.shadows.len()),
            ]
        })
        .unwrap_or_default();
    with_lore(
        named(
            ItemStack::new(&vanilla_items::PLAYER_HEAD),
            TextComponent::plain("Профиль охотника").color(Color::Aqua),
        ),
        lines
            .into_iter()
            .map(|line| TextComponent::plain(line).color(Color::White))
            .collect(),
    )
}

struct MainMenuKind {
    state: Arc<SoloLevelingState>,
    buttons: Section,
}

// SAFETY: This Solo Leveling-owned key uniquely identifies the concrete menu kind.
unsafe impl DowncastType for MainMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("sololeveling:menu/main");
}

impl MainMenuKind {
    fn act(&self, category: Category, player: &Player) {
        let state = &self.state;
        let uuid = player.gameprofile.id();
        match category {
            Category::Hunter => hunter_stats_menu::open(player, Arc::clone(state)),
            Category::Shop => shop_menu::open(player, Arc::clone(state)),
            Category::Contracts => organization_menu::open_view(player, Arc::clone(state)),
            Category::Guild => guild_menu::open(player, Arc::clone(state)),
            Category::Party => party_menu::open(player, Arc::clone(state)),
            Category::Shadows => shadow_menu::open(player, Arc::clone(state)),
            Category::Auction => auction_menu::open(player, Arc::clone(state)),
            // The menu callback owns the inventory lock, so request the sale
            // for the next player tick instead of scanning the inventory here.
            Category::Crystals => {
                state.defer_crystal_sale(uuid);
                tell(player, "Продажа кристаллов запрошена.", Color::Gold);
            }
            // The item maker reads the main hand, which this callback has locked.
            Category::Enchanting => state.defer_enchant_menu(uuid),
            Category::Class => class_menu::open(player, Arc::clone(state)),
            Category::Pay => pay_menu::open(player, Arc::clone(state)),
            Category::Gear => state.defer_gear_menu(uuid),
        }
    }
}

fn tell(player: &Player, text: &str, color: Color) {
    player.send_message(&TextComponent::plain(text.to_owned()).color(color));
}

impl MenuKind for MainMenuKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        click: Click,
        player: &Player,
    ) -> ClickOutcome {
        let Click::Pickup { slot, .. } = click else {
            return if click.slot().is_some_and(|slot| self.buttons.contains(slot)) {
                ClickOutcome::Consume
            } else {
                ClickOutcome::Fallthrough
            };
        };
        if !self.buttons.contains(slot) {
            return ClickOutcome::Fallthrough;
        }
        if let Some(category) = category_at(slot - self.buttons.start()) {
            self.act(category, player);
        }
        ClickOutcome::Consume
    }
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES, MENU_SLOTS, PROFILE_SLOT, category_at};

    #[test]
    fn categories_use_distinct_slots_inside_the_menu() {
        for (index, (slot, category)) in CATEGORIES.iter().enumerate() {
            assert!(*slot < MENU_SLOTS);
            assert_ne!(*slot, PROFILE_SLOT);
            assert_eq!(category_at(*slot), Some(*category));
            assert!(
                CATEGORIES[index + 1..]
                    .iter()
                    .all(|(other, _)| other != slot),
                "slot {slot} is used twice"
            );
        }
        assert_eq!(category_at(PROFILE_SLOT), None);
    }
}
