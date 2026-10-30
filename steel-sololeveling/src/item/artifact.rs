//! Artifact catalog, rarity rules, and the item stacks that carry them.
//!
//! Every artifact is a vanilla item plus attribute modifiers and a Solo Leveling
//! tag. Names come from the Solo Leveling wiki; the numbers and abilities are a
//! server adaptation for Minecraft balance, not a claim about canonical values.
//!
//! Rarity decides two things: abilities start at Epic, and a transferable Epic
//! artifact needs hunter rank C while a transferable Legendary one needs rank A.
//! Personal (bound) artifacts have no rank restriction, on both sides of the
//! market.
//!
//! Gloves deliberately use the chestplate slot: the vanilla client has no glove
//! slot, so an artifact glove and a chestplate cannot be worn together. This is
//! a documented adaptation, not a hidden extra equipment slot.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use steel_registry::attribute::AttributeRef;
use steel_registry::data_components::components::{
    ItemAttributeModifierDisplay, ItemAttributeModifierEntry, ItemAttributeModifiers,
};
use steel_registry::data_components::vanilla_components::ATTRIBUTE_MODIFIERS;
use steel_registry::equipment::EquipmentSlotGroup;
use steel_registry::item_stack::ItemStack;

use steel_core::entity::attribute::AttributeModifierOperation;
use steel_registry::items::Item;
use steel_registry::{vanilla_attributes, vanilla_items};
use steel_utils::Identifier;
use text_components::format::Color;
use text_components::{Modifier as _, TextComponent};
use uuid::Uuid;

use super::binding;
use super::enchantment::{self, Effect, Enchantment};
use super::{named, read_kind, record, tag_string, with_lore, write_tag};
use crate::gear::GearKind;
use crate::identifier::from_sololeveling;
use crate::rank::HunterRank;

/// Kind stored in the record of an artifact item.
const KIND: &str = "artifact";
/// Tag field holding the artifact id.
const ID: &str = "id";
/// Tag field holding the comma-separated enchantment list.
const ENCHANTMENTS: &str = "enchantments";

/// Artifact rarity, from the cheapest transferable to the strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    /// Unused by the catalog; kept so transferable and personal rules stay
    /// readable as one ordered scale.
    Common,
    /// Distinct stats, no ability.
    Rare,
    /// Abilities start here.
    Epic,
    /// Strongest abilities and the strictest transferable rank gate.
    Legendary,
}

impl Rarity {
    /// Returns the Russian display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Common => "Обычный",
            Self::Rare => "Редкий",
            Self::Epic => "Эпический",
            Self::Legendary => "Легендарный",
        }
    }

    /// Returns the tooltip color of the rarity.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Common => Color::Gray,
            Self::Rare => Color::Aqua,
            Self::Epic => Color::LightPurple,
            Self::Legendary => Color::Gold,
        }
    }

    /// Returns the stored name of the rarity.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Rare => "rare",
            Self::Epic => "epic",
            Self::Legendary => "legendary",
        }
    }

    /// Parses a stored rarity name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Common, Self::Rare, Self::Epic, Self::Legendary]
            .into_iter()
            .find(|rarity| rarity.name() == name)
    }

    /// Returns the lowest hunter rank that may use a transferable artifact of
    /// this rarity, or `None` when every rank may.
    #[must_use]
    pub const fn transferable_minimum_rank(self) -> Option<HunterRank> {
        match self {
            Self::Common | Self::Rare => None,
            Self::Epic => Some(HunterRank::C),
            Self::Legendary => Some(HunterRank::A),
        }
    }
}

/// One catalog artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ArtifactId {
    /// Rare dagger: flat attack damage.
    KasakasVenomFang,
    /// Epic sword: bonus damage against armored targets.
    KnightsKiller,
    /// Epic dagger: short speed burst after a hit.
    BarukasDagger,
    /// Legendary daggers: damage scaling with Strength.
    DemonKingsDaggers,
    /// Legendary sword: a small storm around the hit target.
    DemonKingsLongsword,
    /// Rare glove: armor in the chestplate slot.
    ArchersGloves,
    /// Rare boots: armor and movement speed.
    AssassinsShoes,
    /// Epic chestplate: damage reduction at low health.
    HighKnightsChestplate,
    /// Epic glove: a shield on every fifth hit.
    HighKnightsGauntlet,
    /// Legendary helmet: survival of a lethal hit.
    RedKnightsHelmet,
    /// Epic pickaxe: extra crystals from dungeon ore.
    CrystalPickaxe,
}

