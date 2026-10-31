//! Elite and Supreme serial gear definitions.
use super::{
    ArmorLine, CHEST, Category, FEET, GearKind, HELMET, LEGS, Modifier, Serial, SerialId,
    armor_modifier, improved_heavy_modifiers, improved_reinforced_modifiers, light_modifiers,
    weapon_modifiers,
};
use steel_registry::vanilla_items;

// Elite armor modifiers
pub(crate) static ELITE_LIGHT_HELMET_MODIFIERS: [Modifier; 2] =
    light_modifiers(2.8, 0.0315, HELMET);
pub(crate) static ELITE_LIGHT_CHESTPLATE_MODIFIERS: [Modifier; 2] =
    light_modifiers(7.0, 0.042, CHEST);
pub(crate) static ELITE_LIGHT_LEGGINGS_MODIFIERS: [Modifier; 2] = light_modifiers(5.6, 0.042, LEGS);
pub(crate) static ELITE_LIGHT_BOOTS_MODIFIERS: [Modifier; 2] = light_modifiers(1.4, 0.0315, FEET);
pub(crate) static ELITE_REINFORCED_HELMET_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(4.2, HELMET);
pub(crate) static ELITE_REINFORCED_CHESTPLATE_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(9.8, CHEST);
pub(crate) static ELITE_REINFORCED_LEGGINGS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(8.4, LEGS);
pub(crate) static ELITE_REINFORCED_BOOTS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(2.8, FEET);
pub(crate) static ELITE_HEAVY_HELMET_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(4.2, HELMET);
pub(crate) static ELITE_HEAVY_CHESTPLATE_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(11.2, CHEST);
pub(crate) static ELITE_HEAVY_LEGGINGS_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(8.4, LEGS);
pub(crate) static ELITE_HEAVY_BOOTS_MODIFIERS: [Modifier; 3] = improved_heavy_modifiers(4.2, FEET);
pub(crate) static ELITE_ROBE_HELMET_MODIFIERS: [Modifier; 1] = armor_modifier(1.4, HELMET);
pub(crate) static ELITE_ROBE_CHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(5.6, CHEST);
pub(crate) static ELITE_ROBE_LEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(4.2, LEGS);
pub(crate) static ELITE_ROBE_BOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(1.4, FEET);

// Elite weapon modifiers
pub(crate) static ELITE_HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.4, 1.68);
pub(crate) static ELITE_HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(5.6, 2.52);
pub(crate) static ELITE_HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(12.6, 0.945);
pub(crate) static ELITE_HUNTER_BOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.4, 1.68);
pub(crate) static ELITE_HUNTER_CROSSBOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(12.6, 0.945);
// Supreme armor modifiers
pub(crate) static SUPREME_LIGHT_HELMET_MODIFIERS: [Modifier; 2] =
    light_modifiers(4.0, 0.033, HELMET);
pub(crate) static SUPREME_LIGHT_CHESTPLATE_MODIFIERS: [Modifier; 2] =
    light_modifiers(10.0, 0.044, CHEST);
pub(crate) static SUPREME_LIGHT_LEGGINGS_MODIFIERS: [Modifier; 2] =
    light_modifiers(8.0, 0.044, LEGS);
pub(crate) static SUPREME_LIGHT_BOOTS_MODIFIERS: [Modifier; 2] = light_modifiers(2.0, 0.033, FEET);
pub(crate) static SUPREME_REINFORCED_HELMET_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(6.0, HELMET);
pub(crate) static SUPREME_REINFORCED_CHESTPLATE_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(14.0, CHEST);
pub(crate) static SUPREME_REINFORCED_LEGGINGS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(12.0, LEGS);
pub(crate) static SUPREME_REINFORCED_BOOTS_MODIFIERS: [Modifier; 2] =
    improved_reinforced_modifiers(4.0, FEET);
pub(crate) static SUPREME_HEAVY_HELMET_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(6.0, HELMET);
pub(crate) static SUPREME_HEAVY_CHESTPLATE_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(16.0, CHEST);
pub(crate) static SUPREME_HEAVY_LEGGINGS_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(12.0, LEGS);
pub(crate) static SUPREME_HEAVY_BOOTS_MODIFIERS: [Modifier; 3] =
    improved_heavy_modifiers(6.0, FEET);
