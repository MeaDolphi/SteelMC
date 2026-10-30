//! Artifact enchanting, modelled on Daggerfall's item maker.
//!
//! The concept follows DFO Online's port of Daggerfall Unity's item maker
//! (MIT): every item carries a budget of enchantment points, every power
//! spends points, and every side effect has a *negative* cost, so accepting a
//! drawback buys room for more power. The Won price counts powers only, at ten
//! Won per point, so a drawback buys budget but never a discount. One item
//! holds at most ten enchantments, each at most once.
//!
//! Enchantments come in two kinds. *Simple* ones are general effects, such as
//! Lifesteal, offered to several item types; *special* ones belong to one item
//! type: a pickaxe digs faster, boots soften falls, a staff feeds spells. Each
//! artifact has an [`ItemKind`] and the item maker offers only what fits it.
//!
//! The budget comes from the artifact's rarity instead of Daggerfall's
//! materials. Attribute enchantments are modifiers written onto the stack;
//! every other effect is summed into an [`EnchantSummary`] of what a hunter
//! wears, which the damage, tick and reward hooks read. Enchanting an artifact
//! never removes its personal binding, and the artifact's own abilities cost
//! none of its points.

use std::collections::BTreeMap;

use steel_core::entity::attribute::AttributeModifierOperation;
use steel_registry::attribute::AttributeRef;
use steel_registry::vanilla_attributes;

use super::artifact::{Ability, Rarity};
use crate::spell::SpellGear;

/// Most enchantments one item may hold.
pub const MAX_ENCHANTMENTS: usize = 10;

/// Won charged per enchantment point of a power.
pub const WON_PER_POINT: u64 = 10;

/// Health a lifesteal hit restores, as a fraction of the damage dealt.
pub const LIFESTEAL_FRACTION: f32 = 0.1;

/// Health a recoil hit takes from the wielder; recoil never kills.
pub const RECOIL_DAMAGE: f32 = 1.0;

/// Returns the enchantment points an artifact of `rarity` carries.
#[must_use]
pub const fn budget(rarity: Rarity) -> i64 {
    match rarity {
        Rarity::Common => 200,
        Rarity::Rare => 400,
        Rarity::Epic => 800,
        Rarity::Legendary => 1500,
    }
}

/// The type of item an enchantment is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// Swords, daggers and axes.
    Melee,
    /// A Mage staff.
    Staff,
    /// A pickaxe.
    Pickaxe,
    /// Helmets and hoods.
    Helmet,
    /// Chestplates and robes.
    Chest,
    /// Leggings.
    Legs,
    /// Boots and shoes.
    Boots,
    /// Gloves and gauntlets.
    Gloves,
    /// Rings and amulets.
    Jewelry,
}

impl ItemKind {
    const fn bit(self) -> u16 {
        match self {
            Self::Melee => MELEE,
            Self::Staff => STAFF,
            Self::Pickaxe => PICKAXE,
            Self::Helmet => HELMET,
            Self::Chest => CHEST,
            Self::Legs => LEGS,
            Self::Boots => BOOTS,
            Self::Gloves => GLOVES,
            Self::Jewelry => JEWELRY,
        }
    }
}

const MELEE: u16 = 1;
const STAFF: u16 = 1 << 1;
const PICKAXE: u16 = 1 << 2;
const HELMET: u16 = 1 << 3;
const CHEST: u16 = 1 << 4;
const LEGS: u16 = 1 << 5;
const BOOTS: u16 = 1 << 6;
const GLOVES: u16 = 1 << 7;
const JEWELRY: u16 = 1 << 8;
const ARMOR: u16 = HELMET | CHEST | LEGS | BOOTS | GLOVES;
const EVERY: u16 = MELEE | STAFF | PICKAXE | ARMOR | JEWELRY;

/// What an enchantment does once applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// An attribute modifier on the stack, active in the artifact's slot.
    Attribute {
        /// Vanilla attribute the modifier applies to.
        attribute: AttributeRef,
        /// Modifier amount, interpreted by `operation`.
        amount: f64,
        /// How `amount` combines with the attribute's other modifiers.
        operation: AttributeModifierOperation,
    },
    /// A melee hit heals the wielder by part of the damage dealt.
    Lifesteal,
    /// A melee hit costs the wielder health, never below one.
    Recoil,
    /// A gameplay effect summed into the wearer's [`EnchantSummary`].
    Mechanic,
}

/// Static data of one enchantment.
struct Spec {
    tag: &'static str,
    display: &'static str,
    description: &'static str,
    cost: i64,
    kinds: u16,
    effect: Effect,
}

const fn attribute(attribute: AttributeRef, amount: f64, multiplied: bool) -> Effect {
    Effect::Attribute {
        attribute,
        amount,
        operation: if multiplied {
            AttributeModifierOperation::AddMultipliedBase
        } else {
            AttributeModifierOperation::AddValue
        },
    }
}

