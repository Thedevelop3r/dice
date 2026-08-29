//! Raw, single-keypress terminal input — no external crates.
//!
//! The whole point of this module: a menu or in-game prompt reacts the
//! instant a key is *pressed*. There is no line buffer, no echo, and no
//! Enter key required to "submit" a choice. We get there by putting the
//! tty into non-canonical, no-echo ("raw") mode ourselves via direct
//! `termios(3)` calls — the project has no network access to pull in a
//! crate like `crossterm`, and honestly a couple hundred lines of FFI is
//! all this needs.
//!
//! Only Unix targets get the real thing; anything else falls back to a
//! line-buffered reader so the project still compiles elsewhere.

use std::sync::atomic::{AtomicBool, Ordering};

/// Latched once the user asks to leave immediately (Ctrl+C, or stdin
/// closing/EOF, e.g. when the app is driven from a script or pipe). Every
/// blocking read checks this so retry loops fall through instead of
/// hanging forever.
static QUIT: AtomicBool = AtomicBool::new(false);

pub fn quit_requested() -> bool {
    QUIT.load(Ordering::Relaxed)
}

pub fn request_quit() {
    QUIT.store(true, Ordering::Relaxed);
}

/// A single input event. Arrow keys and Enter are decoded from raw bytes
/// (escape sequences included); everything printable arrives as `Char`,
/// already lowercase-normalized is *not* done here — callers decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    /// Ctrl+C or stdin EOF — "get me out of here, now".
    Quit,
}

#[cfg(unix)]
mod raw {
    use std::os::unix::io::AsRawFd;