pub(crate) static SUPREME_ROBE_HELMET_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, HELMET);
pub(crate) static SUPREME_ROBE_CHESTPLATE_MODIFIERS: [Modifier; 1] = armor_modifier(8.0, CHEST);
pub(crate) static SUPREME_ROBE_LEGGINGS_MODIFIERS: [Modifier; 1] = armor_modifier(6.0, LEGS);
pub(crate) static SUPREME_ROBE_BOOTS_MODIFIERS: [Modifier; 1] = armor_modifier(2.0, FEET);

// Supreme weapon modifiers
pub(crate) static SUPREME_HUNTER_SWORD_MODIFIERS: [Modifier; 2] = weapon_modifiers(12.0, 1.76);
pub(crate) static SUPREME_HUNTER_DAGGER_MODIFIERS: [Modifier; 2] = weapon_modifiers(8.0, 2.64);
pub(crate) static SUPREME_HUNTER_AXE_MODIFIERS: [Modifier; 2] = weapon_modifiers(18.0, 0.99);
pub(crate) static SUPREME_HUNTER_BOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(12.0, 1.76);
pub(crate) static SUPREME_HUNTER_CROSSBOW_MODIFIERS: [Modifier; 2] = weapon_modifiers(18.0, 0.99);

// Serial statics

