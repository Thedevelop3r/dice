//! A plain-text, append-only log of Ultra Casino Dice sessions — one line
//! per round, one line per seat's result, one line per session summary —
//! viewable later from the dashboard's History screen (`main.rs`).
//!
//! Deliberately not folded into `stats::Store`: that file is a `key=value`
//! settings map rewritten whole on every save, which is the wrong shape
//! for a log that only ever grows. This is the simplest thing that could
//! work — one `OpenOptions::append` per line — and it's a plain text file
//! the player can `tail -f` or open by hand if they want to.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

fn log_path() -> PathBuf {
    crate::stats::data_dir().join("ultra_history.log")
}

/// Appends one line, creating the save directory and file on first use.
/// Silently does nothing on a write failure (a full disk or a read-only
/// filesystem shouldn't crash a spectacle screen) — the same
/// best-effort spirit as `Store::save`'s `let _ =` call sites elsewhere.
pub fn append(line: &str) {
    if let Some(dir) = log_path().parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{line}");
    }
}

/// The whole log, oldest entry first (the order it was written in).
/// Empty if nothing has been logged yet.
pub fn read_all() -> Vec<String> {
    fs::read_to_string(log_path()).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()
}

pub fn path_display() -> String {
    log_path().display().to_string()
}

/// `"YYYY-MM-DD HH:MM UTC"` for the current moment — just enough of a
/// timestamp to label history entries without pulling in a date/time
/// crate. The civil-date math is Howard Hinnant's well-known
/// `civil_from_days` algorithm (proleptic Gregorian, correct for any date
/// this app will ever see).
pub fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02} UTC")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
