//! Playing-card rendering — generic enough for any card game on the floor
//! (Blackjack, Baccarat, Video Poker, Three Card Poker, War, Hi-Lo) to
//! reuse.
//!
//! Cards draw at the same three sizes dice do, and for the same reason:
//! `Scale::Big` is exactly three times `Scale::Small` on both axes — 21x15
//! against 7x5 — and the block functions measure the terminal and take the
//! largest that fits. A big card fills its face with the suit rendered as
//! block art; the small one keeps the original bare corner indices.

use super::theme::{BOLD, CYAN, RED, RESET, WHITE};
use super::Scale;
use super::Theme;

/// The rank+suit index printed in a card's corners. Three columns is
/// exactly enough for the widest of them, `10♦` — pinning it here is what
/// keeps a ten from rendering one column wider than every other card and
/// shearing the rest of the hand out of alignment.
const INDEX_W: usize = 3;

// Suit faces as block art, sized to sit in the middle of a big or middle
// card. The small card has no room for one and shows only its indices.
const SPADE_BIG: [&str; 5] = ["   █   ", "  ███  ", " █████ ", "███████", "  ███  "];
const HEART_BIG: [&str; 5] = [" ██ ██ ", "███████", "███████", " █████ ", "   █   "];
const DIAMOND_BIG: [&str; 5] = ["   █   ", "  ███  ", " █████ ", "  ███  ", "   █   "];
const CLUB_BIG: [&str; 5] = ["  ███  ", "█ ███ █", "███████", " █████ ", "   █   "];

const SPADE_MID: [&str; 3] = ["  █  ", " ███ ", "█████"];
const HEART_MID: [&str; 3] = ["██ ██", "█████", " ███ "];
const DIAMOND_MID: [&str; 3] = ["  █  ", " ███ ", "  █  "];
const CLUB_MID: [&str; 3] = [" ███ ", "█████", "  █  "];

/// The biggest card size that fits `per_row` cards across and `rows` rows
/// of them, leaving `reserved` terminal rows for the surrounding chrome.
pub fn fit(per_row: usize, rows: usize, reserved: usize) -> Scale {
    super::fit_scale(per_row, rows, reserved, Scale::card_width)
}

/// The block art for a card's suit, read off the last character of its
/// label. Empty at the smallest size, which has no room for it.
fn suit_art(label: &str, scale: Scale) -> &'static [&'static str] {
    let suit = label.chars().last().unwrap_or('♠');
    match scale {
        Scale::Small => &[],
        Scale::Mid => match suit {
            '♥' => &HEART_MID,
            '♦' => &DIAMOND_MID,
            '♣' => &CLUB_MID,
            _ => &SPADE_MID,
        },
        Scale::Big => match suit {
            '♥' => &HEART_BIG,
            '♦' => &DIAMOND_BIG,
            '♣' => &CLUB_BIG,
            _ => &SPADE_BIG,
        },
    }
}