macro_rules! enchantments {
    ($($variant:ident => $key:literal, $name:literal, $description:literal, $cost:expr, $kinds:expr, $effect:expr;)*) => {
        /// One enchantment the item maker offers.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Enchantment {
            $(
                #[doc = $name]
                $variant,
            )*
        }

        /// Every enchantment, in menu order.
        pub const ALL: &[Enchantment] = &[$(Enchantment::$variant),*];

        impl Enchantment {
            fn spec(self) -> Spec {
                match self {
                    $(Self::$variant => Spec {
                        tag: $key,
                        display: $name,
                        description: $description,
                        cost: $cost,
                        kinds: $kinds,
                        effect: $effect,
                    },)*
                }
            }
        }
    };
}

enchantments! {
    // Simple enchantments: general effects for several item types.
    Lifesteal => "lifesteal", "Вампиризм", "Удар лечит на 10% нанесённого урона", 300, MELEE, Effect::Lifesteal;
    Bloodsucker => "bloodsucker", "Кровопийца", "Заклинание разрушения лечит на 8% урона", 300, STAFF, Effect::Mechanic;
    Regeneration => "regeneration", "Регенерация", "+1 HP каждые 10 с", 200, ARMOR | JEWELRY, Effect::Mechanic;
    SecondWind => "second_wind", "Второе дыхание", "Ниже 30% здоровья: +4 HP, раз в 60 с", 250, ARMOR | JEWELRY, Effect::Mechanic;
    Fury => "fury", "Ярость", "Ниже 50% здоровья: +15% урона", 200, MELEE | STAFF | JEWELRY, Effect::Mechanic;
    Evasion => "evasion", "Уклонение", "5% шанс полностью избежать удара", 300, ARMOR | JEWELRY, Effect::Mechanic;
    Reflection => "reflection", "Отражение", "10% урона ближнего боя возвращается атакующему", 200, ARMOR, Effect::Mechanic;
    Flight => "flight", "Бегство", "Получив урон: +20% скорости на 3 с, раз в 15 с", 120, ARMOR | JEWELRY, Effect::Mechanic;
    Hunter => "hunter", "Охотник", "+10% опыта охотника за убийства", 250, MELEE | STAFF | JEWELRY, Effect::Mechanic;
    Gatherer => "gatherer", "Добытчик", "+10% очков Системы за убийства", 250, MELEE | STAFF | JEWELRY, Effect::Mechanic;
    Antidote => "antidote", "Противоядие", "Иммунитет к яду и иссушению", 150, ARMOR | JEWELRY, Effect::Mechanic;
    ManaFeed => "mana_feed", "Подпитка маны", "Удар или заклинание даёт магу +2 маны", 150, MELEE | STAFF | JEWELRY, Effect::Mechanic;
    Focus => "focus", "Сосредоточенность", "−10% перезарядки способностей артефактов", 200, MELEE | STAFF | JEWELRY, Effect::Mechanic;
    Fortress => "fortress", "Крепость", "−5% входящего урона", 250, ARMOR | JEWELRY, Effect::Mechanic;
    // Melee weapons.
    Sharpness => "sharpness", "Острота", "+1 урона атаки", 150, MELEE, attribute(vanilla_attributes::ATTACK_DAMAGE, 1.0, false);
    Swiftblade => "swiftblade", "Быстрый клинок", "+10% скорости атаки", 120, MELEE, attribute(vanilla_attributes::ATTACK_SPEED, 0.1, true);
    Executioner => "executioner", "Палач", "+25% урона по целям ниже 30% здоровья", 200, MELEE, Effect::Mechanic;
    Bossbane => "bossbane", "Гроза боссов", "+15% урона по боссам врат", 250, MELEE, Effect::Mechanic;
    Crush => "crush", "Сокрушение", "10% шанс удара ×1,5", 200, MELEE, Effect::Mechanic;
    Cleave => "cleave", "Рассечение", "30% урона удара задевает мобов в радиусе 2", 300, MELEE, Effect::Mechanic;
    Venom => "venom", "Яд", "20% шанс отравить цель на 4 с", 150, MELEE, Effect::Mechanic;
    FrostEdge => "frost_edge", "Ледяная кромка", "20% шанс замедлить цель на 2 с", 150, MELEE, Effect::Mechanic;
    Flame => "flame", "Пламя", "Удар поджигает цель на 3 с", 150, MELEE, Effect::Mechanic;
    Reach => "reach", "Длинная рука", "+0,5 блока дальности удара", 120, MELEE, attribute(vanilla_attributes::ENTITY_INTERACTION_RANGE, 0.5, false);
    Knockback => "knockback", "Отброс", "Сильнее отбрасывает цель", 80, MELEE, attribute(vanilla_attributes::ATTACK_KNOCKBACK, 0.5, false);
    // Mage staffs.
    Spellpower => "spellpower", "Сила чар", "+8% магического урона", 200, STAFF, Effect::Mechanic;
    ManaWell => "mana_well", "Источник", "+25 маны", 150, STAFF, Effect::Mechanic;
    Flow => "flow", "Поток", "+15% восстановления маны", 150, STAFF, Effect::Mechanic;
    Quickcast => "quickcast", "Скорописание", "−8% перезарядки заклинаний", 250, STAFF, Effect::Mechanic;
    Thrift => "thrift", "Бережливость", "−8% стоимости заклинаний", 250, STAFF, Effect::Mechanic;
    Grace => "grace", "Благодать", "+12% силы исцеления", 200, STAFF, Effect::Mechanic;
    Echo => "echo", "Эхо", "+5% шанс повторить заклинание разрушения", 300, STAFF, Effect::Mechanic;
    Embers => "embers", "Тлеющий жар", "Огненные заклинания поджигают вдвое дольше", 120, STAFF, Effect::Mechanic;
    // Pickaxes.
    Quarrying => "quarrying", "Проворство", "+30% скорости добычи", 120, PICKAXE, attribute(vanilla_attributes::BLOCK_BREAK_SPEED, 0.3, true);
    Prospector => "prospector", "Старатель", "+15% шанс лишнего кристалла", 250, PICKAXE, Effect::Mechanic;
    EssenceSense => "essence_sense", "Чутьё на эссенцию", "3% шанс эссенции из кристальной руды", 300, PICKAXE, Effect::Mechanic;
    LongSwing => "long_swing", "Дальний замах", "+1 блок дальности добычи", 100, PICKAXE, attribute(vanilla_attributes::BLOCK_INTERACTION_RANGE, 1.0, false);
    MiningRush => "mining_rush", "Азарт добычи", "Добыча кристалла: +20% скорости добычи на 5 с", 100, PICKAXE, Effect::Mechanic;
    // Armor and jewelry.
    Protection => "protection", "Защита", "+1 брони", 150, HELMET | CHEST | LEGS | BOOTS, attribute(vanilla_attributes::ARMOR, 1.0, false);
    Vitality => "vitality", "Живучесть", "+2 максимального здоровья", 200, HELMET | CHEST | JEWELRY, attribute(vanilla_attributes::MAX_HEALTH, 2.0, false);
    ClearSight => "clear_sight", "Ясный взор", "Иммунитет к слепоте и тьме", 150, HELMET, Effect::Mechanic;
    DeepBreath => "deep_breath", "Глубокий вдох", "+2 к запасу воздуха", 80, HELMET, attribute(vanilla_attributes::OXYGEN_BONUS, 2.0, false);
    ColdMind => "cold_mind", "Холодный разум", "+20 маны (только магу)", 150, HELMET, Effect::Mechanic;
    Toughness => "toughness", "Закалка", "+1 прочности брони", 150, CHEST, attribute(vanilla_attributes::ARMOR_TOUGHNESS, 1.0, false);
    Thorns => "thorns", "Шипы", "Возвращает 15% урона ближнего боя атакующему", 250, CHEST, Effect::Mechanic;
    Restoration => "restoration", "Восстановление", "+1 HP каждые 8 с", 250, CHEST, Effect::Mechanic;
    FireWard => "fire_ward", "Огнеупорность", "−50% урона от огня", 120, CHEST, Effect::Mechanic;
    BlastWard => "blast_ward", "Взрывостойкость", "−30% урона от взрывов", 120, CHEST, Effect::Mechanic;
    Steadfast => "steadfast", "Стойкость", "+10% сопротивления отбрасыванию", 100, LEGS, attribute(vanilla_attributes::KNOCKBACK_RESISTANCE, 0.1, false);
    ArrowWard => "arrow_ward", "Укрытие от стрел", "−25% урона снарядами", 150, LEGS, Effect::Mechanic;
    Dash => "dash", "Рывок", "+8% скорости", 200, LEGS, attribute(vanilla_attributes::MOVEMENT_SPEED, 0.08, true);
    Fleetness => "fleetness", "Лёгкость", "+5% скорости", 150, BOOTS | JEWELRY, attribute(vanilla_attributes::MOVEMENT_SPEED, 0.05, true);
    SoftFall => "soft_fall", "Мягкое падение", "−40% урона от падения", 100, BOOTS, attribute(vanilla_attributes::FALL_DAMAGE_MULTIPLIER, -0.4, false);
    Leaping => "leaping", "Прыгучесть", "Выше прыжок", 100, BOOTS, attribute(vanilla_attributes::JUMP_STRENGTH, 0.1, false);
    Stride => "stride", "Широкий шаг", "Перешагивает блоки высотой 1", 120, BOOTS, attribute(vanilla_attributes::STEP_HEIGHT, 0.5, false);
    Grip => "grip", "Хватка", "+8% скорости атаки", 150, GLOVES, attribute(vanilla_attributes::ATTACK_SPEED, 0.08, true);
    Brawler => "brawler", "Кулак бойца", "+1 урона атаки", 150, GLOVES, attribute(vanilla_attributes::ATTACK_DAMAGE, 1.0, false);
    NimbleFingers => "nimble_fingers", "Ловкие пальцы", "+20% скорости добычи", 100, GLOVES, attribute(vanilla_attributes::BLOCK_BREAK_SPEED, 0.2, true);
    Wisdom => "wisdom", "Мудрость", "+10% опыта охотника за убийства", 300, JEWELRY, Effect::Mechanic;
    Greed => "greed", "Алчность", "+10% очков Системы за убийства", 300, JEWELRY, Effect::Mechanic;
    ProspectorsLuck => "prospectors_luck", "Удача старателя", "+10% шанс лишнего кристалла", 200, JEWELRY, Effect::Mechanic;
    ManaResonance => "mana_resonance", "Резонанс маны", "+30 маны (только магу)", 200, JEWELRY, Effect::Mechanic;
    // Side effects.
    Sluggish => "sluggish", "Медлительность", "−5% скорости", -150, EVERY, attribute(vanilla_attributes::MOVEMENT_SPEED, -0.05, true);
    Frail => "frail", "Хрупкость", "−2 максимального здоровья", -200, EVERY, attribute(vanilla_attributes::MAX_HEALTH, -2.0, false);
    Vulnerability => "vulnerability", "Уязвимость", "+10% входящего урона", -250, EVERY, Effect::Mechanic;
    BloodCurse => "blood_curse", "Проклятие крови", "Лечение на 20% слабее", -150, EVERY, Effect::Mechanic;
    Distraction => "distraction", "Рассеянность", "−10% опыта охотника", -100, EVERY, Effect::Mechanic;
    Heavy => "heavy", "Тяжесть", "−10% скорости атаки", -150, MELEE, attribute(vanilla_attributes::ATTACK_SPEED, -0.1, true);
    Recoil => "recoil", "Отдача", "Попадание отнимает 1 HP у владельца (не убивает)", -250, MELEE, Effect::Recoil;
    Dull => "dull", "Тупое лезвие", "−1 урона атаки", -150, MELEE, attribute(vanilla_attributes::ATTACK_DAMAGE, -1.0, false);
    ShortReach => "short_reach", "Короткая рука", "−0,5 блока дальности удара", -100, MELEE, attribute(vanilla_attributes::ENTITY_INTERACTION_RANGE, -0.5, false);
    ManaLeak => "mana_leak", "Утечка маны", "−15% восстановления маны", -150, STAFF, Effect::Mechanic;
    Unstable => "unstable", "Нестабильность", "Заклинания на 15% дороже", -200, STAFF, Effect::Mechanic;
    Clumsy => "clumsy", "Неуклюжесть", "−20% скорости добычи", -120, PICKAXE, attribute(vanilla_attributes::BLOCK_BREAK_SPEED, -0.2, true);
    HeavyTread => "heavy_tread", "Тяжёлая поступь", "−10% сопротивления отбрасыванию", -80, ARMOR, attribute(vanilla_attributes::KNOCKBACK_RESISTANCE, -0.1, false);
}

