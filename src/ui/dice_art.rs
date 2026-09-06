//! Dice rendering — pip faces, table layouts, and the roll animation.
//!
//! Per the house rules: any time dice settle, they visibly tumble first.
//! `animate_roll` redraws the whole frame ten times over roughly a second
//! (front-loaded fast, easing to a stop) before landing on the true,
//! already-determined result on its last frame.
//!
//! Faces draw at the largest of three sizes that fits the live terminal.
//! `Scale::Big` is exactly three times `Scale::Small` on both axes —
//! 27x15 against the original 9x5 — with `Scale::Mid` as the step
//! between. Callers never pick a size: every block function measures the
//! terminal and takes the biggest that fits, so Chuck-a-Luck's three dice
//! render huge while Ultra Casino Dice's twelve fall back gracefully on
//! the very same screen.

use super::theme::Theme;
use super::Scale;
use super::{sleep_ms, Screen};
use crate::rng::Rng;

use super::theme::{GOLD, GREEN, YELLOW};

/// Ten frames, decelerating from a snappy 55ms to a settling 175ms —
/// sums to ~1.1s of tumble before the dice land.
const ROLL_FRAME_DELAYS: [u64; 10] = [55, 65, 75, 90, 100, 115, 130, 145, 160, 175];

/// The 3x3 pip grid for each face, as a bitmask over positions
/// `row * 3 + col`. These are the standard die layouts — the same ones
/// the app has always drawn, just expressed once instead of per size.
const PIP_MASKS: [u16; 6] = [
    1 << 4,                                                     // 1: center
    (1 << 0) | (1 << 8),                                        // 2: the long diagonal
    (1 << 0) | (1 << 4) | (1 << 8),                             // 3
    (1 << 0) | (1 << 2) | (1 << 6) | (1 << 8),                  // 4: corners
    (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 8),       // 5
    (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 8), // 6
];

/// `(pad_w, pip_w, gap_w, pad_h, pip_h, gap_h)` around the 3x3 pip grid.
/// A die's width is `2 + 2*pad_w + 3*pip_w + 2*gap_w`, and its height the
/// same sum in the other axis — the `geometry_matches_declared_size` test
/// holds these to account against `Scale`'s declared sizes, so the two can
/// never quietly drift apart.
const fn metrics(scale: Scale) -> (usize, usize, usize, usize, usize, usize) {
    match scale {
        Scale::Small => (1, 1, 1, 0, 1, 0),
        Scale::Mid => (1, 4, 1, 0, 2, 1),
        Scale::Big => (1, 7, 1, 1, 3, 1),
    }
}

/// The biggest die size that fits `per_row` dice across and `rows` rows of
/// them, leaving `reserved` terminal rows for the surrounding chrome.
pub fn fit(per_row: usize, rows: usize, reserved: usize) -> Scale {
    super::fit_scale(per_row, rows, reserved, Scale::die_width)
}

/// One pip, as `pip_h` rows of `pip_w` columns. At the smallest size a
/// pip is the original single `●`; larger pips are filled blocks with
/// their corners shaved off so they read round rather than square.
fn pip_rows(pip_w: usize, pip_h: usize) -> Vec<String> {
    (0..pip_h)
        .map(|r| {
            if pip_w == 1 {
                "●".to_string()
            } else if pip_w >= 5 && pip_h >= 3 && (r == 0 || r == pip_h - 1) {
                format!(" {} ", "█".repeat(pip_w - 2))
            } else {
                "█".repeat(pip_w)
            }
        })
        .collect()
}

fn face_lines(theme: &Theme, v: u32, code: &str, scale: Scale) -> Vec<String> {
    let (pad_w, pip_w, gap_w, pad_h, pip_h, gap_h) = metrics(scale);
    let inner_w = scale.die_width() - 2;
    let inner_h = scale.height() - 2;
    let mut out = Vec::with_capacity(scale.height());
    let mut buf = String::with_capacity(scale.die_width() * 4);

    let edge = |buf: &mut String, left: char, right: char| {
        buf.clear();
        buf.push(left);
        for _ in 0..inner_w {
            buf.push('─');
        }
        buf.push(right);
    };

    edge(&mut buf, '┌', '┐');
    out.push(theme.paint(code, &buf));

    if (1..=6).contains(&v) {
        let mask = PIP_MASKS[(v - 1) as usize];
        let pip = pip_rows(pip_w, pip_h);
        let period = pip_h + gap_h;
        for r in 0..inner_h {
            buf.clear();
            buf.push('│');
            // Which row of which pip row-band, if any, this line falls in.
            let band = r.checked_sub(pad_h).and_then(|y| {
                let grid_row = y / period;
                let within = y % period;
                (grid_row < 3 && within < pip_h).then_some((grid_row, within))
            });
            match band {
                Some((grid_row, within)) => {
                    for _ in 0..pad_w {
                        buf.push(' ');
                    }
                    for col in 0..3 {
                        if col > 0 {
                            for _ in 0..gap_w {
                                buf.push(' ');
                            }
                        }
                        if mask & (1 << (grid_row * 3 + col)) != 0 {
                            buf.push_str(&pip[within]);
                        } else {
                            for _ in 0..pip_w {
                                buf.push(' ');
                            }
                        }
                    }
                    for _ in 0..pad_w {
                        buf.push(' ');
                    }
                }
                None => {
                    for _ in 0..inner_w {
                        buf.push(' ');
                    }
                }
            }
            buf.push('│');
            out.push(theme.paint(code, &buf));
        }
    } else {
        // A face outside 1-6 — the Dice Lab's d20s and friends. There are
        // no pips for those, so the value is simply centered.
        let label = v.to_string();
        let mid = inner_h / 2;
        for r in 0..inner_h {
            buf.clear();
            buf.push('│');
            if r == mid {
                let pad = inner_w.saturating_sub(label.chars().count());
                let left = pad / 2;
                for _ in 0..left {
                    buf.push(' ');
                }
                buf.push_str(&label);
                for _ in 0..pad - left {
                    buf.push(' ');
                }
            } else {
                for _ in 0..inner_w {
                    buf.push(' ');
                }
            }
            buf.push('│');
            out.push(theme.paint(code, &buf));
        }
    }

    edge(&mut buf, '└', '┘');
    out.push(theme.paint(code, &buf));
    out
}

