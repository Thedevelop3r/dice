//! The floor plan: which tables exist, how they are grouped on the
//! dashboard, and the screensaver that walks the whole building.
//!
//! With two dozen tables a single flat menu stopped being readable, so the
//! floor is grouped the way a real one is — dice in one room, cards in
//! another, the wheels, and the arcade. Keeping that layout here rather
//! than in `main.rs` means adding a table is a line in one list.

use super::{
    baccarat, bigsix, bingo, blackjack, chuck, crash, hilo, horses, keno, mines, plinko, roulette, scratch, slots, table, threecard, vidpoker, war, Ctx,
};
use crate::ui::{self, widgets, Screen};

/// A table that can run itself with nobody watching.
pub struct Attract {
    pub label: &'static str,
    pub run: fn(&mut Ctx),
}

/// Every table with a hands-off mode, in the order the floor cycler walks
/// them. Ultra Casino Dice is deliberately absent: it is already a
/// self-running session with its own leave handling, so it gets a floor
/// entry of its own rather than a slot in the rotation it would never
/// hand back.
pub const ATTRACT: [Attract; 17] = [
    Attract { label: "Chuck-a-Luck", run: chuck::idle },
    Attract { label: "Roulette", run: roulette::idle },
    Attract { label: "Blackjack", run: blackjack::idle },
    Attract { label: "Baccarat", run: baccarat::idle },
    Attract { label: "Video Poker", run: vidpoker::idle },
    Attract { label: "Three Card Poker", run: threecard::idle },
    Attract { label: "Casino War", run: war::idle },
    Attract { label: "Hi-Lo", run: hilo::idle },
    Attract { label: "Big Six", run: bigsix::idle },
    Attract { label: "Slots", run: slots::idle },
    Attract { label: "Keno", run: keno::idle },
    Attract { label: "Bingo", run: bingo::idle },
    Attract { label: "Plinko", run: plinko::idle },
    Attract { label: "Mines", run: mines::idle },
    Attract { label: "Crash", run: crash::idle },
    Attract { label: "Scratch Cards", run: scratch::idle },
    Attract { label: "Horse Racing", run: horses::idle },
];

/// How long each table holds the floor before the cycler moves on.
const SLOT: std::time::Duration = std::time::Duration::from_secs(24);

/// A title card between tables, so it is always clear what is being shown.
fn announce(screen: &mut Screen, label: &str, position: usize, total: usize) {
    let theme = screen.theme;
    screen.begin();
    screen.blank();
    screen.blank();
    for line in widgets::banner(&theme, &label.to_uppercase(), true) {
        screen.line(&format!("    {line}"));
    }
    screen.blank();
    screen.line(&theme.dim(&format!("    table {position} of {total} on the floor")));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('q', "back to the floor")]));
    screen.present();
}

/// Walks every self-running table in turn, giving each a slot, until the
/// watcher asks to leave. This is the whole casino running itself.
pub fn cycle(ctx: &mut Ctx) {
    loop {
        for (i, t) in ATTRACT.iter().enumerate() {
            announce(ctx.screen, t.label, i + 1, ATTRACT.len());
            if table::idle_hold(1_600) && table::idle_left() {
                return;
            }
            table::idle_budget(Some(SLOT));
            (t.run)(ctx);
            if table::idle_left() || ui::quit_requested() {
                return;
            }
        }
    }
}

/// Opens one table's attract mode on its own, with no time limit — it
/// runs until the watcher leaves.
pub fn attract_one(ctx: &mut Ctx, which: usize) {
    let Some(t) = ATTRACT.get(which) else { return };
    table::idle_budget(None);
    (t.run)(ctx);
}