// Elite armor serials
pub(crate) static ELITE_LIGHT_HELMET: Serial = Serial {
    id: SerialId::EliteLightHelmet,
    name: "Элитный шлем лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &ELITE_LIGHT_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static ELITE_LIGHT_CHESTPLATE: Serial = Serial {
    id: SerialId::EliteLightChestplate,
    name: "Элитная куртка лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ELITE_LIGHT_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static ELITE_LIGHT_LEGGINGS: Serial = Serial {
    id: SerialId::EliteLightLeggings,
    name: "Элитные поножи лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &ELITE_LIGHT_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static ELITE_LIGHT_BOOTS: Serial = Serial {
    id: SerialId::EliteLightBoots,
    name: "Элитные сапоги лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ELITE_LIGHT_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static ELITE_REINFORCED_HELMET: Serial = Serial {
    id: SerialId::EliteReinforcedHelmet,
    name: "Элитный шлем усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &ELITE_REINFORCED_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static ELITE_REINFORCED_CHESTPLATE: Serial = Serial {
    id: SerialId::EliteReinforcedChestplate,
    name: "Элитный нагрудник усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &ELITE_REINFORCED_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static ELITE_REINFORCED_LEGGINGS: Serial = Serial {
    id: SerialId::EliteReinforcedLeggings,
    name: "Элитные поножи усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &ELITE_REINFORCED_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static ELITE_REINFORCED_BOOTS: Serial = Serial {
    id: SerialId::EliteReinforcedBoots,
    name: "Элитные сапоги усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &ELITE_REINFORCED_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static ELITE_HEAVY_HELMET: Serial = Serial {
    id: SerialId::EliteHeavyHelmet,
    name: "Элитный шлем тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &ELITE_HEAVY_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static ELITE_HEAVY_CHESTPLATE: Serial = Serial {
    id: SerialId::EliteHeavyChestplate,
    name: "Элитный нагрудник тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &ELITE_HEAVY_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static ELITE_HEAVY_LEGGINGS: Serial = Serial {
    id: SerialId::EliteHeavyLeggings,
    name: "Элитные поножи тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &ELITE_HEAVY_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static ELITE_HEAVY_BOOTS: Serial = Serial {
    id: SerialId::EliteHeavyBoots,
    name: "Элитные сабатоны тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &ELITE_HEAVY_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static ELITE_ROBE_HELMET: Serial = Serial {
    id: SerialId::EliteRobeHelmet,
    name: "Элитный капюшон роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &ELITE_ROBE_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static ELITE_ROBE_CHESTPLATE: Serial = Serial {
    id: SerialId::EliteRobeChestplate,
    name: "Элитная мантия роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &ELITE_ROBE_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static ELITE_ROBE_LEGGINGS: Serial = Serial {
    id: SerialId::EliteRobeLeggings,
    name: "Элитные брюки роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &ELITE_ROBE_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static ELITE_ROBE_BOOTS: Serial = Serial {
    id: SerialId::EliteRobeBoots,
    name: "Элитные туфли роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &ELITE_ROBE_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};

// Elite weapon serials
pub(crate) static ELITE_HUNTER_SWORD: Serial = Serial {
    id: SerialId::EliteHunterSword,
    name: "Элитный меч охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &ELITE_HUNTER_SWORD_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static ELITE_HUNTER_DAGGER: Serial = Serial {
    id: SerialId::EliteHunterDagger,
    name: "Элитный кинжал охотника",
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &ELITE_HUNTER_DAGGER_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static ELITE_HUNTER_AXE: Serial = Serial {
    id: SerialId::EliteHunterAxe,
    name: "Элитный топор охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &ELITE_HUNTER_AXE_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static ELITE_HUNTER_BOW: Serial = Serial {
    id: SerialId::EliteHunterBow,
    name: "Элитный лук охотника",
    category: Category::Weapon,
    holder: &vanilla_items::BOW,
    modifiers: &ELITE_HUNTER_BOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static ELITE_HUNTER_CROSSBOW: Serial = Serial {
    id: SerialId::EliteHunterCrossbow,
    name: "Элитный арбалет охотника",
    category: Category::Weapon,
    holder: &vanilla_items::CROSSBOW,
    modifiers: &ELITE_HUNTER_CROSSBOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};

// Supreme armor serials
pub(crate) static SUPREME_LIGHT_HELMET: Serial = Serial {
    id: SerialId::SupremeLightHelmet,
    name: "Высший шлем лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &SUPREME_LIGHT_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static SUPREME_LIGHT_CHESTPLATE: Serial = Serial {
    id: SerialId::SupremeLightChestplate,
    name: "Высшая куртка лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &SUPREME_LIGHT_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static SUPREME_LIGHT_LEGGINGS: Serial = Serial {
    id: SerialId::SupremeLightLeggings,
    name: "Высшие поножи лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &SUPREME_LIGHT_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static SUPREME_LIGHT_BOOTS: Serial = Serial {
    id: SerialId::SupremeLightBoots,
    name: "Высшие сапоги лёгкой брони",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &SUPREME_LIGHT_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Light),
    spell_gear: None,
};
pub(crate) static SUPREME_REINFORCED_HELMET: Serial = Serial {
    id: SerialId::SupremeReinforcedHelmet,
    name: "Высший шлем усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_HELMET,
    modifiers: &SUPREME_REINFORCED_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static SUPREME_REINFORCED_CHESTPLATE: Serial = Serial {
    id: SerialId::SupremeReinforcedChestplate,
    name: "Высший нагрудник усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_CHESTPLATE,
    modifiers: &SUPREME_REINFORCED_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static SUPREME_REINFORCED_LEGGINGS: Serial = Serial {
    id: SerialId::SupremeReinforcedLeggings,
    name: "Высшие поножи усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_LEGGINGS,
    modifiers: &SUPREME_REINFORCED_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static SUPREME_REINFORCED_BOOTS: Serial = Serial {
    id: SerialId::SupremeReinforcedBoots,
    name: "Высшие сапоги усиленной брони",
    category: Category::Armor,
    holder: &vanilla_items::IRON_BOOTS,
    modifiers: &SUPREME_REINFORCED_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Reinforced),
    spell_gear: None,
};
pub(crate) static SUPREME_HEAVY_HELMET: Serial = Serial {
    id: SerialId::SupremeHeavyHelmet,
    name: "Высший шлем тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_HELMET,
    modifiers: &SUPREME_HEAVY_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static SUPREME_HEAVY_CHESTPLATE: Serial = Serial {
    id: SerialId::SupremeHeavyChestplate,
    name: "Высший нагрудник тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_CHESTPLATE,
    modifiers: &SUPREME_HEAVY_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static SUPREME_HEAVY_LEGGINGS: Serial = Serial {
    id: SerialId::SupremeHeavyLeggings,
    name: "Высшие поножи тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_LEGGINGS,
    modifiers: &SUPREME_HEAVY_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static SUPREME_HEAVY_BOOTS: Serial = Serial {
    id: SerialId::SupremeHeavyBoots,
    name: "Высшие сабатоны тяжёлой брони",
    category: Category::Armor,
    holder: &vanilla_items::DIAMOND_BOOTS,
    modifiers: &SUPREME_HEAVY_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Heavy),
    spell_gear: None,
};
pub(crate) static SUPREME_ROBE_HELMET: Serial = Serial {
    id: SerialId::SupremeRobeHelmet,
    name: "Высший капюшон роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_HELMET,
    modifiers: &SUPREME_ROBE_HELMET_MODIFIERS,
    gear_kind: Some(GearKind::Head),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static SUPREME_ROBE_CHESTPLATE: Serial = Serial {
    id: SerialId::SupremeRobeChestplate,
    name: "Высшая мантия роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_CHESTPLATE,
    modifiers: &SUPREME_ROBE_CHESTPLATE_MODIFIERS,
    gear_kind: Some(GearKind::Chest),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static SUPREME_ROBE_LEGGINGS: Serial = Serial {
    id: SerialId::SupremeRobeLeggings,
    name: "Высшие брюки роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_LEGGINGS,
    modifiers: &SUPREME_ROBE_LEGGINGS_MODIFIERS,
    gear_kind: Some(GearKind::Legs),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};
pub(crate) static SUPREME_ROBE_BOOTS: Serial = Serial {
    id: SerialId::SupremeRobeBoots,
    name: "Высшие туфли роб",
    category: Category::Armor,
    holder: &vanilla_items::LEATHER_BOOTS,
    modifiers: &SUPREME_ROBE_BOOTS_MODIFIERS,
    gear_kind: Some(GearKind::Feet),
    line: Some(ArmorLine::Robe),
    spell_gear: None,
};

// Supreme weapon serials
pub(crate) static SUPREME_HUNTER_SWORD: Serial = Serial {
    id: SerialId::SupremeHunterSword,
    name: "Высший меч охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_SWORD,
    modifiers: &SUPREME_HUNTER_SWORD_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static SUPREME_HUNTER_DAGGER: Serial = Serial {
    id: SerialId::SupremeHunterDagger,
    name: "Высший кинжал охотника",
    category: Category::Weapon,
    holder: &vanilla_items::STONE_SWORD,
    modifiers: &SUPREME_HUNTER_DAGGER_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static SUPREME_HUNTER_AXE: Serial = Serial {
    id: SerialId::SupremeHunterAxe,
    name: "Высший топор охотника",
    category: Category::Weapon,
    holder: &vanilla_items::IRON_AXE,
    modifiers: &SUPREME_HUNTER_AXE_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static SUPREME_HUNTER_BOW: Serial = Serial {
    id: SerialId::SupremeHunterBow,
    name: "Высший лук охотника",
    category: Category::Weapon,
    holder: &vanilla_items::BOW,
    modifiers: &SUPREME_HUNTER_BOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};
pub(crate) static SUPREME_HUNTER_CROSSBOW: Serial = Serial {
    id: SerialId::SupremeHunterCrossbow,
    name: "Высший арбалет охотника",
    category: Category::Weapon,
    holder: &vanilla_items::CROSSBOW,
    modifiers: &SUPREME_HUNTER_CROSSBOW_MODIFIERS,
    gear_kind: None,
    line: None,
    spell_gear: None,
};

/// Elite serial gear made to order from dungeon materials.
pub const ELITE: [SerialId; 21] = [
    SerialId::EliteLightHelmet,
    SerialId::EliteLightChestplate,
    SerialId::EliteLightLeggings,
    SerialId::EliteLightBoots,
    SerialId::EliteReinforcedHelmet,
    SerialId::EliteReinforcedChestplate,
    SerialId::EliteReinforcedLeggings,
    SerialId::EliteReinforcedBoots,
    SerialId::EliteHeavyHelmet,
    SerialId::EliteHeavyChestplate,
    SerialId::EliteHeavyLeggings,
    SerialId::EliteHeavyBoots,
    SerialId::EliteRobeHelmet,
    SerialId::EliteRobeChestplate,
    SerialId::EliteRobeLeggings,
    SerialId::EliteRobeBoots,
    SerialId::EliteHunterSword,
    SerialId::EliteHunterDagger,
    SerialId::EliteHunterAxe,
    SerialId::EliteHunterBow,
    SerialId::EliteHunterCrossbow,
];

/// Supreme serial gear made to order from dungeon materials.
pub const SUPREME: [SerialId; 21] = [
    SerialId::SupremeLightHelmet,
    SerialId::SupremeLightChestplate,
    SerialId::SupremeLightLeggings,
    SerialId::SupremeLightBoots,
    SerialId::SupremeReinforcedHelmet,
    SerialId::SupremeReinforcedChestplate,
    SerialId::SupremeReinforcedLeggings,
    SerialId::SupremeReinforcedBoots,
    SerialId::SupremeHeavyHelmet,
    SerialId::SupremeHeavyChestplate,
    SerialId::SupremeHeavyLeggings,
    SerialId::SupremeHeavyBoots,
    SerialId::SupremeRobeHelmet,
    SerialId::SupremeRobeChestplate,
    SerialId::SupremeRobeLeggings,
    SerialId::SupremeRobeBoots,
    SerialId::SupremeHunterSword,
    SerialId::SupremeHunterDagger,
    SerialId::SupremeHunterAxe,
    SerialId::SupremeHunterBow,
    SerialId::SupremeHunterCrossbow,
];
