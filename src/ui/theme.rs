//! Casino color palette and small text-styling helpers. Everything the
//! rest of the app draws with goes through here, so a new theme (or a
//! colorblind/no-color mode) is a one-file change.
//!
//! The palette below is deliberately larger than what today's games use —
//! it's the reserved set future tables (poker chip colors, a felt
//! background, a purple/pink neon theme) draw from without touching this
//! file again.
#![allow(dead_code)]

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const ITALIC: &str = "\x1b[3m";
pub const UNDERLINE: &str = "\x1b[4m";
pub const BLINK: &str = "\x1b[5m";
pub const REVERSE: &str = "\x1b[7m";

pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";

// A small "casino" extension on the base 8 colors, via 256-color codes —
// felt green, gold, neon pink and a couple of accent shades that read a
// lot richer than plain ANSI on any modern terminal.
pub const FELT: &str = "\x1b[38;5;22m";
pub const FELT_BRIGHT: &str = "\x1b[38;5;35m";
pub const GOLD: &str = "\x1b[38;5;220m";
pub const GOLD_DIM: &str = "\x1b[38;5;178m";
pub const NEON_PINK: &str = "\x1b[38;5;198m";
pub const NEON_PURPLE: &str = "\x1b[38;5;135m";
pub const ICE: &str = "\x1b[38;5;51m";
pub const CHIP_RED: &str = "\x1b[38;5;196m";
pub const CHIP_BLUE: &str = "\x1b[38;5;33m";
pub const CHIP_GREEN: &str = "\x1b[38;5;46m";
pub const CHIP_BLACK: &str = "\x1b[38;5;240m";
pub const FELT_BG: &str = "\x1b[48;5;22m";
pub const GOLD_BG: &str = "\x1b[48;5;220m";

/// The active look-and-feel. Only `colors` is user-facing today, but the
/// struct is here so a future theme picker just adds a variant.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub colors: bool,
}

impl Theme {
    pub fn new(colors: bool) -> Theme {
        Theme { colors }
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.colors {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn gold(&self, text: &str) -> String {
        self.paint(GOLD, text)
    }
    pub fn felt(&self, text: &str) -> String {
        self.paint(FELT_BRIGHT, text)
    }
    pub fn win(&self, text: &str) -> String {
        self.paint(GREEN, text)
    }
    pub fn lose(&self, text: &str) -> String {
        self.paint(RED, text)
    }
    pub fn accent(&self, text: &str) -> String {
        self.paint(ICE, text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.paint(DIM, text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.paint(BOLD, text)
    }
    pub fn key(&self, text: &str) -> String {
        if self.colors {
            format!("{BOLD}{GOLD}{text}{RESET}")
        } else {
            text.to_string()
        }
    }
    pub fn chip(&self, text: &str) -> String {
        self.paint(NEON_PINK, text)
    }
}

/// Backward-compatible free function (older call sites use this form).
pub fn color(on: bool, code: &str, text: &str) -> String {
    if on {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}