impl ArtifactId {
    /// Returns the stable tag value of the artifact.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::KasakasVenomFang => "kasakas_venom_fang",
            Self::KnightsKiller => "knights_killer",
            Self::BarukasDagger => "barukas_dagger",
            Self::DemonKingsDaggers => "demon_kings_daggers",
            Self::DemonKingsLongsword => "demon_kings_longsword",
            Self::ArchersGloves => "archers_gloves",
            Self::AssassinsShoes => "assassins_shoes",
            Self::HighKnightsChestplate => "high_knights_chestplate",
            Self::HighKnightsGauntlet => "high_knights_gauntlet",
            Self::RedKnightsHelmet => "red_knights_helmet",
            Self::CrystalPickaxe => "crystal_pickaxe",
        }
    }

    /// Parses the stable tag value.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|artifact| artifact.name() == name)
    }

    /// Returns the System equipment slot this artifact goes into, or `None`
    /// for weapons and tools, which are held instead.
    #[must_use]
    pub const fn gear_kind(self) -> Option<GearKind> {
        match self {
            Self::RedKnightsHelmet => Some(GearKind::Head),
            Self::HighKnightsChestplate => Some(GearKind::Chest),
            Self::AssassinsShoes => Some(GearKind::Feet),
            Self::ArchersGloves | Self::HighKnightsGauntlet => Some(GearKind::Gloves),
            Self::KasakasVenomFang
            | Self::KnightsKiller
            | Self::BarukasDagger
            | Self::DemonKingsDaggers
            | Self::DemonKingsLongsword
            | Self::CrystalPickaxe => None,
        }
    }

    /// Returns the catalog entry of this artifact.
    #[must_use]
    pub fn artifact(self) -> &'static Artifact {
        match self {
            Self::KasakasVenomFang => &KASAKAS_VENOM_FANG,
            Self::KnightsKiller => &KNIGHTS_KILLER,
            Self::BarukasDagger => &BARUKAS_DAGGER,
            Self::DemonKingsDaggers => &DEMON_KINGS_DAGGERS,
            Self::DemonKingsLongsword => &DEMON_KINGS_LONGSWORD,
            Self::ArchersGloves => &ARCHERS_GLOVES,
            Self::AssassinsShoes => &ASSASSINS_SHOES,
            Self::HighKnightsChestplate => &HIGH_KNIGHTS_CHESTPLATE,
            Self::HighKnightsGauntlet => &HIGH_KNIGHTS_GAUNTLET,
            Self::RedKnightsHelmet => &RED_KNIGHTS_HELMET,
            Self::CrystalPickaxe => &CRYSTAL_PICKAXE,
        }
    }
}

/// Every catalog artifact.
pub const ALL: [ArtifactId; 11] = [
    ArtifactId::KasakasVenomFang,
    ArtifactId::KnightsKiller,
    ArtifactId::BarukasDagger,
    ArtifactId::DemonKingsDaggers,
    ArtifactId::DemonKingsLongsword,
    ArtifactId::ArchersGloves,
    ArtifactId::AssassinsShoes,
    ArtifactId::HighKnightsChestplate,
    ArtifactId::HighKnightsGauntlet,
    ArtifactId::RedKnightsHelmet,
    ArtifactId::CrystalPickaxe,
];

/// Which catalog list an artifact belongs to, for chest and shop rolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Held in the main hand.
    Weapon,
    /// Worn in an armor slot.
    Armor,
}

