//! The casino, written down.
//!
//! Until this file, closing the program threw the whole world away except
//! for two numbers. Everything the last several phases built — a hundred
//! and twenty people with histories, a floor of tables, a night's books,
//! what the room was in the mood for — existed only while the process did.
//! This is where it stops being true.
//!
//! ## What gets written, and what deliberately does not
//!
//! Saved: the books in full, every person the casino knows and everything
//! it knows about them, which tables were open and at what limit, the
//! room's taste in games, and the night's running totals.
//!
//! Not saved, on purpose:
//!
//! - **Who was sitting where.** People come back and choose again. Freezing
//!   a half-played round of blackjack to disk would mean a save format that
//!   has to understand every game in the building, which is exactly the
//!   coupling the rest of this module spent seven phases avoiding.
//! - **A tournament in progress.** Same reason, more so: it is a bracket
//!   mid-flight. One that has finished has already paid out, and the money
//!   is in the roster where it belongs.
//! - **The bucketed recent history.** The *totals* survive; the shape of
//!   the last hour does not. Reopening the doors is a new night.
//!
//! ## How it is written
//!
//! The three steps the roadmap asks for, in order:
//!
//! 1. **Temporary file.** Everything is written to `<name>.tmp`, never over
//!    the live save.
//! 2. **Validation.** The temporary file is read back and parsed. If it
//!    does not survive its own round trip, the save is abandoned and the
//!    existing file is left exactly as it was.
//! 3. **Atomic replacement.** `rename` swaps the temporary file into place
//!    in one step. A reader either sees the whole old file or the whole new
//!    one — never half of either, and never nothing at all.
//!
//! That order matters more than it looks. Writing straight over the save
//! file means a crash mid-write leaves a casino that cannot be opened, and
//! the moment somebody has a hundred hours in one, that is the bug that
//! actually hurts.
//!
//! ## Versions
//!
//! Every file states its version on the first line. A file from an older
//! version is **migrated** on the way in; a file from a *newer* one is
//! refused rather than guessed at, because a program cannot read a format
//! that had not been designed when it was compiled.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::instance::{Kind, Limit};
use super::patron::{Archetype, Lifetime};

/// The format this build writes. Bump it whenever the shape changes, and
/// add a step to `migrate`.
pub const VERSION: u32 = 2;

/// The oldest version that can still be read. Anything below this is from
/// before the format was worth keeping.
pub const OLDEST: u32 = 1;

/// One person, as written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPatron {
    pub id: u64,
    pub name: String,
    pub archetype: Archetype,
    pub nerve: u8,
    pub appetite: u8,
    pub discipline: u8,
    pub read: u8,
    pub lifetime: Lifetime,
}

/// One table, as written down. Who was sitting at it is not part of this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTable {
    pub kind: Kind,
    pub number: u32,
    pub limit: Limit,
}

/// The house's books, as written down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SavedBank {
    pub money: i64,
    pub chips: i64,
    pub collected: i64,
    pub paid: i64,
    pub bought_in: i64,
    pub cashed_out: i64,
    pub handle: i64,
    pub payouts: i64,
    pub rounds: u64,
    pub bets: u64,
    pub spent: i64,
}

/// A whole casino, ready to be written or just read back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Save {
    pub version: u32,
    /// Simulated time the casino had been open for.
    pub elapsed: Duration,
    pub speed: u32,
    pub bank: SavedBank,
    /// Per-table and per-expense books, as `(key, amount)`.
    pub by_table: Vec<(String, i64)>,
    pub by_expense: Vec<(String, i64)>,
    pub people: Vec<SavedPatron>,
    pub tables: Vec<SavedTable>,
    /// The room's taste, as `(game key, appeal)`.
    pub appeal: Vec<(String, i64)>,
    /// How many of each kind of table have ever been opened, so numbering
    /// carries on climbing rather than starting again at #1.
    pub opened: Vec<(String, u32)>,
    pub tourneys_held: u32,
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// No file, which is not an error — it is a casino that has never run.
    Missing,
    /// The file could not be read or written.
    Io(String),
    /// It does not begin with a version line.
    NoVersion,
    /// It is from a build that had not been written yet.
    FromTheFuture(u32),
    /// It is older than anything this build knows how to migrate.
    TooOld(u32),
    /// A line does not parse.
    Corrupt(String),
}

