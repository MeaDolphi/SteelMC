//! Newcomer guidance built from existing achievements and game menus.

use crate::quest_book::Achievement;

/// The existing path steps that make up the short introductory route.
pub const QUICK_START: [Achievement; 6] = [
    Achievement::StarterKit,
    Achievement::FirstGate,
    Achievement::FirstBoss,
    Achievement::Level10,
    Achievement::RankD,
    Achievement::ClassChosen,
];

/// Returns the first unfinished introductory achievement.
#[must_use]
pub fn next_step(achievements: &[Achievement]) -> Option<Achievement> {
    QUICK_START
        .into_iter()
        .find(|step| !achievements.contains(step))
}

/// A system described by one short reference card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideChapter {
    /// System book, combat keys and the Mage grimoire.
    Controls,
    /// Body armor and artifact equipment.
    Equipment,
    /// Levels, stats, skills and classes.
    Development,
    /// Gate navigation, combat and crystal mining.
    Gates,
    /// Won, system points and the different shops.
    Economy,
    /// Shadow roster and summoning.
    Shadows,
    /// Parties, guilds and the auction.
    Group,
}

/// One immutable reference card shown in the guide.
#[derive(Debug, Clone, Copy)]
pub struct GuideSection {
    /// System this card opens.
    pub chapter: GuideChapter,
    /// Short Russian title.
    pub title: &'static str,
    /// Two to four concise explanation lines.
    pub lines: &'static [&'static str],
}

/// Every reference card, in menu order.
pub const SECTIONS: [GuideSection; 7] = [
    GuideSection {
        chapter: GuideChapter::Controls,
        title: "Управление",
        lines: &[
            "ПКМ по «Системе» в 9-м слоте открывает все меню.",
            "Q — классовый, Shift+Q — общий навык.",
            "Ctrl+Q — врождённый навык. Всё работает пустой рукой.",
            "Гримуар мага: ПКМ — каст, Shift+ПКМ — выбор, Q — смена.",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Equipment,
        title: "Снаряжение и броня",
        lines: &[
            "Обычная броня и стартовый сет надеваются на тело.",
            "Артефакты и редкое снаряжение — в меню «Снаряжение».",
            "Стартовый сет не артефакт; артефакт-броня на тело не надевается.",
            "Посох мага даёт бонусы только из «Снаряжения».",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Development,
        title: "Развитие охотника",
        lines: &[
            "Уровни дают очки характеристик и навыков в меню «Охотник».",
            "Ранг растёт с уровнем; класс открывается с D.",
            "Q применяет навыки, распределение можно сбросить вне данжа.",
            "Личные вещи другого класса ждут в классовом Хранилище.",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Gates,
        title: "Врата и кристаллы",
        lines: &[
            "Меню «Врата» показывает ранг и переносит к входу.",
            "Группа входит вместе с лидером; HP мобов видно над головой.",
            "Кристальная руда добывается киркой и попадает в инвентарь.",
            "Зачистите комнаты и босса; выход появится в данже.",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Economy,
        title: "Деньги и магазины",
        lines: &[
            "Won нужен торговцам, очки Системы — магазину Системы.",
            "Кристаллы продаются Организации через «Продать кристаллы».",
            "Постоянный магазин продаёт расходники и еду.",
            "Случайный магазин предлагает дорогие артефакты.",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Shadows,
        title: "Армия теней",
        lines: &[
            "Поднятые тени хранятся в «Армии теней».",
            "Призывайте нужную тень и отзывайте её перед новой охотой.",
            "Сильные тени имеют собственные классы и способности.",
        ],
    },
    GuideSection {
        chapter: GuideChapter::Group,
        title: "Группа и сообщество",
        lines: &[
            "Пригласите союзника в группу; лидер ведёт её во врата.",
            "Гильдия открывает общие задания и казну.",
            "Аукцион нужен для торговли с другими охотниками.",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::{QUICK_START, SECTIONS, next_step};
    use crate::quest_book::Achievement;

    #[test]
    fn quick_start_returns_the_first_unfinished_existing_achievement() {
        let done = [
            Achievement::StarterKit,
            Achievement::FirstBoss,
            Achievement::Level10,
        ];
        assert_eq!(next_step(&done), Some(Achievement::FirstGate));
        assert_eq!(next_step(&QUICK_START), None);
    }

    #[test]
    fn reference_mentions_current_keys_and_equipment_separation() {
        let text = SECTIONS
            .iter()
            .flat_map(|section| section.lines.iter().copied())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Q — классовый",
            "Shift+Q — общий",
            "Ctrl+Q — врождённый",
            "Стартовый сет не артефакт",
            "Посох мага",
        ] {
            assert!(text.contains(expected), "missing {expected:?}");
        }
    }
}