/// One attribute bonus an artifact grants while worn.
#[derive(Debug, Clone, Copy)]
pub struct Modifier {
    /// Vanilla attribute the bonus applies to.
    pub attribute: AttributeRef,
    /// Bonus amount, interpreted by `operation`.
    pub amount: f64,
    /// How `amount` combines with the attribute's other modifiers.
    pub operation: AttributeModifierOperation,
    /// Equipment slots the bonus is active in.
    pub slot: EquipmentSlotGroup,
}

/// One artifact ability. Abilities exist only from [`Rarity::Epic`] upward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ability {
    /// Extra damage against targets wearing any armor.
    ArmorPiercing {
        /// Damage bonus as a fraction of the hit.
        percent: f32,
    },
    /// A short movement-speed burst after a successful hit.
    SpeedOnHit {
        /// Speed bonus as a fraction of the base attribute.
        percent: f64,
        /// Duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Flat attack damage that grows with the holder's Strength.
    StrengthScaling {
        /// Damage per Strength point.
        per_point: f64,
        /// Highest total bonus.
        maximum: f64,
    },
    /// Reduced incoming damage while the holder is badly hurt.
    LowHealthReduction {
        /// Health fraction below which the ability is active.
        threshold: f32,
        /// Fraction of incoming damage removed.
        reduction: f32,
        /// Duration in seconds, refreshed on each qualifying hit.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// A shield after every Nth successful hit.
    HitCounterShield {
        /// Successful hits needed for one shield.
        hits: u8,
        /// Absorption health granted, in half-hearts.
        shield_health: f32,
        /// Duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// A short storm around the struck target.
    Storm {
        /// Radius in blocks.
        radius: f64,
        /// Duration in seconds.
        seconds: u64,
        /// Damage per second to every other hostile entity inside.
        damage_per_second: f32,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Survives one otherwise lethal hit.
    LethalSurvival {
        /// Health left after the survived hit.
        health: f32,
        /// Shield granted together with the survival.
        shield_health: f32,
        /// Shield duration in seconds.
        seconds: u64,
        /// Cooldown in seconds.
        cooldown: u64,
    },
    /// Extra magic crystals from dungeon ore.
    CrystalLuck {
        /// Percent chance of one additional crystal.
        percent: u32,
    },
}

impl Ability {
    /// Returns the persisted cooldown key of the ability.
    #[must_use]
    pub const fn cooldown_key(self) -> &'static str {
        match self {
            Self::ArmorPiercing { .. } => "armor_piercing",
            Self::SpeedOnHit { .. } => "speed_on_hit",
            Self::StrengthScaling { .. } => "strength_scaling",
            Self::LowHealthReduction { .. } => "low_health_reduction",
            Self::HitCounterShield { .. } => "hit_counter_shield",
            Self::Storm { .. } => "storm",
            Self::LethalSurvival { .. } => "lethal_survival",
            Self::CrystalLuck { .. } => "crystal_luck",
        }
    }

    /// Returns the cooldown in seconds, or zero when the ability has none.
    #[must_use]
    pub const fn cooldown_seconds(self) -> u64 {
        match self {
            Self::SpeedOnHit { cooldown, .. }
            | Self::LowHealthReduction { cooldown, .. }
            | Self::HitCounterShield { cooldown, .. }
            | Self::Storm { cooldown, .. }
            | Self::LethalSurvival { cooldown, .. } => cooldown,
            Self::ArmorPiercing { .. }
            | Self::StrengthScaling { .. }
            | Self::CrystalLuck { .. } => 0,
        }
    }
}

/// One catalog entry.
#[derive(Debug)]
pub struct Artifact {
    /// Catalog identity.
    pub id: ArtifactId,
    /// Russian display name.
    pub name: &'static str,
    /// Rarity, which also decides the transferable rank gate.
    pub rarity: Rarity,
    /// Which roll list the artifact belongs to.
    pub category: Category,
    /// Vanilla item the artifact is built from.
    pub holder: &'static LazyLock<Item>,
    /// Attribute bonuses granted while worn.
    pub modifiers: &'static [Modifier],
    /// Abilities; empty below Epic.
    pub abilities: &'static [Ability],
}

const ATTACK: EquipmentSlotGroup = EquipmentSlotGroup::MainHand;
const HELMET: EquipmentSlotGroup = EquipmentSlotGroup::Head;
const CHEST: EquipmentSlotGroup = EquipmentSlotGroup::Chest;
const FEET: EquipmentSlotGroup = EquipmentSlotGroup::Feet;

static KASAKAS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static KNIGHT_KILLER_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 4.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static BARUKA_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ATTACK_DAMAGE,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: ATTACK,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.08,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: EquipmentSlotGroup::Hand,
    },
];
static DEMON_DAGGERS_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 6.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static DEMON_LONGSWORD_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ATTACK_DAMAGE,
    amount: 7.0,
    operation: AttributeModifierOperation::AddValue,
    slot: ATTACK,
}];
static ARCHER_GLOVES_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 1.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static ASSASSIN_SHOES_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 1.0,
        operation: AttributeModifierOperation::AddValue,
        slot: FEET,
    },
    Modifier {
        attribute: vanilla_attributes::MOVEMENT_SPEED,
        amount: 0.05,
        operation: AttributeModifierOperation::AddMultipliedBase,
        slot: FEET,
    },
];
static HIGH_KNIGHT_CHEST_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 3.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static HIGH_KNIGHT_GAUNTLET_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::ARMOR,
    amount: 2.0,
    operation: AttributeModifierOperation::AddValue,
    slot: CHEST,
}];
static RED_KNIGHT_HELMET_MODIFIERS: [Modifier; 2] = [
    Modifier {
        attribute: vanilla_attributes::ARMOR,
        amount: 3.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
    Modifier {
        attribute: vanilla_attributes::MAX_HEALTH,
        amount: 4.0,
        operation: AttributeModifierOperation::AddValue,
        slot: HELMET,
    },
];
static CRYSTAL_PICKAXE_MODIFIERS: [Modifier; 1] = [Modifier {
    attribute: vanilla_attributes::MINING_EFFICIENCY,
    amount: 0.15,
    operation: AttributeModifierOperation::AddMultipliedBase,
    slot: EquipmentSlotGroup::Hand,
}];

static KASAKAS_VENOM_FANG: Artifact = Artifact {
    id: ArtifactId::KasakasVenomFang,
    name: "Клык Касаки",
    rarity: Rarity::Rare,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &KASAKAS_MODIFIERS,
    abilities: &[],
};
static KNIGHTS_KILLER: Artifact = Artifact {
    id: ArtifactId::KnightsKiller,
    name: "Убийца рыцарей",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &KNIGHT_KILLER_MODIFIERS,
    abilities: &[Ability::ArmorPiercing { percent: 0.15 }],
};
static BARUKAS_DAGGER: Artifact = Artifact {
    id: ArtifactId::BarukasDagger,
    name: "Кинжал Баруки",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::GOLDEN_SWORD,
    modifiers: &BARUKA_MODIFIERS,
    abilities: &[Ability::SpeedOnHit {
        percent: 0.15,
        seconds: 3,
        cooldown: 10,
    }],
};
static DEMON_KINGS_DAGGERS: Artifact = Artifact {
    id: ArtifactId::DemonKingsDaggers,
    name: "Кинжалы короля демонов",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::DIAMOND_SWORD,
    modifiers: &DEMON_DAGGERS_MODIFIERS,
    abilities: &[Ability::StrengthScaling {
        per_point: 0.05,
        maximum: 5.0,
    }],
};
static DEMON_KINGS_LONGSWORD: Artifact = Artifact {
    id: ArtifactId::DemonKingsLongsword,
    name: "Длинный меч короля демонов",
    rarity: Rarity::Legendary,
    category: Category::Weapon,
    holder: &vanilla_items::NETHERITE_SWORD,
    modifiers: &DEMON_LONGSWORD_MODIFIERS,
    abilities: &[Ability::Storm {
        radius: 3.0,
        seconds: 3,
        damage_per_second: 2.0,
        cooldown: 20,
    }],
};
static ARCHERS_GLOVES: Artifact = Artifact {
    id: ArtifactId::ArchersGloves,
    name: "Перчатки лучника",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ARCHER_GLOVES_MODIFIERS,
    abilities: &[],
};
static ASSASSINS_SHOES: Artifact = Artifact {
    id: ArtifactId::AssassinsShoes,
    name: "Обувь убийцы",
    rarity: Rarity::Rare,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ASSASSIN_SHOES_MODIFIERS,
    abilities: &[],
};
static HIGH_KNIGHTS_CHESTPLATE: Artifact = Artifact {
    id: ArtifactId::HighKnightsChestplate,
    name: "Нагрудник высшего рыцаря",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &HIGH_KNIGHT_CHEST_MODIFIERS,
    abilities: &[Ability::LowHealthReduction {
        threshold: 0.3,
        reduction: 0.15,
        seconds: 4,
        cooldown: 30,
    }],
};
static HIGH_KNIGHTS_GAUNTLET: Artifact = Artifact {
    id: ArtifactId::HighKnightsGauntlet,
    name: "Перчатки высшего рыцаря",
    rarity: Rarity::Epic,
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &HIGH_KNIGHT_GAUNTLET_MODIFIERS,
    abilities: &[Ability::HitCounterShield {
        hits: 5,
        shield_health: 4.0,
        seconds: 5,
        cooldown: 15,
    }],
};
static RED_KNIGHTS_HELMET: Artifact = Artifact {
    id: ArtifactId::RedKnightsHelmet,
    name: "Шлем красного рыцаря",
    rarity: Rarity::Legendary,
    category: Category::Armor,
    holder: &vanilla_items::NETHERITE_HELMET,
    modifiers: &RED_KNIGHT_HELMET_MODIFIERS,
    abilities: &[Ability::LethalSurvival {
        health: 1.0,
        shield_health: 6.0,
        seconds: 3,
        cooldown: 120,
    }],
};
static CRYSTAL_PICKAXE: Artifact = Artifact {
    id: ArtifactId::CrystalPickaxe,
    name: "Кирка магических кристаллов",
    rarity: Rarity::Epic,
    category: Category::Weapon,
    holder: &vanilla_items::NETHERITE_PICKAXE,
    modifiers: &CRYSTAL_PICKAXE_MODIFIERS,
    abilities: &[Ability::CrystalLuck { percent: 50 }],
};

/// Returns every weapon artifact of `rarity`.
#[must_use]
pub fn weapons_of(rarity: Rarity) -> Vec<ArtifactId> {
    ALL.into_iter()
        .filter(|id| {
            let artifact = id.artifact();
            artifact.category == Category::Weapon && artifact.rarity == rarity
        })
        .collect()
}

/// Returns every armor artifact of `rarity`.
#[must_use]
pub fn armors_of(rarity: Rarity) -> Vec<ArtifactId> {
    ALL.into_iter()
        .filter(|id| {
            let artifact = id.artifact();
            artifact.category == Category::Armor && artifact.rarity == rarity
        })
        .collect()
}

/// Builds the item stack of one artifact.
///
/// A personal stack carries its owner's binding; `owner` is `None` for a
/// transferable instance.
#[must_use]
pub fn stack(id: ArtifactId, owner: Option<Uuid>) -> ItemStack {
    enchanted_stack(id, owner, &[])
}

/// Builds the item stack of one artifact carrying `enchantments`.
///
/// Enchanting rebuilds the stack from the catalog, so the binding is passed
/// back in and survives.
#[must_use]
pub fn enchanted_stack(
    id: ArtifactId,
    owner: Option<Uuid>,
    enchantments: &[Enchantment],
) -> ItemStack {
    let artifact = id.artifact();
    let lines = artifact
        .modifiers
        .iter()
        .map(|modifier| {
            TextComponent::plain(format!(
                "{} {}",
                describe_attribute(modifier.attribute),
                describe_amount(modifier)
            ))
            .color(Color::Gray)
        })
        .chain(artifact.abilities.iter().map(|ability| {
            TextComponent::plain(describe_ability(*ability)).color(Color::LightPurple)
        }))
        .chain(enchantments.iter().map(|enchantment| {
            TextComponent::plain(format!(
                "{}: {}",
                enchantment.display_name(),
                enchantment.describe()
            ))
            .color(if enchantment.is_side_effect() {
                Color::Red
            } else {
                Color::Blue
            })
        }))
        .chain([
            TextComponent::plain(format!("Редкость: {}", artifact.rarity.display_name()))
                .color(artifact.rarity.color()),
        ])
        .chain(
            owner
                .is_none()
                .then(|| artifact.rarity.transferable_minimum_rank())
                .flatten()
                .map(|rank| {
                    TextComponent::plain(format!("Без привязки работает с ранга {}", rank.name()))
                        .color(Color::Yellow)
                }),
        )
        .chain((!enchantments.is_empty()).then(|| {
            TextComponent::plain(format!(
                "Очки зачарования: {} / {}",
                enchantment::points_used(enchantments),
                enchantment::budget(artifact.rarity)
            ))
            .color(Color::DarkAqua)
        }))
        .collect::<Vec<_>>();

    let mut stack = named(
        ItemStack::new(artifact.holder),
        TextComponent::plain(artifact.name.to_owned()).color(artifact.rarity.color()),
    );
    stack = with_lore(stack, lines);
    let modifiers = modifiers_component(artifact, enchantments);
    if !modifiers.modifiers.is_empty() {
        stack.set(ATTRIBUTE_MODIFIERS, modifiers);
    }
    super::model::apply(&mut stack, &format!("artifact/{}", id.name()));
    if artifact.category == Category::Armor {
        super::model::apply_equipment(&mut stack, id.name());
    }
    let mut fields = record(KIND);
    fields.insert(ID, id.name().to_owned());
    fields.insert("rarity", artifact.rarity.name().to_owned());
    if !enchantments.is_empty() {
        fields.insert(ENCHANTMENTS, enchantment::encode(enchantments));
    }
    write_tag(&mut stack, fields);
    if let Some(owner) = owner {
        binding::bind(&mut stack, owner);
    }
    stack
}

/// Returns the artifact a stack holds, or `None` for an ordinary item.
#[must_use]
pub fn of(stack: &ItemStack) -> Option<ArtifactId> {
    let tag = read_kind(stack, KIND)?;
    ArtifactId::from_name(&tag_string(&tag, ID)?)
}

/// Returns the enchantments laid on an artifact stack.
#[must_use]
pub fn enchantments_of(stack: &ItemStack) -> Vec<Enchantment> {
    read_kind(stack, KIND)
        .and_then(|tag| tag_string(&tag, ENCHANTMENTS))
        .map_or_else(Vec::new, |stored| enchantment::decode(&stored))
}

/// Returns the rarity of an artifact stack, falling back to the catalog.
#[must_use]
pub fn rarity_of(stack: &ItemStack) -> Option<Rarity> {
    let id = of(stack)?;
    let tag = read_kind(stack, KIND)?;
    tag_string(&tag, "rarity")
        .and_then(|name| Rarity::from_name(&name))
        .or(Some(id.artifact().rarity))
}

/// Returns the rank a hunter needs to use `stack`, or `None` when anyone can.
#[must_use]
pub fn required_rank(stack: &ItemStack) -> Option<HunterRank> {
    let id = of(stack)?;
    if binding::bound_owner(stack).is_some() {
        return None;
    }
    id.artifact().rarity.transferable_minimum_rank()
}

/// Returns whether a hunter of `rank` may use `stack`.
///
/// The check runs on use and on equip, not only in a shop menu, so a
/// transferable artifact cannot be handed to a lower rank after purchase.
#[must_use]
pub fn usable_by(stack: &ItemStack, rank: HunterRank) -> bool {
    let Some(id) = of(stack) else {
        return true;
    };
    let artifact = id.artifact();
    if binding::bound_owner(stack).is_some() {
        return true;
    }
    artifact
        .rarity
        .transferable_minimum_rank()
        .is_none_or(|minimum| rank >= minimum)
}

/// Returns the artifact a player holds in a given hand, if any.
#[must_use]
pub fn held(stack: &ItemStack) -> Option<&'static Artifact> {
    of(stack).map(ArtifactId::artifact)
}

