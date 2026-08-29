//! The boxed, keyed menu used for the main dashboard and every sub-menu
//! (settings, the store, rules). One call draws it and blocks for the
//! matching keypress — nothing to type, nothing to confirm with Enter.

use super::theme::{BOLD, DIM, GOLD, GOLD_DIM};
use super::{choose_key, Screen};

pub struct MenuItem {
    pub key: char,
    pub label: String,
    pub hint: String,
}

impl MenuItem {
    pub fn new(key: char, label: impl Into<String>, hint: impl Into<String>) -> MenuItem {
        MenuItem { key, label: label.into(), hint: hint.into() }
    }
}

/// Appends a boxed list of `items` to the screen's current frame and, once
/// presented, blocks for a matching keypress. `title` sits in the box's
/// top border. Returns `None` only if the whole app is being torn down.
pub fn choose_from(screen: &mut Screen, title: &str, items: &[MenuItem]) -> Option<char> {
    let theme = screen.theme;
    let width = items
        .iter()
        .map(|i| i.label.chars().count() + i.hint.chars().count() + 10)
        .max()
        .unwrap_or(20)
        .max(title.chars().count() + 6)
        .min(76);

    let top = format!("┌─ {} {}", theme.paint(BOLD, title), "─".repeat(width.saturating_sub(title.chars().count() + 4)));
    screen.line(&theme.paint(GOLD_DIM, &format!("{top}┐")));
    for it in items {
        let line = format!(
            "  {} {:<20} {}",
            theme.paint(GOLD, &format!("[{}]", it.key.to_ascii_uppercase())),
            theme.paint(BOLD, &it.label),
            theme.paint(DIM, &it.hint)
        );
        screen.line(&format!("{}{}", theme.paint(GOLD_DIM, "│"), pad_visible(&line, width + 2)));
    }
    screen.line(&theme.paint(GOLD_DIM, &format!("└{}┘", "─".repeat(width + 2))));
    screen.present();

    let valid: Vec<char> = items.iter().map(|i| i.key).collect();
    let fallback = valid.iter().find(|c| **c == 'q' || **c == 'b').copied().unwrap_or(valid[0]);
    choose_key(&valid, fallback)
}

/// Right-pads a string to `width` *visible* columns, ignoring ANSI escape
/// codes when counting — good enough for the fixed-format lines we build.
fn pad_visible(s: &str, width: usize) -> String {
    let visible = visible_len(s);
    if visible >= width {
        format!("{s}│")
    } else {
        format!("{}{}│", s, " ".repeat(width - visible))
    }
}

pub fn visible_len(s: &str) -> usize {
    let mut count = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        count += 1;
    }
    count
}
