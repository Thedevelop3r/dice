//! Playing-card rendering — generic enough for any future card game
//! (Blackjack today; poker, baccarat, war tomorrow) to reuse.

use super::theme::{BOLD, RED, WHITE};
use super::Theme;

/// A single face-up card, drawn as a small boxed rank+suit. `red` picks
/// hearts/diamonds coloring vs. the default (clubs/spades).
pub fn card_block(theme: &Theme, label: &str, red: bool) -> [String; 5] {
    let code = if red { RED } else { WHITE };
    let l = format!("{label:<2}");
    [
        theme.paint(code, "┌─────┐"),
        theme.paint(code, &format!("│{}   │", theme.paint(BOLD, &l))),
        theme.paint(code, "│     │"),
        theme.paint(code, &format!("│   {}│", theme.paint(BOLD, &format!("{label:>2}")))),
        theme.paint(code, "└─────┘"),
    ]
}

/// A face-down card — the dealer's hole card.
pub fn card_back(theme: &Theme) -> [String; 5] {
    let code = super::theme::CYAN;
    [
        theme.paint(code, "┌─────┐"),
        theme.paint(code, "│▒▒▒▒▒│"),
        theme.paint(code, "│▒▒▒▒▒│"),
        theme.paint(code, "│▒▒▒▒▒│"),
        theme.paint(code, "└─────┘"),
    ]
}

/// Lays a hand of cards out side by side. Pass `None` for a card to draw
/// its back instead (the dealer's hole card before the reveal).
pub fn hand_block(theme: &Theme, cards: &[(String, bool)], hidden: &[bool]) -> Vec<String> {
    let blocks: Vec<[String; 5]> = cards
        .iter()
        .enumerate()
        .map(|(i, (label, red))| {
            if hidden.get(i).copied().unwrap_or(false) {
                card_back(theme)
            } else {
                card_block(theme, label, *red)
            }
        })
        .collect();
    let mut out = Vec::with_capacity(5);
    for row in 0..5 {
        let mut line = String::new();
        for b in &blocks {
            line.push_str(&b[row]);
            line.push(' ');
        }
        out.push(line);
    }
    out
}
