//! Small reusable widgets built on top of `Screen` + `input`: a live
//! numeric stepper (arrows/presets/typed digits, no Enter required unless
//! you're typing a specific number), a name field, progress bars, and the
//! footer key-hint strip every screen ends with.

use super::input::Key;
use super::theme::{BOLD, DIM, GOLD, GREEN, RED};
use super::{read_key, Screen};

/// A live bet/quantity stepper. `draw` renders the *entire* frame given
/// the value so far (every redraw is a full frame, matching the rest of
/// the UI) and is called once up front and again after every change.
///
/// Controls: ↑/→ or `+` adds `step`; ↓/← or `-` subtracts it; digits type
/// a fresh number directly; Backspace erases a digit; `presets` are extra
/// single-key jumps (e.g. `('a', max)` for all-in); Enter confirms; Esc
/// cancels.
pub fn number_picker<F: FnMut(&mut Screen, i64)>(
    screen: &mut Screen,
    min: i64,
    max: i64,
    default: i64,
    step: i64,
    presets: &[(char, i64)],
    mut draw: F,
) -> Option<i64> {
    let mut value = default.clamp(min, max);
    let mut typing = false;
    draw(screen, value);
    screen.present();
    loop {
        match read_key() {
            Key::Up | Key::Right => {
                typing = false;
                value = (value + step).min(max);
                draw(screen, value);
                screen.present();
            }
            Key::Down | Key::Left => {
                typing = false;
                value = (value - step).max(min);
                draw(screen, value);
                screen.present();
            }
            Key::Char(c) if c.is_ascii_digit() => {
                let base = if typing { value } else { 0 };
                let next = base * 10 + c.to_digit(10).unwrap() as i64;
                typing = true;
                value = next.clamp(min, max);
                draw(screen, value);
                screen.present();
            }
            Key::Backspace => {
                if typing {
                    value = (value / 10).clamp(min.min(0), max);
                    if value < min {
                        value = min;
                        typing = false;
                    }
                    draw(screen, value);
                    screen.present();
                }
            }
            Key::Char(c) => {
                if let Some((_, v)) = presets.iter().find(|(k, _)| k.eq_ignore_ascii_case(&c)) {
                    typing = false;
                    value = (*v).clamp(min, max);
                    draw(screen, value);
                    screen.present();
                }
            }
            Key::Enter => return Some(value),
            Key::Esc | Key::Quit => return None,
        }
    }
}

/// A short free-text field (player names) — the one place typed text is
/// unavoidable. Characters echo live; Enter confirms, Esc keeps `default`.
pub fn text_input<F: FnMut(&mut Screen, &str)>(screen: &mut Screen, default: &str, max_len: usize, mut draw: F) -> String {
    let mut buf = String::new();
    draw(screen, &buf);
    screen.present();
    loop {
        match read_key() {
            Key::Char(c) if !c.is_control() && buf.chars().count() < max_len => {
                buf.push(c);
                draw(screen, &buf);
                screen.present();
            }
            Key::Backspace => {
                buf.pop();
                draw(screen, &buf);
                screen.present();
            }
            Key::Enter => return if buf.trim().is_empty() { default.to_string() } else { buf.trim().to_string() },
            Key::Esc | Key::Quit => return default.to_string(),
            _ => {}
        }
    }
}

/// A filled progress bar, `width` cells wide, used for scoreboards and
/// wallet/target readouts.
pub fn bar(theme: &super::Theme, value: i64, target: i64, width: usize) -> String {
    let frac = if target <= 0 { 0.0 } else { (value.max(0) as f64 / target as f64).clamp(0.0, 1.0) };
    let filled = (frac * width as f64).round() as usize;
    let filled = filled.min(width);
    let bar = format!("{}{}", "█".repeat(filled), "·".repeat(width - filled));
    theme.paint(if filled >= width { GREEN } else { super::theme::CYAN }, &bar)
}

/// The footer strip of key hints every screen ends its frame with, e.g.
/// `[R] Roll   [H] Hold   [Q] Quit`.
pub fn footer(theme: &super::Theme, hints: &[(char, &str)]) -> String {
    let mut s = String::new();
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            s.push_str("   ");
        }
        s.push_str(&theme.paint(GOLD, &format!("[{}]", key.to_ascii_uppercase())));
        s.push(' ');
        s.push_str(&theme.paint(DIM, label));
    }
    s
}

/// A centered, boxed banner for big moments — round wins, jackpots, busts.
pub fn banner(theme: &super::Theme, text: &str, win: bool) -> Vec<String> {
    let code = if win { GREEN } else { RED };
    let inner = format!(" {text} ");
    let width = inner.chars().count();
    let bold_inner = if theme.colors {
        format!("{BOLD}{code}{inner}{}", super::theme::RESET)
    } else {
        inner.clone()
    };
    vec![
        theme.paint(code, &format!("╔{}╗", "═".repeat(width))),
        format!("{}{bold_inner}{}", theme.paint(code, "║"), theme.paint(code, "║")),
        theme.paint(code, &format!("╚{}╝", "═".repeat(width))),
    ]
}