impl Enchantment {
    /// Returns the stable tag value of the enchantment.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.spec().tag
    }

    /// Parses the stable tag value.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.iter()
            .copied()
            .find(|enchantment| enchantment.name() == name)
    }

    /// Returns the Russian display name.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        self.spec().display
    }

    /// Returns the enchantment cost in points; side effects are negative.
    #[must_use]
    pub fn cost(self) -> i64 {
        self.spec().cost
    }

    /// Returns whether the enchantment is a side effect.
    #[must_use]
    pub fn is_side_effect(self) -> bool {
        self.cost() < 0
    }

    /// Returns whether the enchantment is offered to several item types.
    #[must_use]
    pub fn is_simple(self) -> bool {
        self.spec().kinds.count_ones() > 1
    }

    /// Returns the Won price; side effects are free.
    #[must_use]
    pub fn price(self) -> u64 {
        if self.cost() > 0 {
            self.cost().unsigned_abs() * WON_PER_POINT
        } else {
            0
        }
    }

    /// Returns whether an item of `kind` may take the enchantment.
    #[must_use]
    pub fn fits(self, kind: ItemKind) -> bool {
        self.spec().kinds & kind.bit() != 0
    }

    /// Returns what the enchantment does.
    #[must_use]
    pub fn effect(self) -> Effect {
        self.spec().effect
    }

    /// Returns a one-line Russian description for tooltips.
    #[must_use]
    pub fn describe(self) -> &'static str {
        self.spec().description
    }
}

