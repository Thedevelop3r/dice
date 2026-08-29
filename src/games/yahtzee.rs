//! Yahtzee — full 13-category scorecard, hot-seat multiplayer and AI opponents.

use super::{Ctx, Difficulty, Player};
use crate::dice;
use crate::ui::{self, dice_art, widgets, Screen};

pub const CATEGORIES: [&str; 13] = [
    "Ones", "Twos", "Threes", "Fours", "Fives", "Sixes",
    "Three of a Kind", "Four of a Kind", "Full House",
    "Small Straight", "Large Straight", "Yahtzee", "Chance",
];

pub struct Card {
    pub slots: [Option<u32>; 13],
    pub bonus_yahtzees: u32,
}

impl Card {
    pub fn new() -> Card {
        Card { slots: [None; 13], bonus_yahtzees: 0 }
    }

    pub fn upper_subtotal(&self) -> u32 {
        self.slots[..6].iter().flatten().sum()
    }

    pub fn upper_bonus(&self) -> u32 {
        if self.upper_subtotal() >= 63 { 35 } else { 0 }
    }

    pub fn lower_subtotal(&self) -> u32 {
        self.slots[6..].iter().flatten().sum::<u32>() + self.bonus_yahtzees * 100
    }

    pub fn total(&self) -> u32 {
        self.upper_subtotal() + self.upper_bonus() + self.lower_subtotal()
    }
}

/// Points `dice` would earn in category `cat` (0..13).
pub fn score_for(cat: usize, d: &[u32]) -> u32 {
    let t = dice::tally(d, 6);
    let sum: u32 = d.iter().sum();
    match cat {
        0..=5 => {
            let face = cat as u32 + 1;
            t[face as usize] * face
        }
        6 if t.iter().any(|c| *c >= 3) => sum,
        7 if t.iter().any(|c| *c >= 4) => sum,
        8 => {
            let has3 = t.contains(&3);
            let has2 = t.contains(&2);
            let has5 = t.contains(&5);
            if (has3 && has2) || has5 { 25 } else { 0 }
        }
        9 if longest_run(&t) >= 4 => 30,
        10 if longest_run(&t) >= 5 => 40,
        11 if t.contains(&5) => 50,
        12 => sum,
        _ => 0,
    }
}

fn longest_run(tally: &[u32]) -> u32 {
    let mut best = 0;
    let mut run = 0;
    for &count in &tally[1..=6] {
        if count > 0 {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

pub fn play(ctx: &mut Ctx, players: Vec<Player>) {
    let mut cards: Vec<Card> = players.iter().map(|_| Card::new()).collect();

    for round in 1..=13u32 {
        for (i, p) in players.iter().enumerate() {
            let d = roll_phase(ctx, &players, p, round, &cards[i]);
            let Some(d) = d else { return };
            let cat = choose_category(ctx, p, &cards[i], &d);
            let Some(cat) = cat else { return };

            // Joker/bonus rule: an extra Yahtzee after a scored 50 is worth 100 more.
            let bonus = score_for(11, &d) == 50 && cards[i].slots[11] == Some(50);
            if bonus {
                cards[i].bonus_yahtzees += 1;
            }
            let pts = score_for(cat, &d);
            cards[i].slots[cat] = Some(pts);
            show_scored(ctx, p, &cards[i], cat, pts, bonus);
        }
    }

    final_scores(ctx, &players, &cards);
}

fn hud(screen: &mut Screen, _players: &[Player], round: u32, p: &Player) {
    ui::header(screen, &format!("YAHTZEE — round {round}/13 — {}'s turn", p.name));
}

/// Rolls up to three times, letting the player (or AI) hold dice between
/// rolls. Held dice sit still through the animation; the rest tumble.
/// `None` = quit.
fn roll_phase(ctx: &mut Ctx, players: &[Player], p: &Player, round: u32, card: &Card) -> Option<Vec<u32>> {
    let theme = ctx.theme();
    let seed = vec![1u32; 5];
    let all_open = [false; 5];
    let mut d = dice_art::animate_roll(ctx.screen, ctx.rng, 6, &seed, &all_open, |screen, values| {
        screen.begin();
        hud(screen, players, round, p);
        screen.blank();
        for line in dice_art::dice_block(&theme, values, Some(&[false; 5][..])) {
            screen.line(&line);
        }
        screen.blank();
        screen.line(&theme.dim("rolling..."));
    });
    let mut held = [false; 5];
    ctx.store.bump("yahtzee.rolls", 1);

    for roll_no in 1..=2 {
        if let Some(diff) = p.ai {
            ui::sleep_ms(500);
            held = ai_hold(diff, &d, card, ctx);
            draw_hold_screen(ctx, players, &HoldView { round, player: p, dice: &d, held: &held, roll_no, ai_turn: true });
            ctx.screen.present();
            ui::sleep_ms(600);
        } else {
            loop {
                draw_hold_screen(ctx, players, &HoldView { round, player: p, dice: &d, held: &held, roll_no, ai_turn: false });
                ctx.screen.present();
                match ui::read_key() {
                    ui::Key::Char(c) if ('1'..='5').contains(&c) => {
                        let i = c.to_digit(10).unwrap() as usize - 1;
                        held[i] = !held[i];
                    }
                    ui::Key::Char('r') => break,
                    ui::Key::Char('s') | ui::Key::Enter => return Some(d),
                    ui::Key::Quit | ui::Key::Char('q') => return None,
                    _ => {}
                }
            }
        }

        let theme = ctx.theme();
        let held_snapshot = held;
        d = dice_art::animate_roll(ctx.screen, ctx.rng, 6, &d, &held_snapshot, |screen, values| {
            screen.begin();
            hud(screen, players, round, p);
            screen.blank();
            for line in dice_art::dice_block(&theme, values, Some(&held_snapshot[..])) {
                screen.line(&line);
            }
            screen.blank();
            screen.line(&theme.dim("rolling..."));
        });
        ctx.store.bump("yahtzee.rolls", 1);
    }

    ctx.screen.begin();
    hud(ctx.screen, players, round, p);
    ctx.screen.blank();
    for line in dice_art::dice_block(&ctx.theme(), &d, None) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&ctx.theme().dim("final roll — choose a category"));
    ctx.screen.present();
    ui::sleep_ms(if p.is_ai() { 500 } else { 0 });
    Some(d)
}

/// Everything `draw_hold_screen` needs beyond `ctx`/`players`, bundled so
/// the function doesn't carry an unwieldy argument list.
struct HoldView<'a> {
    round: u32,
    player: &'a Player,
    dice: &'a [u32],
    held: &'a [bool; 5],
    roll_no: u32,
    ai_turn: bool,
}

fn draw_hold_screen(ctx: &mut Ctx, players: &[Player], v: &HoldView) {
    let theme = ctx.theme();
    ctx.screen.begin();
    hud(ctx.screen, players, v.round, v.player);
    ctx.screen.blank();
    for line in dice_art::dice_block(&theme, v.dice, Some(&v.held[..])) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("roll {}/3 · {} rerolls left", v.roll_no, 3 - v.roll_no)));
    if v.ai_turn {
        ctx.screen.line(&theme.dim("the house AI is deciding..."));
    } else {
        ctx.screen.line(&widgets::footer(
            &theme,
            &[('1', "toggle die 1"), ('…', ""), ('5', "toggle die 5"), ('r', "reroll"), ('s', "stand"), ('q', "quit")],
        ));
    }
}

