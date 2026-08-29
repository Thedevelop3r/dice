pub mod luckbet;
pub mod pig;
pub mod tournament;
pub mod yahtzee;
pub mod chuck;
pub mod lab;

use crate::rng::Rng;
use crate::stats::Store;

/// Shared state handed to every game.
pub struct Ctx<'a> {
    pub rng: &'a mut Rng,
    pub store: &'a mut Store,
    pub colors: bool,
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