/// Everything the enchantments a hunter wears add up to, beyond attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EnchantSummary {
    /// Share of destruction-spell damage healed.
    pub bloodsucker: f32,
    /// Health regenerated per second.
    pub regen_per_second: f32,
    /// Second Wind is worn.
    pub second_wind: bool,
    /// Extra damage below half health, as a fraction.
    pub fury: f32,
    /// Chance to evade a hit, as a fraction.
    pub evasion: f32,
    /// Share of melee damage returned to the attacker.
    pub reflect: f32,
    /// Flight is worn.
    pub flight: bool,
    /// Change of hunter experience from kills, as a fraction.
    pub experience: f32,
    /// Change of system points from kills, as a fraction.
    pub points: f32,
    /// Immunity to poison and wither.
    pub antidote: bool,
    /// Mana a Mage gains per hit or spell.
    pub mana_feed: f32,
    /// Reduction of artifact ability cooldowns, as a fraction.
    pub focus: f32,
    /// Change of incoming damage, as a fraction; negative reduces it.
    pub incoming: f32,
    /// Change of healing received, as a fraction.
    pub healing_received: f32,
    /// Extra damage to targets below 30% health.
    pub executioner: f32,
    /// Extra damage to gate bosses.
    pub bossbane: f32,
    /// Chance of a crushing hit, as a fraction.
    pub crush: f32,
    /// Share of a hit that cleaves nearby mobs.
    pub cleave: f32,
    /// Chance to poison on hit.
    pub venom: f32,
    /// Chance to slow on hit.
    pub frost: f32,
    /// Hits set the target on fire.
    pub flame: bool,
    /// Immunity to blindness and darkness.
    pub clear_sight: bool,
    /// Reduction of fire damage.
    pub fire_ward: f32,
    /// Reduction of explosion damage.
    pub blast_ward: f32,
    /// Reduction of projectile damage.
    pub arrow_ward: f32,
    /// Percent chance of an extra crystal.
    pub crystal_percent: u32,
    /// Percent chance of an essence from crystal ore.
    pub essence_percent: u32,
    /// Mining Rush is worn.
    pub mining_rush: bool,
    /// Spellcasting bonuses, used only by a Mage.
    pub spell: SpellGear,
    /// Share of melee damage healed by artifacts.
    pub bloodthirst: f32,
    /// Health fraction below which Berserk acts; zero when not worn.
    pub berserk_threshold: f32,
    /// Berserk damage bonus.
    pub berserk_bonus: f32,
    /// Melee hits per Double Strike; zero when not worn.
    pub double_strike_every: u32,
    /// Damage of a Double Strike as a fraction of the hit.
    pub double_strike_share: f32,
    /// Shadow Guard invulnerability seconds and cooldown, when worn.
    pub shadow_guard: Option<(u64, u64)>,
}