/// Returns whether `ability` is available on the artifact a stack holds.
#[must_use]
pub fn has_ability(stack: &ItemStack, want: &Ability) -> bool {
    of(stack).is_some_and(|id| {
        id.artifact()
            .abilities
            .iter()
            .any(|ability| ability.cooldown_key() == want.cooldown_key())
    })
}

/// Returns the first ability of `stack` with the given cooldown key.
#[must_use]
pub fn ability_of(stack: &ItemStack, key: &str) -> Option<Ability> {
    let id = of(stack)?;
    id.artifact()
        .abilities
        .iter()
        .copied()
        .find(|ability| ability.cooldown_key() == key)
}

fn modifiers_component(
    artifact: &'static Artifact,
    enchantments: &[Enchantment],
) -> ItemAttributeModifiers {
    let slot = enchantment_slot(artifact);
    let enchanted = enchantments.iter().filter_map(|enchantment| {
        let Effect::Attribute {
            attribute,
            amount,
            operation,
        } = enchantment.effect()
        else {
            return None;
        };
        Some(ItemAttributeModifierEntry {
            attribute,
            id: from_sololeveling(format!(
                "artifact/{}/enchantment/{}",
                artifact.id.name(),
                enchantment.name()
            )),
            amount,
            operation,
            slot,
            display: ItemAttributeModifierDisplay::Default,
        })
    });
    ItemAttributeModifiers {
        modifiers: artifact
            .modifiers
            .iter()
            .map(|modifier| ItemAttributeModifierEntry {
                attribute: modifier.attribute,
                id: attribute_modifier_id(artifact.id, modifier.attribute),
                amount: modifier.amount,
                operation: modifier.operation,
                slot: modifier.slot,
                display: ItemAttributeModifierDisplay::Default,
            })
            .chain(enchanted)
            .collect(),
    }
}

