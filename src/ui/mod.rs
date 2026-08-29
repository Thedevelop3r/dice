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

pub use input::{choose_key, confirm_key, poll_leave_signal, quit_requested, read_key, wait_any_key, Key};
pub use theme::{Theme, GOLD};

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