impl EnchantSummary {
    /// Nothing worn.
    pub const NONE: Self = Self {
        bloodsucker: 0.0,
        regen_per_second: 0.0,
        second_wind: false,
        fury: 0.0,
        evasion: 0.0,
        reflect: 0.0,
        flight: false,
        experience: 0.0,
        points: 0.0,
        antidote: false,
        mana_feed: 0.0,
        focus: 0.0,
        incoming: 0.0,
        healing_received: 0.0,
        executioner: 0.0,
        bossbane: 0.0,
        crush: 0.0,
        cleave: 0.0,
        venom: 0.0,
        frost: 0.0,
        flame: false,
        clear_sight: false,
        fire_ward: 0.0,
        blast_ward: 0.0,
        arrow_ward: 0.0,
        crystal_percent: 0,
        essence_percent: 0,
        mining_rush: false,
        spell: SpellGear::NONE,
        bloodthirst: 0.0,
        berserk_threshold: 0.0,
        berserk_bonus: 0.0,
        double_strike_every: 0,
        double_strike_share: 0.0,
        shadow_guard: None,
    };

    /// Adds the effects of worn artifact abilities that act like
    /// enchantments.
    #[must_use]
    pub fn with_abilities(mut self, abilities: &[Ability]) -> Self {
        for ability in abilities {
            match *ability {
                Ability::BossSlayer { percent } => self.bossbane += percent,
                Ability::Greed { percent } => self.points += percent,
                Ability::CrystalLuck { percent } => self.crystal_percent += percent,
                Ability::Bloodthirst { percent } => self.bloodthirst += percent,
                Ability::Berserk { threshold, percent } => {
                    self.berserk_threshold = self.berserk_threshold.max(threshold);
                    self.berserk_bonus += percent;
                }
                Ability::DoubleStrike { every, percent } => {
                    self.double_strike_every = every;
                    self.double_strike_share = percent;
                }
                Ability::ShadowGuard { seconds, cooldown } => {
                    self.shadow_guard = Some((seconds, cooldown));
                }
                _ => {}
            }
        }
        self
    }

