//! Dice rendering — pip faces, table layouts, and the roll animation.
//!
//! Per the house rules: any time dice settle, they visibly tumble first.
//! `animate_roll` redraws the whole frame ten times over roughly a second
//! (front-loaded fast, easing to a stop) before landing on the true,
//! already-determined result on its last frame.

use super::theme::Theme;
use super::{sleep_ms, Screen};
use crate::rng::Rng;

use super::theme::{GOLD, GREEN, YELLOW};

/// Ten frames, decelerating from a snappy 55ms to a settling 175ms —
/// sums to ~1.1s of tumble before the dice land.
const ROLL_FRAME_DELAYS: [u64; 10] = [55, 65, 75, 90, 100, 115, 130, 145, 160, 175];

fn face_lines(theme: &Theme, v: u32, code: &str) -> [String; 5] {
    let pips: [[&str; 3]; 6] = [
        ["     ", "  ●  ", "     "],
        ["●    ", "     ", "    ●"],
        ["●    ", "  ●  ", "    ●"],
        ["●   ●", "     ", "●   ●"],
        ["●   ●", "  ●  ", "●   ●"],
        ["●   ●", "●   ●", "●   ●"],
    ];
    let body = if (1..=6).contains(&v) {
        let p = pips[(v - 1) as usize];
        [
            "┌───────┐".to_string(),
            format!("│ {} │", p[0]),
            format!("│ {} │", p[1]),
            format!("│ {} │", p[2]),
            "└───────┘".to_string(),
        ]
    } else {
        [
            "┌───────┐".to_string(),
            "│       │".to_string(),
            format!("│{:^7}│", v),
            "│       │".to_string(),
            "└───────┘".to_string(),
        ]
    };
    body.map(|line| theme.paint(code, &line))
}

/// Renders `values` side by side. `held` (Yahtzee's kept dice) highlights
/// in gold with a HELD tag; everything else uses the base die color.
pub fn dice_block(theme: &Theme, values: &[u32], held: Option<&[bool]>) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    let faces: Vec<[String; 5]> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let is_held = held.is_some_and(|h| h.get(i).copied().unwrap_or(false));
            face_lines(theme, *v, if is_held { GOLD } else { YELLOW })
        })
        .collect();
    let mut out = Vec::with_capacity(6);
    for row in 0..5 {
        let mut line = String::new();
        for f in &faces {
            line.push_str(&f[row]);
            line.push(' ');
        }
        out.push(line);
    }
    if let Some(h) = held {
        let mut labels = String::new();
        for (i, _) in faces.iter().enumerate() {
            let is_held = h.get(i).copied().unwrap_or(false);
            let tag = if is_held { format!("[{}] HELD", i + 1) } else { format!("[{}]     ", i + 1) };
            labels.push_str(&format!("{:<10}", tag));
        }
        out.push(theme.dim(&labels));
    }
    out
}

/// A labelled multi-die table — Luck Bet's eight, Ultra Casino Dice's
/// twelve, or any other lettered spread — winners lit green. `per_row`
/// wraps the dice onto additional lines once there are more of them than
/// comfortably fit on one row: Luck Bet passes its full `DICE` count to
/// keep its original single-row look, Ultra Casino Dice wraps at 6 so
/// twelve dice stay inside a normal terminal width.
pub fn table_block(theme: &Theme, values: &[u32], labels: &[char], hits: &[bool], gold: bool, per_row: usize) -> Vec<String> {
    let base = if gold { GOLD } else { super::theme::CYAN };
    let per_row = per_row.max(1);
    let mut out = Vec::new();
    for chunk_start in (0..values.len()).step_by(per_row) {
        let end = (chunk_start + per_row).min(values.len());
        let faces: Vec<[String; 5]> = values[chunk_start..end]
            .iter()
            .enumerate()
            .map(|(j, v)| {
                let i = chunk_start + j;
                let code = if hits.get(i).copied().unwrap_or(false) { GREEN } else { base };
                face_lines(theme, *v, code)
            })
            .collect();
        for row in 0..5 {
            let mut line = String::new();
            for f in &faces {
                line.push_str(&f[row]);
                line.push(' ');
            }
            out.push(line);
        }
        let mut tags = String::new();
        for j in 0..faces.len() {
            let i = chunk_start + j;
            let letter = labels.get(i).copied().unwrap_or('?');
            let hit = hits.get(i).copied().unwrap_or(false);
            let cell = format!("{:^9}", letter);
            tags.push_str(&theme.paint(if hit { GREEN } else { super::theme::BOLD }, &cell));
            tags.push(' ');
        }
        out.push(tags);
    }
    out
}