/// Greedy keep policy: chase the face that already appears most, but keep a
/// made straight when the scorecard still needs one.
fn ai_hold(diff: Difficulty, d: &[u32], card: &Card, ctx: &mut Ctx) -> [bool; 5] {
    let mut held = [false; 5];
    if diff == Difficulty::Easy && ctx.rng.below(3) == 0 {
        return held;
    }
    let t = dice::tally(d, 6);
    let straight_open = card.slots[9].is_none() || card.slots[10].is_none();
    let run = longest_run(&t);
    if diff != Difficulty::Easy && straight_open && run >= 3 && t.iter().skip(1).all(|c| *c <= 1) {
        for (i, v) in d.iter().enumerate() {
            held[i] = t[*v as usize] == 1;
        }
        return held;
    }
    let best_face = (1..=6usize)
        .max_by_key(|f| (t[*f], if diff == Difficulty::Hard { *f } else { 0 }))
        .unwrap_or(6);
    for (i, v) in d.iter().enumerate() {
        held[i] = *v as usize == best_face;
    }
    held
}

/// Picks where to score. `None` = quit. Open categories are offered on
/// letter keys a..m so the pick lands the instant it's pressed.
fn choose_category(ctx: &mut Ctx, p: &Player, card: &Card, d: &[u32]) -> Option<usize> {
    let open: Vec<usize> = (0..13).filter(|i| card.slots[*i].is_none()).collect();
    if p.is_ai() {
        let diff = p.ai.unwrap();
        let pick = open
            .iter()
            .copied()
            .max_by_key(|c| {
                let s = score_for(*c, d) as i64;
                let bonus_pull = if *c < 6 && diff == Difficulty::Hard { s - (*c as i64 + 1) * 3 } else { 0 };
                let dump_penalty = if s == 0 { -(*c as i64) } else { 0 };
                s * 10 + bonus_pull + dump_penalty
            })
            .unwrap_or(open[0]);
        ui::sleep_ms(400);
        return Some(pick);
    }

    let theme = ctx.theme();
    loop {
        ctx.screen.begin();
        ui::header(ctx.screen, "SCORE WHERE?");
        ctx.screen.blank();
        for line in dice_art::dice_block(&theme, d, None) {
            ctx.screen.line(&line);
        }
        ctx.screen.blank();
        let mut valid = Vec::new();
        for (i, cat) in open.iter().enumerate() {
            let key = (b'a' + i as u8) as char;
            valid.push(key);
            let pts = score_for(*cat, d);
            ctx.screen.line(&format!(
                "  {} {:<17} {}",
                theme.paint(ui::theme::GOLD, &format!("[{}]", key.to_ascii_uppercase())),
                CATEGORIES[*cat],
                theme.paint(if pts > 0 { ui::theme::GREEN } else { ui::theme::DIM }, &format!("{pts} pts"))
            ));
        }
        ctx.screen.blank();
        ctx.screen.line(&theme.dim("press a letter to score there · q to quit"));
        ctx.screen.present();
        valid.push('q');
        match ui::choose_key(&valid, 'q') {
            Some('q') | None => return None,
            Some(c) => {
                let idx = (c as u8 - b'a') as usize;
                if let Some(cat) = open.get(idx) {
                    return Some(*cat);
                }
            }
        }
    }
}