    /// Sums the effects of every enchantment a hunter wears.
    #[must_use]
    pub fn of(enchantments: impl IntoIterator<Item = Enchantment>) -> Self {
        let mut summary = Self::NONE;
        for enchantment in enchantments {
            summary.add(enchantment);
        }
        summary
    }

    fn add(&mut self, enchantment: Enchantment) {
        let spell = &mut self.spell;
        match enchantment {
            Enchantment::Bloodsucker => self.bloodsucker += 0.08,
            Enchantment::Regeneration => self.regen_per_second += 0.1,
            Enchantment::Restoration => self.regen_per_second += 0.125,
            Enchantment::SecondWind => self.second_wind = true,
            Enchantment::Fury => self.fury += 0.15,
            Enchantment::Evasion => self.evasion += 0.05,
            Enchantment::Reflection => self.reflect += 0.10,
            Enchantment::Thorns => self.reflect += 0.15,
            Enchantment::Flight => self.flight = true,
            Enchantment::Hunter | Enchantment::Wisdom => self.experience += 0.10,
            Enchantment::Distraction => self.experience -= 0.10,
            Enchantment::Gatherer | Enchantment::Greed => self.points += 0.10,
            Enchantment::Antidote => self.antidote = true,
            Enchantment::ManaFeed => self.mana_feed += 2.0,
            Enchantment::Focus => self.focus += 0.10,
            Enchantment::Fortress => self.incoming -= 0.05,
            Enchantment::Vulnerability => self.incoming += 0.10,
            Enchantment::BloodCurse => self.healing_received -= 0.20,
            Enchantment::Executioner => self.executioner += 0.25,
            Enchantment::Bossbane => self.bossbane += 0.15,
            Enchantment::Crush => self.crush += 0.10,
            Enchantment::Cleave => self.cleave += 0.30,
            Enchantment::Venom => self.venom += 0.20,
            Enchantment::FrostEdge => self.frost += 0.20,
            Enchantment::Flame => self.flame = true,
            Enchantment::ClearSight => self.clear_sight = true,
            Enchantment::FireWard => self.fire_ward += 0.50,
            Enchantment::BlastWard => self.blast_ward += 0.30,
            Enchantment::ArrowWard => self.arrow_ward += 0.25,
            Enchantment::Prospector => self.crystal_percent += 15,
            Enchantment::ProspectorsLuck => self.crystal_percent += 10,
            Enchantment::EssenceSense => self.essence_percent += 3,
            Enchantment::MiningRush => self.mining_rush = true,
            Enchantment::Spellpower => spell.magic += 0.08,
            Enchantment::ManaWell => spell.mana += 25.0,
            Enchantment::ColdMind => spell.mana += 20.0,
            Enchantment::ManaResonance => spell.mana += 30.0,
            Enchantment::Flow => spell.regen += 0.15,
            Enchantment::ManaLeak => spell.regen -= 0.15,
            Enchantment::Quickcast => spell.cooldown += 0.08,
            Enchantment::Thrift => spell.cost -= 0.08,
            Enchantment::Unstable => spell.cost += 0.15,
            Enchantment::Grace => spell.healing += 0.12,
            Enchantment::Echo => spell.echo_percent += 5,
            Enchantment::Embers => spell.burning = true,
            // Attribute enchantments live on the stack; Lifesteal and Recoil
            // read the wielded weapon on every hit.
            Enchantment::Lifesteal
            | Enchantment::Recoil
            | Enchantment::Sharpness
            | Enchantment::Swiftblade
            | Enchantment::Reach
            | Enchantment::Knockback
            | Enchantment::Quarrying
            | Enchantment::LongSwing
            | Enchantment::Protection
            | Enchantment::Vitality
            | Enchantment::DeepBreath
            | Enchantment::Toughness
            | Enchantment::Steadfast
            | Enchantment::Dash
            | Enchantment::Fleetness
            | Enchantment::SoftFall
            | Enchantment::Leaping
            | Enchantment::Stride
            | Enchantment::Grip
            | Enchantment::Brawler
            | Enchantment::NimbleFingers
            | Enchantment::Sluggish
            | Enchantment::Frail
            | Enchantment::Heavy
            | Enchantment::Dull
            | Enchantment::ShortReach
            | Enchantment::Clumsy
            | Enchantment::HeavyTread => {}
        }
    }

    /// Returns `amount` of healing after the wearer's healing changes.
    #[must_use]
    pub fn healed(self, amount: f32) -> f32 {
        (amount * (1.0 + self.healing_received)).max(0.0)
    }
}