/// Rolls `values.len()` dice, redrawing the whole frame ten times over
/// about a second before settling on the real result. Positions marked
/// `true` in `held` sit still throughout (Yahtzee's kept dice); the rest
/// tumble. `draw_frame` is handed the in-progress values each frame and
/// must render the *entire* screen (header, HUD, dice, footer — whatever
/// belongs on that screen) since every frame starts from a hard clear.
pub fn animate_roll<F: FnMut(&mut Screen, &[u32])>(
    screen: &mut Screen,
    rng: &mut Rng,
    sides: u32,
    values: &[u32],
    held: &[bool],
    mut draw_frame: F,
) -> Vec<u32> {
    let n = values.len();
    let is_held = |i: usize| held.get(i).copied().unwrap_or(false);
    let roll_one = |rng: &mut Rng, i: usize, cur: u32| if is_held(i) { cur } else { rng.roll(sides) };

    let final_values: Vec<u32> = (0..n).map(|i| roll_one(rng, i, values[i])).collect();
    let last = ROLL_FRAME_DELAYS.len() - 1;
    for (frame_i, &delay) in ROLL_FRAME_DELAYS.iter().enumerate() {
        let frame_values: Vec<u32> = if frame_i == last {
            final_values.clone()
        } else {
            (0..n).map(|i| roll_one(rng, i, values[i])).collect()
        };
        draw_frame(screen, &frame_values);
        screen.present();
        sleep_ms(delay);
    }
    final_values
}

/// 60 frames at 50ms (~3s) — the Turbo table's signature fast, sustained
/// spin, distinct from the standard one-second settle.
pub const TURBO_FRAME_DELAYS: [u64; 60] = [50; 60];

/// 140 frames at 50ms — a full 7 seconds at 20 frames a second, Ultra
/// Casino Dice's signature long, sustained tumble before the whole table
/// settles at once.
pub const ULTRA_FRAME_DELAYS: [u64; 140] = [50; 140];

/// Rolls a whole labelled table of `dice_count` dice at once — every die
/// tumbles every frame, unlike `animate_roll`'s per-die holds. `delays`
/// controls the frame count/pacing: pass `&ROLL_FRAME_DELAYS` (via
/// `standard_frames()`) for the normal one-second settle, `&TURBO_FRAME_DELAYS`
/// for Luck Bet Turbo's sustained spin, or `&ULTRA_FRAME_DELAYS` for Ultra
/// Casino Dice's full seven-second tumble.
pub fn animate_table_roll<F: FnMut(&mut Screen, &[u32])>(
    screen: &mut Screen,
    rng: &mut Rng,
    dice_count: usize,
    delays: &[u64],
    mut draw_frame: F,
) -> Vec<u32> {
    let mut final_values = vec![0u32; dice_count];
    for v in final_values.iter_mut() {
        *v = rng.roll(6);
    }
    let last = delays.len().saturating_sub(1);
    for (frame_i, &delay) in delays.iter().enumerate() {
        let frame: Vec<u32> = if frame_i == last {
            final_values.clone()
        } else {
            (0..dice_count).map(|_| rng.roll(6)).collect()
        };
        draw_frame(screen, &frame);
        screen.present();
        sleep_ms(delay);
    }
    final_values
}

pub fn standard_frames() -> &'static [u64] {
    &ROLL_FRAME_DELAYS
}