    // Matches Linux's `struct termios` (asm-generic/termbits.h). NCCS=32.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Termios {
        c_iflag: u32,
        c_oflag: u32,
        c_cflag: u32,
        c_lflag: u32,
        c_line: u8,
        c_cc: [u8; 32],
        c_ispeed: u32,
        c_ospeed: u32,
    }

    const ICANON: u32 = 0o0000002;
    const ECHO: u32 = 0o0000010;
    const ISIG: u32 = 0o0000001;
    const IXON: u32 = 0o0002000;
    const ICRNL: u32 = 0o0000400;
    const VMIN: usize = 6;
    const VTIME: usize = 5;
    const TCSANOW: i32 = 0;

    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0o0004000;

    unsafe extern "C" {
        fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, action: i32, termios: *const Termios) -> i32;
        fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
        fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    }

    /// Puts stdin into raw mode on construction, restores the original
    /// mode on drop (including on panic-unwind), so the user's shell is
    /// never left in a broken state.
    pub struct RawGuard {
        fd: i32,
        original: Termios,
    }

    impl RawGuard {
        pub fn enable() -> Option<RawGuard> {
            let fd = std::io::stdin().as_raw_fd();
            let mut original: Termios = unsafe { std::mem::zeroed() };
            if unsafe { tcgetattr(fd, &mut original) } != 0 {
                return None;
            }
            let mut raw = original;
            raw.c_lflag &= !(ICANON | ECHO | ISIG);
            raw.c_iflag &= !(IXON | ICRNL);
            // Deliberately leave c_oflag (OPOST/ONLCR) untouched: this is
            // *input* raw mode only. Disabling output post-processing too
            // would stop the tty from turning our "\n"s into "\r\n"s,
            // staircasing every line we print down-and-right.
            raw.c_cc[VMIN] = 1;
            raw.c_cc[VTIME] = 0;
            if unsafe { tcsetattr(fd, TCSANOW, &raw) } != 0 {
                return None;
            }
            Some(RawGuard { fd, original })
        }
    }

    impl Drop for RawGuard {
        fn drop(&mut self) {
            unsafe {
                tcsetattr(self.fd, TCSANOW, &self.original);
            }
        }
    }

    fn set_nonblocking(fd: i32, on: bool) {
        unsafe {
            let flags = fcntl(fd, F_GETFL, 0);
            if flags < 0 {
                return;
            }
            let next = if on { flags | O_NONBLOCK } else { flags & !O_NONBLOCK };
            fcntl(fd, F_SETFL, next);
        }
    }

    fn read_byte_blocking() -> Option<u8> {
        let fd = std::io::stdin().as_raw_fd();
        let mut buf = [0u8; 1];
        loop {
            let n = unsafe { read(fd, buf.as_mut_ptr(), 1) };
            if n == 1 {
                return Some(buf[0]);
            }
            if n == 0 {
                return None; // EOF
            }
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
    }

    /// Tries to read one more byte without blocking — used right after an
    /// ESC byte to tell "lone Esc key" from "start of an arrow-key escape
    /// sequence", which arrive as a burst from the terminal.
    fn read_byte_nonblocking() -> Option<u8> {
        let fd = std::io::stdin().as_raw_fd();
        set_nonblocking(fd, true);
        // Escape sequences land in the input buffer essentially atomically;
        // a very short spin is enough without adding perceptible latency
        // to a genuine standalone Esc press.
        let mut byte = None;
        for _ in 0..3 {
            let mut buf = [0u8; 1];
            let n = unsafe { read(fd, buf.as_mut_ptr(), 1) };
            if n == 1 {
                byte = Some(buf[0]);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        set_nonblocking(fd, false);
        byte
    }

    /// A single non-blocking read attempt — no spin, no wait. Used by
    /// `poll_leave_signal` to check "is anything waiting?" once per
    /// animation frame without slowing the animation down.
    fn read_byte_nonblocking_once() -> Option<u8> {
        let fd = std::io::stdin().as_raw_fd();
        set_nonblocking(fd, true);
        let mut buf = [0u8; 1];
        let n = unsafe { read(fd, buf.as_mut_ptr(), 1) };
        set_nonblocking(fd, false);
        if n == 1 {
            Some(buf[0])
        } else {
            None
        }
    }

    /// True if Q/q or Ctrl+C was pressed since the last call. Drains and
    /// discards *every* byte currently waiting (not just one), so holding
    /// a key down — or a stray escape sequence — can't starve the check;
    /// everything except Q and Ctrl+C is silently swallowed rather than
    /// queued, since an unattended screen has nowhere to feed it later.
    /// This is how a hands-off animation or idle loop notices a leave
    /// request without ever blocking a frame on `read_key()`.
    pub fn poll_leave_signal() -> bool {
        let mut left = false;
        loop {
            match read_byte_nonblocking_once() {
                Some(0x03) => {
                    super::request_quit();
                    left = true;
                }
                Some(b) if b == b'q' || b == b'Q' => left = true,
                Some(_) => {}
                None => break,
            }
        }
        left
    }

    pub fn read_key() -> super::Key {
        use super::Key;
        loop {
            let Some(b) = read_byte_blocking() else {
                super::request_quit();
                return Key::Quit;
            };
            match b {
                0x03 => {
                    super::request_quit();
                    return Key::Quit;
                }
                0x1b => {
                    let Some(b2) = read_byte_nonblocking() else { return Key::Esc };
                    if b2 != b'[' && b2 != b'O' {
                        continue; // unrecognized sequence lead-in; drop it
                    }
                    let Some(b3) = read_byte_nonblocking() else { continue };
                    match b3 {
                        b'A' => return Key::Up,
                        b'B' => return Key::Down,
                        b'C' => return Key::Right,
                        b'D' => return Key::Left,
                        _ => continue,
                    }
                }
                b'\r' | b'\n' => return Key::Enter,
                0x7f | 0x08 => return Key::Backspace,
                0x00..=0x1f => continue, // other control bytes: ignore
                0x20..=0x7f => return Key::Char(b as char),
                _ => {
                    // Lead byte of a multi-byte UTF-8 sequence.
                    let extra = if b >> 5 == 0b110 {
                        1
                    } else if b >> 4 == 0b1110 {
                        2
                    } else if b >> 3 == 0b11110 {
                        3
                    } else {
                        0
                    };
                    let mut buf = vec![b];
                    for _ in 0..extra {
                        match read_byte_blocking() {
                            Some(nb) => buf.push(nb),
                            None => break,
                        }
                    }
                    if let Some(c) = std::str::from_utf8(&buf).ok().and_then(|s| s.chars().next()) {
                        return Key::Char(c);
                    }
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod raw {
    pub struct RawGuard;
    impl RawGuard {
        pub fn enable() -> Option<RawGuard> {
            None
        }
    }

    /// Degraded fallback: read a whole line and hand back its first
    /// character. Keeps the project compiling on non-Unix targets.
    pub fn read_key() -> super::Key {
        use super::Key;
        use std::io::BufRead;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            super::request_quit();
            return Key::Quit;
        }
        match line.trim().chars().next() {
            Some(c) => Key::Char(c),
            None => Key::Enter,
        }
    }

    /// No non-blocking read exists on this degraded fallback path, so an
    /// unattended screen here simply can't be interrupted mid-animation —
    /// acceptable on a target with no real raw-mode input anyway.
    pub fn poll_leave_signal() -> bool {
        false
    }
}

pub use raw::RawGuard;

/// Blocks for exactly one key press.
pub fn read_key() -> Key {
    raw::read_key()
}

/// Non-blocking: has the user asked to leave since the last call? See
/// `raw::poll_leave_signal` for the real (Unix) implementation — this is
/// what unattended animations and idle screens poll instead of blocking
/// on `read_key()`.
pub fn poll_leave_signal() -> bool {
    raw::poll_leave_signal()
}

/// Waits for any key at all — the "press any key to continue" beat.
pub fn wait_any_key() {
    let _ = read_key();
}

/// Blocks until the user presses one of `valid` (case-insensitive) or
/// Ctrl+C/EOF fires. On quit, returns `fallback` if it is itself one of
/// the valid choices, otherwise `None` so the caller can unwind.
pub fn choose_key(valid: &[char], fallback: char) -> Option<char> {
    loop {
        match read_key() {
            Key::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if let Some(m) = valid.iter().find(|v| v.to_ascii_lowercase() == lower) {
                    return Some(*m);
                }
            }
            Key::Quit | Key::Esc => {
                return if valid.contains(&fallback) { Some(fallback) } else { None };
            }
            _ => {}
        }
    }
}

/// Yes/no, single key: y or n (Enter defaults to `default`).
pub fn confirm_key(default: bool) -> bool {
    loop {
        match read_key() {
            Key::Char(c) => match c.to_ascii_lowercase() {
                'y' => return true,
                'n' => return false,
                _ => {}
            },
            Key::Enter => return default,
            Key::Quit | Key::Esc => return false,
            _ => {}
        }
    }
}