/// Weapons enchant the main hand; armor enchants the slot it is worn in.
fn enchantment_slot(artifact: &Artifact) -> EquipmentSlotGroup {
    match artifact.category {
        Category::Weapon => ATTACK,
        Category::Armor => artifact
            .modifiers
            .iter()
            .map(|modifier| modifier.slot)
            .next()
            .unwrap_or(EquipmentSlotGroup::Armor),
    }
}

fn attribute_modifier_id(artifact: ArtifactId, attribute: AttributeRef) -> Identifier {
    from_sololeveling(format!(
        "artifact/{}/{}",
        artifact.name(),
        attribute.key.path
    ))
}

fn describe_attribute(attribute: AttributeRef) -> &'static str {
    match attribute.key.path.as_ref() {
        "attack_damage" => "Урон атаки",
        "armor" => "Броня",
        "movement_speed" => "Скорость",
        "max_health" => "Максимальное здоровье",
        "mining_efficiency" => "Скорость добычи",
        _ => "Характеристика",
    }
}

fn describe_amount(modifier: &Modifier) -> String {
    match modifier.operation {
        AttributeModifierOperation::AddMultipliedBase
        | AttributeModifierOperation::AddMultipliedTotal => {
            format!("{:+.0}%", modifier.amount * 100.0)
        }
        AttributeModifierOperation::AddValue => format!("{:+.1}", modifier.amount),
    }
}