/// Renders `values` side by side at an explicit size. `held` (Yahtzee's
/// kept dice) highlights in gold with a HELD tag; everything else uses
/// the base die color.
pub fn dice_block_scaled(theme: &Theme, values: &[u32], held: Option<&[bool]>, scale: Scale) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    let faces: Vec<Vec<String>> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let is_held = held.is_some_and(|h| h.get(i).copied().unwrap_or(false));
            face_lines(theme, *v, if is_held { GOLD } else { YELLOW }, scale)
        })
        .collect();
    let mut out = Vec::with_capacity(scale.height() + 1);
    for row in 0..scale.height() {
        let mut line = String::with_capacity(faces.len() * (scale.die_width() + 12));
        for f in &faces {
            line.push_str(&f[row]);
            line.push(' ');
        }
        out.push(line);
    }
    if let Some(h) = held {
        let cell = scale.die_width() + 1;
        let mut labels = String::with_capacity(faces.len() * cell);
        for i in 0..faces.len() {
            let is_held = h.get(i).copied().unwrap_or(false);
            let tag = if is_held { format!("[{}] HELD", i + 1) } else { format!("[{}]", i + 1) };
            labels.push_str(&format!("{tag:<cell$}"));
        }
        out.push(theme.dim(&labels));
    }
    out
}

/// Renders `values` side by side at the biggest size the terminal fits.
/// Yahtzee (the only caller that passes `held`) draws a tall scorecard
/// alongside its dice, so it gets a deeper allowance for chrome.
pub fn dice_block(theme: &Theme, values: &[u32], held: Option<&[bool]>) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    let reserved = if held.is_some() { 24 } else { 14 };
    dice_block_scaled(theme, values, held, fit(values.len(), 1, reserved))
}

/// A labelled multi-die table — Luck Bet's eight, Ultra Casino Dice's
/// twelve, or any other lettered spread — winners lit green, at an
/// explicit size. `per_row` wraps the dice onto additional lines once
/// there are more of them than comfortably fit on one row.
pub fn table_block_scaled(theme: &Theme, values: &[u32], labels: &[char], hits: &[bool], gold: bool, per_row: usize, scale: Scale) -> Vec<String> {
    let base = if gold { GOLD } else { super::theme::CYAN };
    let per_row = per_row.max(1);
    let cell = scale.die_width();
    let rows = values.len().div_ceil(per_row);
    let mut out = Vec::with_capacity(rows * (scale.height() + 1));
    for chunk_start in (0..values.len()).step_by(per_row) {
        let end = (chunk_start + per_row).min(values.len());
        let faces: Vec<Vec<String>> = values[chunk_start..end]
            .iter()
            .enumerate()
            .map(|(j, v)| {
                let i = chunk_start + j;
                let code = if hits.get(i).copied().unwrap_or(false) { GREEN } else { base };
                face_lines(theme, *v, code, scale)
            })
            .collect();
        for row in 0..scale.height() {
            let mut line = String::with_capacity(faces.len() * (cell + 12));
            for f in &faces {
                line.push_str(&f[row]);
                line.push(' ');
            }
            out.push(line);
        }
        let mut tags = String::with_capacity(faces.len() * (cell + 12));
        for j in 0..faces.len() {
            let i = chunk_start + j;
            let letter = labels.get(i).copied().unwrap_or('?');
            let hit = hits.get(i).copied().unwrap_or(false);
            let text = format!("{letter:^cell$}");
            tags.push_str(&theme.paint(if hit { GREEN } else { super::theme::BOLD }, &text));
            tags.push(' ');
        }
        out.push(tags);
    }
    out
}

