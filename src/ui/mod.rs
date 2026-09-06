//! The UI layer: everything that touches the terminal lives under here so
//! game logic never calls `println!`/raw escape codes directly, and a new
//! game only ever needs `Screen` + a handful of widgets. Swapping the
//! whole look — a new theme, a different renderer — means editing this
//! directory and nothing else.

pub mod card_art;
pub mod dice_art;
pub mod input;
pub mod menu;
pub mod theme;
pub mod widgets;

pub use input::{choose_key, confirm_key, poll_action, poll_leave_signal, quit_requested, read_key, wait_any_key, Key, Poll};
pub use theme::{Theme, GOLD};

/// How big a die or card is drawn. `Big` is exactly three times `Small`
/// on both axes — 27x15 against 9x5 for a die, 21x15 against 7x5 for a
/// card — with `Mid` the step between. Nothing in a game picks a size
/// directly: the renderers measure the terminal and take the largest
/// that fits, so a three-die table renders huge while a twelve-die one
/// degrades gracefully on the very same screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Small,
    Mid,
    Big,
}

impl Scale {
    /// Biggest first — the order `fit_scale` walks.
    pub const ALL: [Scale; 3] = [Scale::Big, Scale::Mid, Scale::Small];

    pub const fn height(self) -> usize {
        match self {
            Scale::Small => 5,
            Scale::Mid => 10,
            Scale::Big => 15,
        }
    }

    pub const fn die_width(self) -> usize {
        match self {
            Scale::Small => 9,
            Scale::Mid => 18,
            Scale::Big => 27,
        }
    }

    pub const fn card_width(self) -> usize {
        match self {
            Scale::Small => 7,
            Scale::Mid => 14,
            Scale::Big => 21,
        }
    }
}

/// The biggest scale at which `per_row` items across and `rows` rows of
/// them still fit the live terminal, once `reserved` rows are set aside
/// for the header, HUD and footer drawn around them. `width_of` picks
/// the per-item width for the thing being laid out (a die or a card).
/// Always yields at least `Small`, which is what the app drew before
/// sizing existed, so there is no window too small to play in.
pub fn fit_scale(per_row: usize, rows: usize, reserved: usize, width_of: impl Fn(Scale) -> usize) -> Scale {
    let (cols, term_rows) = term_size();
    let avail_w = cols as usize;
    let avail_h = (term_rows as usize).saturating_sub(reserved);
    let per_row = per_row.max(1);
    let rows = rows.max(1);
    for s in Scale::ALL {
        // +1 on each axis for the gap that separates an item from its
        // neighbour and from whatever is drawn beneath it.
        if per_row * (width_of(s) + 1) <= avail_w && rows * (s.height() + 1) <= avail_h {
            return s;
        }
    }
    Scale::Small
}

use std::io::Write;
use std::time::Duration;

/// Owns the terminal session: alternate screen + raw input mode, both torn
/// down automatically on drop (including on panic-unwind), so the user's
/// shell is never left cleared, cursor-hidden or echo-less.
pub struct Screen {
    pub theme: Theme,
    _raw: Option<input::RawGuard>,
    buf: String,
}

impl Screen {
    pub fn open(colors: bool) -> Screen {
        let mut out = std::io::stdout();
        print!("\x1b[?1049h\x1b[?25l"); // alternate screen + hide cursor
        let _ = out.flush();
        Screen { theme: Theme::new(colors), _raw: input::RawGuard::enable(), buf: String::new() }
    }

    /// A `Screen` that has claimed no terminal at all — no alternate
    /// buffer, no raw mode, nothing written on drop. Unit tests that need
    /// a whole `Ctx` to exercise settlement maths build one of these so
    /// they never touch the real stdout the test harness is capturing.
    #[cfg(test)]
    pub fn headless() -> Screen {
        Screen { theme: Theme::new(false), _raw: None, buf: String::new() }
    }

    pub fn colors(&self) -> bool {
        self.theme.colors
    }

    pub fn set_colors(&mut self, on: bool) {
        self.theme.colors = on;
    }

    /// Starts a fresh frame: every `push`/`line` call after this appends to
    /// an in-memory buffer, so the whole screen changes in one write and
    /// nothing ever scrolls or flickers mid-draw.
    pub fn begin(&mut self) {
        self.buf.clear();
        // Home the cursor and erase the visible screen. We deliberately
        // skip the xterm "erase saved lines" extension (\x1b[3J) here:
        // we're already drawing inside the alternate screen buffer, which
        // has no scrollback of its own, and that sequence is not
        // universally well-behaved across terminals/multiplexers.
        self.buf.push_str("\x1b[H\x1b[2J");
    }

    pub fn push(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    pub fn line(&mut self, s: &str) {
        self.buf.push_str(s);
        self.buf.push('\n');
    }

    pub fn blank(&mut self) {
        self.buf.push('\n');
    }

    /// Flushes the buffered frame to the terminal in one write.
    pub fn present(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(self.buf.as_bytes());
        let _ = out.flush();
    }

    /// Clears and immediately presents an empty frame — used before a
    /// sequence of direct `println!`-style draws in older call sites that
    /// have not been converted to the buffered frame API yet.
    pub fn clear_now(&mut self) {
        self.begin();
        self.present();
    }

    pub fn size(&self) -> (u16, u16) {
        term_size()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        print!("\x1b[?25h\x1b[?1049l"); // show cursor, leave alternate screen
        let _ = out.flush();
    }
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(unix)]
pub fn term_size() -> (u16, u16) {
    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: u64, ...) -> i32;
    }
    const TIOCGWINSZ: u64 = 0x5413;
    let mut ws = Winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    let ok = unsafe { ioctl(1, TIOCGWINSZ, &mut ws as *mut Winsize) } == 0;
    if ok && ws.ws_col > 0 && ws.ws_row > 0 {
        (ws.ws_col, ws.ws_row)
    } else {
        (100, 40)
    }
}

#[cfg(not(unix))]
pub fn term_size() -> (u16, u16) {
    (100, 40)
}

/// Centers `text` (already may contain color codes) within `width` visible
/// columns, counting only the printable characters. Reserved for wider
/// dashboard layouts; not every screen needs centering.
#[allow(dead_code)]
pub fn center(text: &str, visible_len: usize, width: usize) -> String {
    if visible_len >= width {
        return text.to_string();
    }
    let pad = (width - visible_len) / 2;
    format!("{}{}", " ".repeat(pad), text)
}

pub fn money(theme: &Theme, chips: i64, dollars: i64) -> String {
    format!(
        "{}  {}",
        theme.paint(theme::GOLD, &format!("◆ {chips} chips")),
        theme.paint(theme::GREEN, &format!("${dollars}"))
    )
}

/// A thin gold rule across the given width.
pub fn rule(theme: &Theme, width: usize) -> String {
    theme.dim(&"─".repeat(width))
}

#[allow(dead_code)]
pub fn double_rule(theme: &Theme, width: usize) -> String {
    theme.paint(theme::GOLD_DIM, &"═".repeat(width))
}

/// A section header: blank line, bold title, rule underneath.
pub fn header(screen: &mut Screen, title: &str) {
    let theme = screen.theme;
    screen.blank();
    screen.line(&theme.bold(title));
    screen.line(&rule(&theme, title.chars().count().max(24)));
}

/// Pauses for any keypress with a dim prompt line — replaces the old
/// "press Enter to continue" beat; any key works, none of them need Enter.
pub fn pause(screen: &mut Screen) {
    let theme = screen.theme;
    screen.blank();
    screen.line(&theme.dim("··· press any key to continue ···"));
    screen.present();
    wait_any_key();
}