fn show_scored(ctx: &mut Ctx, p: &Player, card: &Card, cat: usize, pts: u32, bonus: bool) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, &format!("{}'s card", p.name));
    ctx.screen.blank();
    if bonus {
        ctx.screen.line(&theme.win("BONUS YAHTZEE! +100"));
        ctx.screen.blank();
    }
    ctx.screen.line(&format!("  {} → {}", theme.bold(&format!("{pts} pts")), CATEGORIES[cat]));
    ctx.screen.blank();
    print_card(ctx.screen, card);
    ctx.screen.present();
    if p.is_ai() {
        ui::sleep_ms(700);
    } else {
        ui::pause(ctx.screen);
    }
}

fn print_card(screen: &mut Screen, card: &Card) {
    let theme = screen.theme;
    for (i, &name) in CATEGORIES.iter().enumerate() {
        let val = card.slots[i].map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        if i == 6 {
            screen.line(&format!(
                "   {:<18} {:>4}   {}",
                "Upper subtotal",
                card.upper_subtotal(),
                theme.dim(&format!("bonus {}", card.upper_bonus()))
            ));
        }
        screen.line(&format!("   {:<18} {:>4}", name, val));
    }
    if card.bonus_yahtzees > 0 {
        screen.line(&format!("   {:<18} {:>4}", "Yahtzee bonus", card.bonus_yahtzees * 100));
    }
    screen.line(&format!("   {}{:<18} {:>4}{}", if theme.colors { ui::theme::BOLD } else { "" }, "TOTAL", card.total(), if theme.colors { ui::theme::RESET } else { "" }));
}

fn final_scores(ctx: &mut Ctx, players: &[Player], cards: &[Card]) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "FINAL SCORES");
    let mut ranked: Vec<(usize, u32)> = cards.iter().enumerate().map(|(i, c)| (i, c.total())).collect();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
    ctx.screen.blank();
    for (place, (i, total)) in ranked.iter().enumerate() {
        let medal = ["🥇", "🥈", "🥉"].get(place).copied().unwrap_or("  ");
        ctx.screen.line(&format!("  {medal} {:<16} {}", players[*i].name, theme.bold(&total.to_string())));
    }
    let (best_i, best) = ranked[0];
    ctx.screen.blank();
    for line in widgets::banner(&theme, &format!("{} WINS WITH {}", players[best_i].name, best), true) {
        ctx.screen.line(&format!("  {line}"));
    }

    ctx.store.bump("yahtzee.games", 1);
    if !players[best_i].is_ai() {
        ctx.store.bump("yahtzee.wins", 1);
    }
    for (i, c) in cards.iter().enumerate() {
        if !players[i].is_ai() {
            ctx.store.record_best("yahtzee.best", c.total() as i64);
        }
    }
    let _ = ctx.store.save();
    ui::pause(ctx.screen);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_section_counts_only_its_face() {
        assert_eq!(score_for(2, &[3, 3, 3, 1, 6]), 9); // Threes
        assert_eq!(score_for(5, &[3, 3, 3, 1, 6]), 6); // Sixes
    }

    #[test]
    fn combination_categories() {
        assert_eq!(score_for(6, &[4, 4, 4, 2, 1]), 15); // three of a kind = sum
        assert_eq!(score_for(7, &[4, 4, 4, 2, 1]), 0);
        assert_eq!(score_for(8, &[2, 2, 5, 5, 5]), 25); // full house
        assert_eq!(score_for(8, &[2, 2, 2, 5, 6]), 0);
        assert_eq!(score_for(9, &[1, 2, 3, 4, 4]), 30); // small straight
        assert_eq!(score_for(10, &[2, 3, 4, 5, 6]), 40); // large straight
        assert_eq!(score_for(10, &[1, 2, 3, 4, 4]), 0);
        assert_eq!(score_for(11, &[6, 6, 6, 6, 6]), 50); // yahtzee
        assert_eq!(score_for(12, &[1, 2, 3, 4, 5]), 15); // chance
    }

    #[test]
    fn five_of_a_kind_is_also_a_full_house() {
        assert_eq!(score_for(8, &[4, 4, 4, 4, 4]), 25);
    }

    #[test]
    fn upper_bonus_applies_at_63() {
        let mut c = Card::new();
        for i in 0..6 {
            c.slots[i] = Some((i as u32 + 1) * 3); // exactly 63
        }
        assert_eq!(c.upper_subtotal(), 63);
        assert_eq!(c.upper_bonus(), 35);
        c.slots[0] = Some(0);
        assert_eq!(c.upper_bonus(), 0);
    }
}