/// Returns a one-line Russian summary of an ability for tooltips and messages.
#[must_use]
pub fn describe_ability(ability: Ability) -> String {
    match ability {
        Ability::ArmorPiercing { percent } => {
            format!("+{:.0}% урона по цели в броне", percent * 100.0)
        }
        Ability::SpeedOnHit {
            percent,
            seconds,
            cooldown,
        } => format!(
            "Попадание: +{:.0}% скорости на {seconds} с (перезарядка {cooldown} с)",
            percent * 100.0
        ),
        Ability::StrengthScaling { per_point, maximum } => {
            format!("+{per_point:.2} урона за очко силы, максимум +{maximum:.1}")
        }
        Ability::LowHealthReduction {
            threshold,
            reduction,
            seconds,
            cooldown,
        } => format!(
            "Ниже {:.0}% здоровья: -{:.0}% входящего урона {seconds} с (перезарядка {cooldown} с)",
            threshold * 100.0,
            reduction * 100.0
        ),
        Ability::HitCounterShield {
            hits,
            shield_health,
            seconds,
            cooldown,
        } => format!(
            "Каждое {hits}-е попадание: щит {shield_health:.0} HP на {seconds} с (перезарядка {cooldown} с)"
        ),
        Ability::Storm {
            radius,
            seconds,
            damage_per_second,
            cooldown,
        } => format!(
            "Попадание: буря радиусом {radius:.0} блока на {seconds} с, {damage_per_second:.0} урона в секунду (перезарядка {cooldown} с)"
        ),
        Ability::LethalSurvival {
            health,
            shield_health,
            seconds,
            cooldown,
        } => format!(
            "Смертельный удар оставляет {health:.0} HP и даёт щит {shield_health:.0} HP на {seconds} с (перезарядка {cooldown} с)"
        ),
        Ability::CrystalLuck { percent } => {
            format!("+{percent}% шанс дополнительного кристалла")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::data_components::vanilla_components::LORE;

    #[test]
    fn only_epic_and_above_artifacts_have_abilities() {
        for id in ALL {
            let artifact = id.artifact();
            if artifact.rarity == Rarity::Rare {
                assert!(artifact.abilities.is_empty(), "{:?}", artifact.id);
            } else {
                assert!(!artifact.abilities.is_empty(), "{:?}", artifact.id);
            }
        }
    }

    #[test]
    fn transferable_rank_gates_start_at_c_and_a() {
        assert_eq!(Rarity::Rare.transferable_minimum_rank(), None);
        assert_eq!(
            Rarity::Epic.transferable_minimum_rank(),
            Some(HunterRank::C)
        );
        assert_eq!(
            Rarity::Legendary.transferable_minimum_rank(),
            Some(HunterRank::A)
        );
    }

    #[test]
    fn a_personal_artifact_ignores_the_transferable_rank_gate() {
        let owner = Uuid::new_v4();
        let personal = stack(ArtifactId::RedKnightsHelmet, Some(owner));
        let transferable = stack(ArtifactId::RedKnightsHelmet, None);
        assert!(usable_by(&personal, HunterRank::E));
        assert!(!usable_by(&transferable, HunterRank::E));
        assert!(usable_by(&transferable, HunterRank::A));
        assert_eq!(required_rank(&transferable), Some(HunterRank::A));
        assert_eq!(required_rank(&personal), None);
        let lore = format!("{:?}", transferable.get(LORE));
        assert!(lore.contains("Без привязки работает с ранга A"), "{lore}");
        let lore = format!("{:?}", personal.get(LORE));
        assert!(!lore.contains("Без привязки"), "{lore}");
    }

    #[test]
    fn enchanting_keeps_the_binding_and_adds_its_modifiers() {
        let owner = Uuid::new_v4();
        let applied = [Enchantment::Protection, Enchantment::Frail];
        let stack = enchanted_stack(ArtifactId::AssassinsShoes, Some(owner), &applied);
        assert_eq!(of(&stack), Some(ArtifactId::AssassinsShoes));
        assert_eq!(binding::bound_owner(&stack), Some(owner));
        assert_eq!(enchantments_of(&stack), applied);
        let modifiers = stack.get(ATTRIBUTE_MODIFIERS).expect("modifiers");
        assert_eq!(
            modifiers.modifiers.len(),
            ASSASSIN_SHOES_MODIFIERS.len() + applied.len()
        );
        assert!(
            modifiers
                .modifiers
                .iter()
                .all(|modifier| modifier.slot == FEET)
        );
    }

    #[test]
    fn stacks_round_trip_through_their_tag() {
        for id in ALL {
            let stack = stack(id, None);
            assert_eq!(of(&stack), Some(id));
            assert_eq!(rarity_of(&stack), Some(id.artifact().rarity));
        }
    }
}
