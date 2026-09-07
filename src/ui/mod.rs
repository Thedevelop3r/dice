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
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The casino's live money and chips, shown in the top-right of every
/// screen once the floor is open.
///
/// It is a shared, lock-free cell rather than a string the app remembers to
/// refresh: the simulation thread writes into it whenever the books move,
/// and `Screen::present` reads it on the way out. That is what makes the
/// figure current on *every* frame the app draws — including deep inside a
/// hand of blackjack, where nothing else would think to update it.
#[derive(Clone, Default)]
pub struct Badge {
    money: Arc<AtomicI64>,
    chips: Arc<AtomicI64>,
    live: Arc<AtomicBool>,
}

impl Badge {
    pub fn new() -> Badge {
        Badge::default()
    }

    /// Called by the simulation thread. Never blocks a frame.
    pub fn set(&self, money: i64, chips: i64) {
        self.money.store(money, Ordering::Relaxed);
        self.chips.store(chips, Ordering::Relaxed);
        self.live.store(true, Ordering::Relaxed);
    }

    /// Hides the badge — the floor is closed.
    pub fn clear(&self) {
        self.live.store(false, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    pub fn read(&self) -> (i64, i64) {
        (self.money.load(Ordering::Relaxed), self.chips.load(Ordering::Relaxed))
    }
}

/// `1234567` as `1,234,567`.
pub fn thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if neg { format!("-{out}") } else { out }
}

/// How many columns a string actually occupies, ignoring colour escapes.
pub fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\u{1b}' {
            in_escape = true;
        } else {
            n += 1;
        }
    }
    n
}

/// Cuts a possibly-coloured string down to `width` visible columns.
///
/// Escapes are carried through rather than counted, and a colour left open
/// by the cut is closed on the way out — otherwise a truncated line would
/// bleed its colour across the rest of the screen. This is what lets a
/// panel be laid out for one width and drawn safely in a narrower window.
pub fn clip(s: &str, width: usize) -> String {
    if visible_len(s) <= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut seen = 0;
    let mut in_escape = false;
    let mut painted = false;
    for c in s.chars() {
        if in_escape {
            out.push(c);
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        if c == '\u{1b}' {
            out.push(c);
            in_escape = true;
            painted = true;
            continue;
        }
        if seen == width {
            break;
        }
        out.push(c);
        seen += 1;
    }
    if painted {
        out.push_str(theme::RESET);
    }
    out
}

/// Pads a possibly-coloured string to `width` visible columns, text first.
/// `format!("{:<9}")` counts escape bytes as characters, so any column
/// holding painted text has to be padded through here instead.
pub fn pad_end(s: &str, width: usize) -> String {
    let vis = visible_len(s);
    format!("{s}{}", " ".repeat(width.saturating_sub(vis)))
}

/// The same, padding first — for right-aligned columns of figures.
pub fn pad_start(s: &str, width: usize) -> String {
    let vis = visible_len(s);
    format!("{}{s}", " ".repeat(width.saturating_sub(vis)))
}

/// Owns the terminal session: alternate screen + raw input mode, both torn
/// down automatically on drop (including on panic-unwind), so the user's
/// shell is never left cleared, cursor-hidden or echo-less.
pub struct Screen {
    pub theme: Theme,
    _raw: Option<input::RawGuard>,
    buf: String,
    /// The casino economy readout, stamped into the top-right corner of
    /// every presented frame. `None` until a casino is opened.
    badge: Option<Badge>,
}

impl Screen {
    pub fn open(colors: bool) -> Screen {
        let mut out = std::io::stdout();
        print!("\x1b[?1049h\x1b[?25l"); // alternate screen + hide cursor
        let _ = out.flush();
        Screen { theme: Theme::new(colors), _raw: input::RawGuard::enable(), buf: String::new(), badge: None }
    }

    /// A `Screen` that has claimed no terminal at all — no alternate
    /// buffer, no raw mode, nothing written on drop. Unit tests that need
    /// a whole `Ctx` to exercise settlement maths build one of these so
    /// they never touch the real stdout the test harness is capturing.
    #[cfg(test)]
    pub fn headless() -> Screen {
        Screen { theme: Theme::new(false), _raw: None, buf: String::new(), badge: None }
    }

    /// Hands the screen the casino's live balances. From here on every
    /// frame carries them, whatever screen drew it.
    pub fn attach_badge(&mut self, badge: Badge) {
        self.badge = Some(badge);
    }

    /// The badge as it will be drawn, or `None` when no casino is open.
    fn badge_line(&self) -> Option<String> {
        let b = self.badge.as_ref()?;
        if !b.is_live() {
            return None;
        }
        let (money, chips) = b.read();
        let theme = self.theme;
        Some(format!(
            "{}  {}",
            theme.paint(theme::GREEN, &format!("Money: ${}", thousands(money))),
            theme.paint(theme::GOLD, &format!("Chips: {}", thousands(chips)))
        ))
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

    /// Flushes the buffered frame to the terminal in one write, then
    /// stamps the casino badge into the top-right corner.
    ///
    /// Stamping it here — with an absolute cursor move, after the frame —
    /// rather than asking every screen to draw it is what keeps the badge
    /// genuinely always-on without touching a single other screen.
    pub fn present(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(self.buf.as_bytes());
        if let Some(line) = self.badge_line() {
            let (cols, _) = term_size();
            let col = (cols as usize).saturating_sub(visible_len(&line) + 2).max(1);
            // Save the cursor, jump to the first row, draw, jump back — so
            // whatever the frame was doing is left undisturbed.
            let _ = out.write_all(format!("\x1b[s\x1b[1;{col}H{line}\x1b[u").as_bytes());
        }
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