/// A single face-up card at an explicit size. `red` picks hearts/diamonds
/// coloring vs. the default (clubs/spades).
pub fn card_block_scaled(theme: &Theme, label: &str, red: bool, scale: Scale) -> Vec<String> {
    let code = if red { RED } else { WHITE };
    let inner_w = scale.card_width() - 2;
    let inner_h = scale.height() - 2;
    let idx_left = format!("{label:<INDEX_W$}");
    let idx_right = format!("{label:>INDEX_W$}");
    let art = suit_art(label, scale);
    let art_top = (inner_h.saturating_sub(art.len())) / 2;

    let mut out = Vec::with_capacity(scale.height());
    let mut buf = String::with_capacity(scale.card_width() * 4);

    // The index sits bold inside a card already painted in the suit's
    // color, so it restores that color rather than resetting to plain —
    // otherwise everything after it on the line loses the card's tint.
    let push_index = |buf: &mut String, text: &str| {
        if theme.colors {
            buf.push_str(BOLD);
            buf.push_str(text);
            buf.push_str(RESET);
            buf.push_str(code);
        } else {
            buf.push_str(text);
        }
    };

    buf.push('┌');
    for _ in 0..inner_w {
        buf.push('─');
    }
    buf.push('┐');
    out.push(theme.paint(code, &buf));

    for r in 0..inner_h {
        buf.clear();
        buf.push('│');
        if r == 0 {
            push_index(&mut buf, &idx_left);
            for _ in 0..inner_w.saturating_sub(idx_left.chars().count()) {
                buf.push(' ');
            }
        } else if r == inner_h - 1 {
            for _ in 0..inner_w.saturating_sub(idx_right.chars().count()) {
                buf.push(' ');
            }
            push_index(&mut buf, &idx_right);
        } else if !art.is_empty() && r >= art_top && r < art_top + art.len() {
            let row = art[r - art_top];
            let pad = inner_w.saturating_sub(row.chars().count());
            let left = pad / 2;
            for _ in 0..left {
                buf.push(' ');
            }
            buf.push_str(row);
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

    buf.clear();
    buf.push('└');
    for _ in 0..inner_w {
        buf.push('─');
    }
    buf.push('┘');
    out.push(theme.paint(code, &buf));
    out
}

/// A face-down card at an explicit size — the dealer's hole card.
pub fn card_back_scaled(theme: &Theme, scale: Scale) -> Vec<String> {
    let inner_w = scale.card_width() - 2;
    let inner_h = scale.height() - 2;
    let mut out = Vec::with_capacity(scale.height());
    let mut buf = String::with_capacity(scale.card_width() * 4);

    buf.push('┌');
    for _ in 0..inner_w {
        buf.push('─');
    }
    buf.push('┐');
    out.push(theme.paint(CYAN, &buf));
    for _ in 0..inner_h {
        buf.clear();
        buf.push('│');
        for _ in 0..inner_w {
            buf.push('▒');
        }
        buf.push('│');
        out.push(theme.paint(CYAN, &buf));
    }
    buf.clear();
    buf.push('└');
    for _ in 0..inner_w {
        buf.push('─');
    }
    buf.push('┘');
    out.push(theme.paint(CYAN, &buf));
    out
}

/// Lays a hand of cards out side by side at an explicit size. Positions
/// marked `true` in `hidden` draw their back instead (the dealer's hole
/// card before the reveal).
pub fn hand_block_scaled(theme: &Theme, cards: &[(String, bool)], hidden: &[bool], scale: Scale) -> Vec<String> {
    if cards.is_empty() {
        return Vec::new();
    }
    let blocks: Vec<Vec<String>> = cards
        .iter()
        .enumerate()
        .map(|(i, (label, red))| {
            if hidden.get(i).copied().unwrap_or(false) {
                card_back_scaled(theme, scale)
            } else {
                card_block_scaled(theme, label, *red, scale)
            }
        })
        .collect();
    let mut out = Vec::with_capacity(scale.height());
    for row in 0..scale.height() {
        let mut line = String::with_capacity(blocks.len() * (scale.card_width() + 12));
        for b in &blocks {
            line.push_str(&b[row]);
            line.push(' ');
        }
        out.push(line);
    }
    out
}

/// Lays a hand out side by side at the biggest size the terminal fits.
/// The height budget assumes two hands stacked (a player's and a
/// dealer's), which is how nearly every table here is laid out.
pub fn hand_block(theme: &Theme, cards: &[(String, bool)], hidden: &[bool]) -> Vec<String> {
    if cards.is_empty() {
        return Vec::new();
    }
    hand_block_scaled(theme, cards, hidden, fit(cards.len(), 2, 12))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Theme {
        Theme::new(false)
    }

    #[test]
    fn every_card_renders_a_perfect_rectangle() {
        for s in Scale::ALL {
            for label in ["A♠", "10♦", "K♥", "7♣", "Q♠"] {
                let lines = card_block_scaled(&plain(), label, false, s);
                assert_eq!(lines.len(), s.height(), "{s:?} {label} height");
                for line in &lines {
                    assert_eq!(line.chars().count(), s.card_width(), "{s:?} {label} row {line:?}");
                }
            }
        }
    }

    #[test]
    fn a_ten_is_exactly_as_wide_as_every_other_card() {
        // The whole point of the fixed-width index: a hand holding a ten
        // used to shear one column out of line with its neighbours.
        for s in Scale::ALL {
            let ten = card_block_scaled(&plain(), "10♣", false, s);
            let four = card_block_scaled(&plain(), "4♣", false, s);
            for (a, b) in ten.iter().zip(four.iter()) {
                assert_eq!(a.chars().count(), b.chars().count(), "{s:?}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn card_backs_match_their_faces_in_size() {
        for s in Scale::ALL {
            let back = card_back_scaled(&plain(), s);
            let face = card_block_scaled(&plain(), "A♠", false, s);
            assert_eq!(back.len(), face.len());
            for (a, b) in back.iter().zip(face.iter()) {
                assert_eq!(a.chars().count(), b.chars().count());
            }
        }
    }

    #[test]
    fn big_is_exactly_three_times_small() {
        assert_eq!(Scale::Big.card_width(), Scale::Small.card_width() * 3);
        assert_eq!(Scale::Big.height(), Scale::Small.height() * 3);
    }

    #[test]
    fn small_cards_match_the_original_hand_drawn_art() {
        assert_eq!(
            card_block_scaled(&plain(), "A♠", false, Scale::Small),
            vec!["┌─────┐", "│A♠   │", "│     │", "│   A♠│", "└─────┘"]
        );
    }

    #[test]
    fn a_hand_lays_out_as_one_rectangle() {
        let cards = vec![("A♠".to_string(), false), ("10♥".to_string(), true), ("K♣".to_string(), false)];
        let hidden = vec![false, false, true];
        for s in Scale::ALL {
            let rows = hand_block_scaled(&plain(), &cards, &hidden, s);
            assert_eq!(rows.len(), s.height());
            let w = rows[0].chars().count();
            assert!(rows.iter().all(|r| r.chars().count() == w), "{s:?} ragged hand");
        }
    }
}
