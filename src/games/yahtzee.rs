//! Yahtzee — full 13-category scorecard, hot-seat multiplayer and AI opponents.

use super::{Ctx, Difficulty, Player};
use crate::dice;
use crate::ui::{self, *};

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
        6 => if t.iter().any(|c| *c >= 3) { sum } else { 0 },
        7 => if t.iter().any(|c| *c >= 4) { sum } else { 0 },
        8 => {
            let has3 = t.iter().any(|c| *c == 3);
            let has2 = t.iter().any(|c| *c == 2);
            let has5 = t.iter().any(|c| *c == 5);
            if (has3 && has2) || has5 { 25 } else { 0 }
        }
        9 => if longest_run(&t) >= 4 { 30 } else { 0 },
        10 => if longest_run(&t) >= 5 { 40 } else { 0 },
        11 => if t.iter().any(|c| *c == 5) { 50 } else { 0 },
        12 => sum,
        _ => 0,
    }
}

fn longest_run(tally: &[u32]) -> u32 {
    let mut best = 0;
    let mut run = 0;
    for face in 1..=6usize {
        if tally[face] > 0 {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

pub fn play(ctx: &mut Ctx, players: Vec<Player>) {
    ui::header(ctx.colors, "YAHTZEE");
    println!("  13 rounds. Three rolls a turn — keep the dice you like, reroll the rest.");

    let mut cards: Vec<Card> = players.iter().map(|_| Card::new()).collect();

    for round in 1..=13u32 {
        for (i, p) in players.iter().enumerate() {
            ui::header(ctx.colors, &format!("Round {round}/13 — {}", p.name));
            let d = roll_phase(ctx, p, &cards[i]);
            let Some(d) = d else { return };
            let cat = choose_category(ctx, p, &cards[i], &d);
            let Some(cat) = cat else { return };

            // Joker/bonus rule: an extra Yahtzee after a scored 50 is worth 100 more.
            if score_for(11, &d) == 50 && cards[i].slots[11] == Some(50) {
                cards[i].bonus_yahtzees += 1;
                println!("  {}", color(ctx.colors, GREEN, "BONUS YAHTZEE! +100"));
            }
            let pts = score_for(cat, &d);
            cards[i].slots[cat] = Some(pts);
            println!(
                "  {} → {} in {}",
                color(ctx.colors, BOLD, &format!("{pts} pts")),
                CATEGORIES[cat],
                color(ctx.colors, DIM, "scorecard")
            );
            print_card(ctx.colors, &p.name, &cards[i]);
        }
    }

    println!();
    ui::header(ctx.colors, "FINAL SCORES");
    let mut ranked: Vec<(usize, u32)> = cards.iter().enumerate().map(|(i, c)| (i, c.total())).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    for (place, (i, total)) in ranked.iter().enumerate() {
        let medal = ["🥇", "🥈", "🥉"].get(place).copied().unwrap_or("  ");
        println!("  {medal} {:<14} {}", players[*i].name, color(ctx.colors, BOLD, &total.to_string()));
    }
    let (best_i, best) = ranked[0];
    println!();
    println!("  {}", color(ctx.colors, GREEN, &format!("★ {} wins with {} ★", players[best_i].name, best)));

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
    ui::pause();
}

/// Rolls up to three times, letting the player (or AI) hold dice. `None` = quit.
fn roll_phase(ctx: &mut Ctx, p: &Player, card: &Card) -> Option<Vec<u32>> {
    let mut d = dice::roll_n(ctx.rng, 5, 6);
    let mut held = [false; 5];

    for roll_no in 1..=2 {
        ui::draw_dice(&d, Some(&held), ctx.colors);
        if let Some(diff) = p.ai {
            held = ai_hold(diff, &d, card, ctx);
            let kept: Vec<String> = d.iter().zip(held.iter()).filter(|(_, h)| **h).map(|(v, _)| v.to_string()).collect();
            println!(
                "  {} keeps [{}]",
                color(ctx.colors, MAGENTA, "AI"),
                kept.join(" ")
            );
        } else {
            let ans = ui::prompt(&format!(
                "  roll {roll_no}/3 — dice to keep (e.g. 135), (s)tand, (q)uit: "
            ));
            let low = ans.to_lowercase();
            if low.starts_with('q') {
                return None;
            }
            if low.starts_with('s') {
                return Some(d);
            }
            held = [false; 5];
            for ch in low.chars().filter(|c| c.is_ascii_digit()) {
                let i = ch.to_digit(10).unwrap() as usize;
                if (1..=5).contains(&i) {
                    held[i - 1] = true;
                }
            }
            if held.iter().all(|h| *h) {
                return Some(d);
            }
        }
        for i in 0..5 {
            if !held[i] {
                d[i] = ctx.rng.roll(6);
            }
        }
        ctx.store.bump("yahtzee.rolls", 1);
    }
    ui::draw_dice(&d, None, ctx.colors);
    Some(d)
}

/// Greedy keep policy: chase the face that already appears most, but keep a
/// made straight when the scorecard still needs one.
fn ai_hold(diff: Difficulty, d: &[u32], card: &Card, ctx: &mut Ctx) -> [bool; 5] {
    let mut held = [false; 5];
    if diff == Difficulty::Easy && ctx.rng.below(3) == 0 {
        return held; // sometimes rerolls everything
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

/// Picks where to score. `None` = quit.
fn choose_category(ctx: &mut Ctx, p: &Player, card: &Card, d: &[u32]) -> Option<usize> {
    let open: Vec<usize> = (0..13).filter(|i| card.slots[*i].is_none()).collect();
    if p.is_ai() {
        let diff = p.ai.unwrap();
        let pick = open
            .iter()
            .copied()
            .max_by_key(|c| {
                let s = score_for(*c, d) as i64;
                // Value upper-section progress toward the 63-point bonus.
                let bonus_pull = if *c < 6 && diff == Difficulty::Hard { s - (*c as i64 + 1) * 3 } else { 0 };
                let dump_penalty = if s == 0 { -(*c as i64) } else { 0 };
                s * 10 + bonus_pull + dump_penalty
            })
            .unwrap_or(open[0]);
        println!("  {} scores in {}", color(ctx.colors, MAGENTA, "AI"), CATEGORIES[pick]);
        return Some(pick);
    }

    println!();
    println!("  {}", color(ctx.colors, BOLD, "open categories:"));
    for c in &open {
        println!(
            "   {:>2}. {:<17} {}",
            c + 1,
            CATEGORIES[*c],
            color(ctx.colors, if score_for(*c, d) > 0 { GREEN } else { DIM }, &format!("{} pts", score_for(*c, d)))
        );
    }
    loop {
        let ans = ui::prompt("  score in # (or q to quit): ");
        if ans.to_lowercase().starts_with('q') {
            return None;
        }
        match ans.parse::<usize>() {
            Ok(n) if n >= 1 && n <= 13 && open.contains(&(n - 1)) => return Some(n - 1),
            _ => println!("  ! pick an open category number"),
        }
    }
}

pub fn print_card(colors: bool, name: &str, card: &Card) {
    println!();
    println!("  {}", color(colors, BOLD, &format!("── {name}'s card ──")));
    for i in 0..13 {
        let val = card.slots[i].map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        if i == 6 {
            println!(
                "   {:<18} {:>4}   {}",
                "Upper subtotal",
                card.upper_subtotal(),
                color(colors, DIM, &format!("bonus {}", card.upper_bonus()))
            );
        }
        println!("   {:<18} {:>4}", CATEGORIES[i], val);
    }
    if card.bonus_yahtzees > 0 {
        println!("   {:<18} {:>4}", "Yahtzee bonus", card.bonus_yahtzees * 100);
    }
    println!("   {}{:<18} {:>4}{}", if colors { BOLD } else { "" }, "TOTAL", card.total(), if colors { RESET } else { "" });
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