/// Live enchantment state of one hunter; never persisted.
#[derive(Debug, Clone, Default)]
pub struct EnchantState {
    /// What the hunter's worn enchantments add up to, refreshed on ticks.
    pub summary: EnchantSummary,
    /// Cooldown deadlines of timed enchantments, in Unix milliseconds.
    pub cooldowns: BTreeMap<Enchantment, u64>,
    /// Health regeneration not yet paid out.
    pub regen_owed: f32,
    /// The hunter was hit and Flight may trigger on the next tick.
    pub flight_due: bool,
    /// A crystal was mined and Mining Rush may trigger on the next tick.
    pub rush_due: bool,
    /// Melee hits counted toward the next Double Strike.
    pub strikes: u32,
    /// When the Shadow Guard amulet may save its wearer again.
    pub guard_ready_at: Option<u64>,
}

impl EnchantState {
    /// A hunter with no enchantments counted yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            summary: EnchantSummary::NONE,
            cooldowns: BTreeMap::new(),
            regen_owed: 0.0,
            flight_due: false,
            rush_due: false,
            strikes: 0,
            guard_ready_at: None,
        }
    }

    /// Starts the cooldown of `enchantment` if it is ready; returns whether
    /// it was.
    pub fn trigger(&mut self, enchantment: Enchantment, now: u64, cooldown_millis: u64) -> bool {
        if self
            .cooldowns
            .get(&enchantment)
            .is_some_and(|deadline| *deadline > now)
        {
            return false;
        }
        self.cooldowns.insert(enchantment, now + cooldown_millis);
        true
    }
}

/// Why an enchantment cannot be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantError {
    /// Only catalog artifacts can be enchanted.
    NotArtifact,
    /// The enchantment does not fit this kind of artifact.
    WrongCategory,
    /// The artifact already has this enchantment.
    AlreadyApplied,
    /// The artifact already holds the maximum number of enchantments.
    TooMany,
    /// The enchantment would exceed the artifact's point budget.
    OverBudget,
    /// The hunter cannot pay the Won price.
    InsufficientFunds,
    /// Storage is unavailable or another payment is still committing.
    Storage,
}

impl EnchantError {
    /// Returns the Russian message shown to the hunter.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotArtifact => "Зачаровать можно только артефакт в главной руке.",
            Self::WrongCategory => "Это зачарование не подходит для этого предмета.",
            Self::AlreadyApplied => "Это зачарование уже наложено.",
            Self::TooMany => "На предмете уже 10 зачарований.",
            Self::OverBudget => "Не хватает очков зачарования предмета.",
            Self::InsufficientFunds => "Недостаточно Won.",
            Self::Storage => "Хранилище занято или недоступно: повторите позже.",
        }
    }
}

/// Returns the points the applied enchantments spend; side effects subtract.
#[must_use]
pub fn points_used(applied: &[Enchantment]) -> i64 {
    applied.iter().map(|enchantment| enchantment.cost()).sum()
}

/// Checks whether `next` may be added to an artifact of `kind` holding
/// `applied`.
///
/// Like Daggerfall's item maker, the price is checked before the budget, so a
/// hunter who can afford neither hears about the money.
pub fn check(
    rarity: Rarity,
    kind: Option<ItemKind>,
    applied: &[Enchantment],
    next: Enchantment,
    balance: u64,
) -> Result<(), EnchantError> {
    if !kind.is_some_and(|kind| next.fits(kind)) {
        return Err(EnchantError::WrongCategory);
    }
    if applied.contains(&next) {
        return Err(EnchantError::AlreadyApplied);
    }
    if applied.len() >= MAX_ENCHANTMENTS {
        return Err(EnchantError::TooMany);
    }
    if balance < next.price() {
        return Err(EnchantError::InsufficientFunds);
    }
    if points_used(applied) + next.cost() > budget(rarity) {
        return Err(EnchantError::OverBudget);
    }
    Ok(())
}

/// Encodes an enchantment list for the item tag.
#[must_use]
pub fn encode(applied: &[Enchantment]) -> String {
    applied
        .iter()
        .map(|enchantment| enchantment.name())
        .collect::<Vec<_>>()
        .join(",")
}