impl Fault {
    pub fn describe(&self) -> String {
        match self {
            Fault::Missing => "no casino has been saved yet".into(),
            Fault::Io(e) => format!("the save file could not be read: {e}"),
            Fault::NoVersion => "the save file does not say what version it is".into(),
            Fault::FromTheFuture(v) => format!("the save is version {v}, which this build does not understand"),
            Fault::TooOld(v) => format!("the save is version {v}, which is too old to read"),
            Fault::Corrupt(what) => format!("the save file is damaged: {what}"),
        }
    }
}

/// Where the casino is written. Alongside the player's own save file, in
/// the directory `stats.rs` already owns.
pub fn path() -> PathBuf {
    crate::stats::data_dir().join("casino.save")
}

fn tmp_path(at: &Path) -> PathBuf {
    at.with_extension("save.tmp")
}

// ---------------------------------------------------------------- writing

impl Save {
    /// Renders the save as text.
    ///
    /// Line-based and pipe-delimited, for the same reason `stats.rs` is
    /// `key=value`: it can be read by a person, diffed, and repaired by
    /// hand, and it needs no dependency to parse. Names may contain spaces
    /// but never pipes, which is what makes the delimiter safe.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "version={}", self.version);
        let _ = writeln!(out, "# dice_arena casino — written by the house, read by the house");
        let _ = writeln!(out, "elapsed_ms={}", self.elapsed.as_millis());
        let _ = writeln!(out, "speed={}", self.speed);
        let b = &self.bank;
        let _ = writeln!(out, "bank.money={}", b.money);
        let _ = writeln!(out, "bank.chips={}", b.chips);
        let _ = writeln!(out, "bank.collected={}", b.collected);
        let _ = writeln!(out, "bank.paid={}", b.paid);
        let _ = writeln!(out, "bank.bought_in={}", b.bought_in);
        let _ = writeln!(out, "bank.cashed_out={}", b.cashed_out);
        let _ = writeln!(out, "bank.handle={}", b.handle);
        let _ = writeln!(out, "bank.payouts={}", b.payouts);
        let _ = writeln!(out, "bank.rounds={}", b.rounds);
        let _ = writeln!(out, "bank.bets={}", b.bets);
        let _ = writeln!(out, "bank.spent={}", b.spent);
        let _ = writeln!(out, "tourneys_held={}", self.tourneys_held);
        for (k, v) in &self.by_table {
            let _ = writeln!(out, "table_book={k}|{v}");
        }
        for (k, v) in &self.by_expense {
            let _ = writeln!(out, "expense_book={k}|{v}");
        }
        for (k, v) in &self.appeal {
            let _ = writeln!(out, "appeal={k}|{v}");
        }
        for (k, v) in &self.opened {
            let _ = writeln!(out, "opened={k}|{v}");
        }
        for t in &self.tables {
            let _ = writeln!(out, "table={}|{}|{}", t.kind.key(), t.number, t.limit.label());
        }
        for p in &self.people {
            let l = &p.lifetime;
            let _ = writeln!(
                out,
                "patron={}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                p.id,
                p.name,
                p.archetype.label(),
                p.nerve,
                p.appetite,
                p.discipline,
                p.read,
                l.visits,
                l.rounds,
                l.staked,
                l.returned,
                l.biggest_win,
                l.best_visit,
                l.worst_visit
            );
        }
        out
    }

    /// Writes the save: temporary file, validation, atomic replacement.
    ///
    /// The existing file is untouched unless all three succeed.
    pub fn write(&self, at: &Path) -> Result<(), Fault> {
        if let Some(dir) = at.parent() {
            fs::create_dir_all(dir).map_err(|e| Fault::Io(e.to_string()))?;
        }
        let tmp = tmp_path(at);
        let text = self.encode();

        // 1. Temporary file.
        fs::write(&tmp, &text).map_err(|e| Fault::Io(e.to_string()))?;

        // 2. Validation — read back what actually landed on disk, not the
        //    string we hoped we wrote, and check it parses to the same
        //    casino. A save that cannot be loaded is worse than no save,
        //    because it is discovered at the moment it is needed.
        let back = fs::read_to_string(&tmp).map_err(|e| Fault::Io(e.to_string()))?;
        let parsed = Save::decode(&back)?;
        if parsed != *self {
            let _ = fs::remove_file(&tmp);
            return Err(Fault::Corrupt("the save did not survive its own round trip".into()));
        }

        // 3. Atomic replacement.
        fs::rename(&tmp, at).map_err(|e| Fault::Io(e.to_string()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------- reading

impl Save {
    /// Parses a save, migrating it forward if it is from an older build.
    pub fn decode(text: &str) -> Result<Save, Fault> {
        let mut s = Save { version: 0, speed: super::config::SPEED_UNIT, ..Save::default() };
        let mut seen_version = false;

        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(Fault::Corrupt(format!("line {} is not key=value", n + 1)));
            };
            let (key, value) = (key.trim(), value.trim());
            let num = |v: &str| -> Result<i64, Fault> {
                v.parse::<i64>().map_err(|_| Fault::Corrupt(format!("line {}: {v:?} is not a number", n + 1)))
            };
            match key {
                "version" => {
                    let v = num(value)? as u32;
                    if v > VERSION {
                        return Err(Fault::FromTheFuture(v));
                    }
                    if v < OLDEST {
                        return Err(Fault::TooOld(v));
                    }
                    s.version = v;
                    seen_version = true;
                }
                _ if !seen_version => return Err(Fault::NoVersion),
                "elapsed_ms" => s.elapsed = Duration::from_millis(num(value)?.max(0) as u64),
                "speed" => s.speed = num(value)?.max(1) as u32,
                "tourneys_held" => s.tourneys_held = num(value)?.max(0) as u32,
                "bank.money" => s.bank.money = num(value)?,
                "bank.chips" => s.bank.chips = num(value)?,
                "bank.collected" => s.bank.collected = num(value)?,
                "bank.paid" => s.bank.paid = num(value)?,
                "bank.bought_in" => s.bank.bought_in = num(value)?,
                "bank.cashed_out" => s.bank.cashed_out = num(value)?,
                "bank.handle" => s.bank.handle = num(value)?,
                "bank.payouts" => s.bank.payouts = num(value)?,
                "bank.rounds" => s.bank.rounds = num(value)?.max(0) as u64,
                "bank.bets" => s.bank.bets = num(value)?.max(0) as u64,
                "bank.spent" => s.bank.spent = num(value)?,
                "table_book" | "expense_book" | "appeal" => {
                    let (k, v) = value
                        .split_once('|')
                        .ok_or_else(|| Fault::Corrupt(format!("line {}: expected key|value", n + 1)))?;
                    let pair = (k.to_string(), num(v)?);
                    match key {
                        "table_book" => s.by_table.push(pair),
                        "expense_book" => s.by_expense.push(pair),
                        _ => s.appeal.push(pair),
                    }
                }
                "opened" => {
                    let (k, v) = value
                        .split_once('|')
                        .ok_or_else(|| Fault::Corrupt(format!("line {}: expected key|value", n + 1)))?;
                    s.opened.push((k.to_string(), num(v)?.max(0) as u32));
                }
                "table" => {
                    let f: Vec<&str> = value.split('|').collect();
                    if f.len() != 3 {
                        return Err(Fault::Corrupt(format!("line {}: a table needs three fields", n + 1)));
                    }
                    let kind = Kind::from_key(f[0])
                        .ok_or_else(|| Fault::Corrupt(format!("line {}: no such game as {:?}", n + 1, f[0])))?;
                    let limit = Limit::from_label(f[2])
                        .ok_or_else(|| Fault::Corrupt(format!("line {}: no such limit as {:?}", n + 1, f[2])))?;
                    s.tables.push(SavedTable { kind, number: num(f[1])?.max(0) as u32, limit });
                }
                "patron" => {
                    let f: Vec<&str> = value.split('|').collect();
                    if f.len() != 14 {
                        return Err(Fault::Corrupt(format!("line {}: a patron needs fourteen fields", n + 1)));
                    }
                    let archetype = Archetype::from_label(f[2])
                        .ok_or_else(|| Fault::Corrupt(format!("line {}: no such sort of person as {:?}", n + 1, f[2])))?;
                    let u8f = |i: usize| -> Result<u8, Fault> {
                        Ok(num(f[i])?.clamp(0, 100) as u8)
                    };
                    s.people.push(SavedPatron {
                        id: num(f[0])?.max(0) as u64,
                        name: f[1].to_string(),
                        archetype,
                        nerve: u8f(3)?,
                        appetite: u8f(4)?,
                        discipline: u8f(5)?,
                        read: u8f(6)?,
                        lifetime: Lifetime {
                            visits: num(f[7])?.max(0) as u32,
                            rounds: num(f[8])?.max(0) as u64,
                            staked: num(f[9])?,
                            returned: num(f[10])?,
                            biggest_win: num(f[11])?,
                            best_visit: num(f[12])?,
                            worst_visit: num(f[13])?,
                        },
                    });
                }
                // An unknown key is not a fault. A save written by a newer
                // build with the same version number should not exist, but
                // a stray line a person added by hand should not stop a
                // casino opening either.
                _ => {}
            }
        }

        if !seen_version {
            return Err(Fault::NoVersion);
        }
        migrate(&mut s);
        Ok(s)
    }

    /// Loads from disk. A missing file is [`Fault::Missing`], which callers
    /// should treat as "a casino that has never been run", not as an error.
    pub fn load(at: &Path) -> Result<Save, Fault> {
        match fs::read_to_string(at) {
            Ok(text) => Save::decode(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Fault::Missing),
            Err(e) => Err(Fault::Io(e.to_string())),
        }
    }
}

/// Brings an older save up to the current version.
///
/// Each step moves one version forward and does the smallest honest thing
/// it can. Where an old file simply did not record something, the
/// migration derives it if it can and leaves it at zero if it cannot —
/// never guesses at a figure that would then look like a real one.
fn migrate(s: &mut Save) {
    if s.version < 2 {
        // Version 1 kept only the house's net per table; it did not record
        // the handle at all. The turnover of every night before this build
        // is genuinely unknown, so it starts from zero rather than being
        // invented — but the win it already knew is preserved, and the
        // handle it accumulates from here on is real.
        if s.bank.handle == 0 && s.bank.payouts == 0 {
            s.bank.handle = s.bank.collected;
            s.bank.payouts = s.bank.paid;
        }
        s.version = 2;
    }
    s.version = VERSION;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: u64, name: &str) -> SavedPatron {
        SavedPatron {
            id,
            name: name.into(),
            archetype: Archetype::Whale,
            nerve: 71,
            appetite: 40,
            discipline: 55,
            read: 66,
            lifetime: Lifetime {
                visits: 12,
                rounds: 4_500,
                staked: 900_000,
                returned: 880_000,
                biggest_win: 41_000,
                best_visit: 12_000,
                worst_visit: -30_000,
            },
        }
    }

    fn a_casino() -> Save {
        Save {
            version: VERSION,
            elapsed: Duration::from_millis(12_345_678),
            speed: 2_000,
            bank: SavedBank {
                money: 271_500,
                chips: 480_250,
                collected: 90_000,
                paid: 84_000,
                bought_in: 40_000,
                cashed_out: 28_500,
                handle: 1_200_000,
                payouts: 1_140_000,
                rounds: 90_000,
                bets: 250_000,
                spent: 12_800,
            },
            by_table: vec![("slots".into(), 4_200), ("roulette".into(), -900)],
            by_expense: vec![("overhead".into(), 8_000), ("staffing".into(), 4_800)],
            people: vec![person(1, "Ada Ashcroft"), person(2, "Bruno Bellweather")],
            tables: vec![
                SavedTable { kind: Kind::Slots, number: 3, limit: Limit::House },
                SavedTable { kind: Kind::Baccarat, number: 1, limit: Limit::High },
            ],
            appeal: vec![("slots".into(), 1_150), ("keno".into(), 820)],
            opened: vec![("slots".into(), 9), ("baccarat".into(), 2)],
            tourneys_held: 4,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dice_arena_save_test_{name}"))
    }

    #[test]
    fn a_casino_survives_being_written_down_and_read_back() {
        let before = a_casino();
        let after = Save::decode(&before.encode()).expect("it should parse");
        assert_eq!(after, before, "something was lost on the round trip");
    }

    #[test]
    fn every_person_comes_back_exactly_as_they_went_in() {
        let before = a_casino();
        let after = Save::decode(&before.encode()).unwrap();
        assert_eq!(after.people.len(), 2);
        for (a, b) in after.people.iter().zip(before.people.iter()) {
            assert_eq!(a, b, "{} came back different", b.name);
            assert_eq!(a.lifetime.worst_visit, b.lifetime.worst_visit, "a negative figure was mangled");
        }
        // Names with spaces are fine; the delimiter is a pipe.
        assert!(after.people.iter().any(|p| p.name.contains(' ')));
    }

    #[test]
    fn the_tables_come_back_with_their_limits() {
        let after = Save::decode(&a_casino().encode()).unwrap();
        assert_eq!(after.tables.len(), 2);
        assert_eq!(after.tables[1].kind, Kind::Baccarat);
        assert_eq!(after.tables[1].limit, Limit::High, "a high-limit room reopened as an ordinary one");
        assert_eq!(after.tables[1].number, 1);
    }

    #[test]
    fn a_file_from_the_future_is_refused_rather_than_guessed_at() {
        let text = format!("version={}\nbank.money=10\n", VERSION + 1);
        assert_eq!(Save::decode(&text), Err(Fault::FromTheFuture(VERSION + 1)));
        // ...and one from before the format was worth keeping.
        let ancient = format!("version={}\n", OLDEST - 1);
        assert_eq!(Save::decode(&ancient), Err(Fault::TooOld(OLDEST - 1)));
    }

    #[test]
    fn a_file_that_does_not_say_what_it_is_is_refused() {
        assert_eq!(Save::decode("bank.money=10\n"), Err(Fault::NoVersion));
        assert_eq!(Save::decode(""), Err(Fault::NoVersion));
        assert_eq!(Save::decode("# just a comment\n"), Err(Fault::NoVersion));
    }

    #[test]
    fn damage_is_reported_rather_than_silently_half_loaded() {
        let cases = [
            "version=2\nthis line has no equals sign\n",
            "version=2\nbank.money=not a number\n",
            "version=2\ntable=slots|3\n",
            "version=2\ntable=quoits|3|house\n",
            "version=2\ntable=slots|3|velvet\n",
            "version=2\npatron=1|Ada|whale|1|2|3\n",
            "version=2\npatron=1|Ada|card counter|1|2|3|4|5|6|7|8|9|10|11\n",
        ];
        for text in cases {
            let got = Save::decode(text);
            assert!(matches!(got, Err(Fault::Corrupt(_))), "{text:?} was accepted, giving {got:?}");
            assert!(!got.unwrap_err().describe().is_empty());
        }
    }

    #[test]
    fn a_line_nobody_recognises_does_not_stop_the_doors_opening() {
        // Somebody editing the file by hand, or a key from a build that
        // came and went. A stray line is not worth refusing a casino over.
        let mut text = a_casino().encode();
        text.push_str("some_key_from_nowhere=42\n");
        let after = Save::decode(&text).expect("a stray line should be ignored");
        assert_eq!(after.bank.money, a_casino().bank.money);
    }

    #[test]
    fn an_old_save_is_migrated_rather_than_refused() {
        // Version 1 did not record the handle. Migration derives what it
        // honestly can from what version 1 did keep, and says so.
        let v1 = "version=1\nbank.money=1000\nbank.chips=5000\nbank.collected=900\nbank.paid=700\n";
        let s = Save::decode(v1).expect("a version 1 file should still load");
        assert_eq!(s.version, VERSION, "it was not brought up to date");
        assert_eq!(s.bank.money, 1_000, "what version 1 did know must survive");
        assert_eq!(s.bank.handle, 900, "the handle should be derived from what was recorded");
        assert_eq!(s.bank.payouts, 700);
        assert_eq!(s.bank.handle - s.bank.payouts, 900 - 700, "the win it already knew must not change");
    }

    #[test]
    fn migration_does_not_overwrite_figures_a_newer_file_already_has() {
        let mut s = a_casino();
        s.version = 1;
        let text = s.encode();
        let after = Save::decode(&text).unwrap();
        assert_eq!(after.bank.handle, 1_200_000, "migration clobbered a handle that was already there");
        assert_eq!(after.version, VERSION);
    }

    #[test]
    fn writing_leaves_no_temporary_file_behind_and_the_save_loads() {
        let at = scratch("clean");
        let _ = fs::remove_file(&at);
        let save = a_casino();
        save.write(&at).expect("it should write");
        assert!(at.exists(), "the save is not there");
        assert!(!tmp_path(&at).exists(), "a temporary file was left lying about");
        let back = Save::load(&at).expect("it should load");
        assert_eq!(back, save);
        let _ = fs::remove_file(&at);
    }

    #[test]
    fn a_casino_that_has_never_run_is_missing_and_not_an_error() {
        let at = scratch("never_run");
        let _ = fs::remove_file(&at);
        assert_eq!(Save::load(&at), Err(Fault::Missing));
        assert_eq!(Fault::Missing.describe(), "no casino has been saved yet");
    }

    #[test]
    fn a_second_save_replaces_the_first_whole() {
        let at = scratch("replace");
        let _ = fs::remove_file(&at);
        let mut first = a_casino();
        first.write(&at).unwrap();
        let mut second = a_casino();
        second.bank.money = 999_999;
        second.people.push(person(3, "Cleo Crane"));
        second.write(&at).unwrap();
        let back = Save::load(&at).unwrap();
        assert_eq!(back.bank.money, 999_999);
        assert_eq!(back.people.len(), 3);
        first.bank.money = 999_999;
        assert_ne!(back, first, "the two saves should differ by more than the money");
        let _ = fs::remove_file(&at);
    }

    #[test]
    fn an_unwritable_place_fails_without_destroying_what_is_there() {
        // The property that matters: a failed save must leave the last good
        // one exactly as it was.
        let at = scratch("kept");
        let _ = fs::remove_file(&at);
        let good = a_casino();
        good.write(&at).unwrap();

        // A directory where the temporary file wants to go: the write
        // cannot succeed, and must not take the good save with it.
        let tmp = tmp_path(&at);
        let _ = fs::remove_file(&tmp);
        fs::create_dir_all(&tmp).expect("stand a directory in the way");
        let mut doomed = a_casino();
        doomed.bank.money = 1;
        assert!(doomed.write(&at).is_err(), "writing over a directory should not have worked");
        assert_eq!(Save::load(&at).unwrap(), good, "the good save was damaged by a failed one");

        let _ = fs::remove_dir_all(&tmp);
        let _ = fs::remove_file(&at);
    }

    #[test]
    fn the_file_is_something_a_person_could_read_and_repair() {
        let text = a_casino().encode();
        assert!(text.starts_with("version="), "the version must be the first thing in the file");
        assert!(text.contains("# dice_arena casino"), "a file with no explanation in it");
        assert!(text.contains("bank.money=271500"));
        assert!(text.contains("Ada Ashcroft"));
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            assert!(line.contains('='), "line {line:?} is not readable as a setting");
        }
    }
}