/// A labelled multi-die table at the biggest size the terminal fits.
/// Luck Bet passes its full `DICE` count to keep its original single-row
/// look; Ultra Casino Dice wraps at 6 so twelve dice stay inside a normal
/// terminal width.
pub fn table_block(theme: &Theme, values: &[u32], labels: &[char], hits: &[bool], gold: bool, per_row: usize) -> Vec<String> {
    let per_row = per_row.max(1);
    let rows = values.len().div_ceil(per_row).max(1);
    let scale = fit(per_row.min(values.len().max(1)), rows, 16);
    table_block_scaled(theme, values, labels, hits, gold, per_row, scale)
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

    let final_values: Vec<u32> = (0..n).map(|i| if is_held(i) { values[i] } else { rng.roll(sides) }).collect();
    let last = ROLL_FRAME_DELAYS.len() - 1;
    // One scratch frame reused across the whole animation rather than a
    // fresh allocation on each of the ten redraws.
    let mut frame = final_values.clone();
    for (frame_i, &delay) in ROLL_FRAME_DELAYS.iter().enumerate() {
        if frame_i == last {
            frame.copy_from_slice(&final_values);
        } else {
            for (i, slot) in frame.iter_mut().enumerate() {
                *slot = if is_held(i) { values[i] } else { rng.roll(sides) };
            }
        }
        draw_frame(screen, &frame);
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
    let final_values: Vec<u32> = (0..dice_count).map(|_| rng.roll(6)).collect();
    let last = delays.len().saturating_sub(1);
    let mut frame = final_values.clone();
    for (frame_i, &delay) in delays.iter().enumerate() {
        if frame_i == last {
            frame.copy_from_slice(&final_values);
        } else {
            for slot in frame.iter_mut() {
                *slot = rng.roll(6);
            }
        }
        draw_frame(screen, &frame);
        screen.present();
        sleep_ms(delay);
    }
    final_values
}

pub fn standard_frames() -> &'static [u64] {
    &ROLL_FRAME_DELAYS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Theme {
        Theme::new(false)
    }

    #[test]
    fn geometry_matches_declared_size() {
        for s in Scale::ALL {
            let (pad_w, pip_w, gap_w, pad_h, pip_h, gap_h) = metrics(s);
            assert_eq!(2 + 2 * pad_w + 3 * pip_w + 2 * gap_w, s.die_width(), "{s:?} width");
            assert_eq!(2 + 2 * pad_h + 3 * pip_h + 2 * gap_h, s.height(), "{s:?} height");
        }
    }

    #[test]
    fn big_is_exactly_three_times_small() {
        assert_eq!(Scale::Big.die_width(), Scale::Small.die_width() * 3);
        assert_eq!(Scale::Big.height(), Scale::Small.height() * 3);
    }

    #[test]
    fn every_face_renders_a_perfect_rectangle() {
        for s in Scale::ALL {
            for v in 1..=6u32 {
                let lines = face_lines(&plain(), v, "", s);
                assert_eq!(lines.len(), s.height(), "{s:?} face {v} height");
                for line in &lines {
                    assert_eq!(line.chars().count(), s.die_width(), "{s:?} face {v} row {line:?}");
                }
            }
        }
    }

    #[test]
    fn non_pip_faces_still_render_a_perfect_rectangle() {
        for s in Scale::ALL {
            for v in [0u32, 7, 20, 100] {
                let lines = face_lines(&plain(), v, "", s);
                assert_eq!(lines.len(), s.height());
                for line in &lines {
                    assert_eq!(line.chars().count(), s.die_width(), "{s:?} value {v} row {line:?}");
                }
            }
        }
    }

    #[test]
    fn small_faces_match_the_original_hand_drawn_art() {
        let f = face_lines(&plain(), 5, "", Scale::Small);
        assert_eq!(f, vec!["┌───────┐", "│ ●   ● │", "│   ●   │", "│ ●   ● │", "└───────┘"]);
        let f = face_lines(&plain(), 1, "", Scale::Small);
        assert_eq!(f, vec!["┌───────┐", "│       │", "│   ●   │", "│       │", "└───────┘"]);
    }

    #[test]
    fn pip_counts_match_the_face_value() {
        for v in 1..=6u32 {
            let lines = face_lines(&plain(), v, "", Scale::Small);
            let pips: usize = lines.iter().map(|l| l.matches('●').count()).sum();
            assert_eq!(pips, v as usize, "face {v}");
        }
    }

    #[test]
    fn fit_never_grows_as_the_table_gets_more_crowded() {
        let mut last = Scale::Big;
        for n in 1..=16 {
            let s = fit(n, 1, 14);
            assert!(s.die_width() <= last.die_width(), "{n} dice somehow grew the face");
            last = s;
        }
    }

    #[test]
    fn fit_always_yields_something_drawable() {
        // However absurd the ask, it falls back to the original small face
        // rather than to nothing at all.
        assert_eq!(fit(500, 50, 500), Scale::Small);
    }

    #[test]
    fn blocks_are_rectangular_at_every_scale() {
        for s in Scale::ALL {
            let rows = dice_block_scaled(&plain(), &[1, 2, 3], None, s);
            assert_eq!(rows.len(), s.height());
            let w = rows[0].chars().count();
            assert!(rows.iter().all(|r| r.chars().count() == w));
        }
    }
}
