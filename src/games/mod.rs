#[cfg(test)]
pub mod audit;
pub mod baccarat;
pub mod bigsix;
pub mod bingo;
pub mod blackjack;
pub mod cards;
pub mod chuck;
pub mod crash;
pub mod floor;
pub mod hilo;
pub mod horses;
pub mod keno;
pub mod lab;
pub mod luckbet;
pub mod mines;
pub mod pig;
pub mod plinko;
pub mod roulette;
pub mod scratch;
pub mod slots;
pub mod table;
pub mod threecard;
pub mod tournament;
pub mod ultra;
pub mod vidpoker;
pub mod war;
pub mod yahtzee;

use crate::rng::Rng;
use crate::stats::Store;
use crate::ui::{Screen, Theme};

/// Shared state handed to every game: the RNG, the save file, and the
/// screen it draws to. Games never touch the terminal directly — every
/// draw goes through `ctx.screen`, which is what keeps the UI layer
/// swappable without a game file caring.
pub struct Ctx<'a> {
    pub rng: &'a mut Rng,
    pub store: &'a mut Store,
    pub screen: &'a mut Screen,
}

impl<'a> Ctx<'a> {
    pub fn theme(&self) -> Theme {
        self.screen.theme
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn label(&self) -> &'static str {
        match self {
            Difficulty::Easy => "Easy",
            Difficulty::Normal => "Normal",
            Difficulty::Hard => "Hard",
        }
    }

    pub fn from_index(i: usize) -> Difficulty {
        match i {
            1 => Difficulty::Easy,
            3 => Difficulty::Hard,
            _ => Difficulty::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Player {
    pub name: String,
    pub ai: Option<Difficulty>,
    pub score: i64,
}

impl Player {
    pub fn human(name: &str) -> Player {
        Player { name: name.to_string(), ai: None, score: 0 }
    }

    pub fn bot(name: &str, d: Difficulty) -> Player {
        Player { name: name.to_string(), ai: Some(d), score: 0 }
    }

    pub fn is_ai(&self) -> bool {
        self.ai.is_some()
    }
}

/// Lets the human pick a CPU difficulty with a single keypress: E/N/H.
pub fn pick_difficulty(ctx: &mut Ctx, label: &str) -> Difficulty {
    let theme = ctx.theme();
    ctx.screen.begin();
    crate::ui::header(ctx.screen, label);
    ctx.screen.blank();
    ctx.screen.line(&format!("  {} {}", theme.paint(crate::ui::GOLD, "[E]"), "Easy   — erratic, folds early"));
    ctx.screen.line(&format!("  {} {}", theme.paint(crate::ui::GOLD, "[N]"), "Normal — holds around 20"));
    ctx.screen.line(&format!("  {} {}", theme.paint(crate::ui::GOLD, "[H]"), "Hard   — near-optimal, presses when behind"));
    ctx.screen.present();
    match crate::ui::choose_key(&['e', 'n', 'h'], 'n') {
        Some('e') => Difficulty::Easy,
        Some('h') => Difficulty::Hard,
        _ => Difficulty::Normal,
    }
}