/// Decodes an enchantment list, skipping unknown names.
#[must_use]
pub fn decode(stored: &str) -> Vec<Enchantment> {
    stored
        .split(',')
        .filter_map(Enchantment::from_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [ItemKind; 9] = [
        ItemKind::Melee,
        ItemKind::Staff,
        ItemKind::Pickaxe,
        ItemKind::Helmet,
        ItemKind::Chest,
        ItemKind::Legs,
        ItemKind::Boots,
        ItemKind::Gloves,
        ItemKind::Jewelry,
    ];

    #[test]
    fn the_catalog_has_unique_names_and_fits_every_kind() {
        assert_eq!(ALL.len(), 75);
        let mut names: Vec<&str> = ALL.iter().map(|enchantment| enchantment.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
        let mut shown: Vec<&str> = ALL
            .iter()
            .map(|enchantment| enchantment.display_name())
            .collect();
        shown.sort_unstable();
        shown.dedup();
        assert_eq!(shown.len(), ALL.len());
        for kind in KINDS {
            let powers = ALL
                .iter()
                .filter(|enchantment| enchantment.fits(kind) && !enchantment.is_side_effect())
                .count();
            let drawbacks = ALL
                .iter()
                .filter(|enchantment| enchantment.fits(kind) && enchantment.is_side_effect())
                .count();
            assert!((3..=27).contains(&powers), "{kind:?} has {powers} powers");
            assert!(
                (1..=9).contains(&drawbacks),
                "{kind:?} has {drawbacks} drawbacks"
            );
        }
        assert!(Enchantment::Lifesteal.fits(ItemKind::Melee));
        assert!(!Enchantment::Lifesteal.fits(ItemKind::Pickaxe));
        assert!(Enchantment::Prospector.fits(ItemKind::Pickaxe));
        assert!(!Enchantment::Sharpness.fits(ItemKind::Staff));
        assert!(Enchantment::Regeneration.is_simple());
        assert!(!Enchantment::Thorns.is_simple());
    }

    #[test]
    fn side_effects_buy_budget_but_not_a_discount() {
        let melee = Some(ItemKind::Melee);
        let rare = [Enchantment::Lifesteal];
        assert_eq!(
            check(Rarity::Rare, melee, &rare, Enchantment::Sharpness, u64::MAX),
            Err(EnchantError::OverBudget)
        );
        let with_drawback = [Enchantment::Lifesteal, Enchantment::Recoil];
        assert!(
            check(
                Rarity::Rare,
                melee,
                &with_drawback,
                Enchantment::Sharpness,
                u64::MAX
            )
            .is_ok()
        );
        assert_eq!(points_used(&with_drawback), 50);
        assert_eq!(Enchantment::Recoil.price(), 0);
        assert_eq!(Enchantment::Sharpness.price(), 1500);
    }

    #[test]
    fn the_price_is_checked_before_the_budget() {
        let full = [Enchantment::Lifesteal];
        assert_eq!(
            check(
                Rarity::Rare,
                Some(ItemKind::Melee),
                &full,
                Enchantment::Sharpness,
                0
            ),
            Err(EnchantError::InsufficientFunds)
        );
    }

    #[test]
    fn kinds_duplicates_and_the_limit_are_enforced() {
        let chest = Some(ItemKind::Chest);
        assert_eq!(
            check(Rarity::Epic, chest, &[], Enchantment::Sharpness, u64::MAX),
            Err(EnchantError::WrongCategory)
        );
        assert_eq!(
            check(Rarity::Epic, None, &[], Enchantment::Frail, u64::MAX),
            Err(EnchantError::WrongCategory)
        );
        assert_eq!(
            check(
                Rarity::Epic,
                chest,
                &[Enchantment::Frail],
                Enchantment::Frail,
                u64::MAX
            ),
            Err(EnchantError::AlreadyApplied)
        );
        let ten = [Enchantment::Sluggish; MAX_ENCHANTMENTS];
        assert_eq!(
            check(Rarity::Legendary, chest, &ten, Enchantment::Frail, u64::MAX),
            Err(EnchantError::TooMany)
        );
    }

    #[test]
    fn worn_enchantments_add_up() {
        let summary = EnchantSummary::of([
            Enchantment::Hunter,
            Enchantment::Wisdom,
            Enchantment::Distraction,
            Enchantment::Thorns,
            Enchantment::Reflection,
            Enchantment::Fortress,
            Enchantment::Vulnerability,
            Enchantment::ManaWell,
            Enchantment::Thrift,
        ]);
        assert!((summary.experience - 0.10).abs() < 1e-6);
        assert!((summary.reflect - 0.25).abs() < 1e-6);
        assert!((summary.incoming - 0.05).abs() < 1e-6);
        assert!((summary.spell.mana - 25.0).abs() < 1e-6);
        assert!((summary.spell.cost + 0.08).abs() < 1e-6);
        let cursed = EnchantSummary::of([Enchantment::BloodCurse]);
        assert!((cursed.healed(10.0) - 8.0).abs() < 1e-6);
    }

    #[test]
    fn timed_enchantments_wait_for_their_cooldown() {
        let mut state = EnchantState::new();
        assert!(state.trigger(Enchantment::SecondWind, 1_000, 60_000));
        assert!(!state.trigger(Enchantment::SecondWind, 30_000, 60_000));
        assert!(state.trigger(Enchantment::SecondWind, 61_000, 60_000));
    }

    #[test]
    fn a_list_round_trips_through_its_tag() {
        let applied = vec![
            Enchantment::Protection,
            Enchantment::Frail,
            Enchantment::Cleave,
        ];
        assert_eq!(decode(&encode(&applied)), applied);
        assert_eq!(decode(""), Vec::<Enchantment>::new());
    }
}
