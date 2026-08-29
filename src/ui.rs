//! Terminal helpers: color, prompts, ASCII dice art.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once stdin closes, so retry loops can bail instead of spinning.
static EOF: AtomicBool = AtomicBool::new(false);

pub fn eof_reached() -> bool {
    EOF.load(Ordering::Relaxed)
}

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";

pub fn color(on: bool, code: &str, text: &str) -> String {
    if on { format!("{code}{text}{RESET}") } else { text.to_string() }
}

pub fn banner(colors: bool) {
    let art = r#"
   ____  _         ___
  |  _ \(_)_______/   |  _ __ ___ _ __   __ _
  | | | | |/ __/ _ \ /| | | '__/ _ \ '_ \ / _` |
  | |_| | | (_|  __/ | | | | |  __/ | | | (_| |
  |____/|_|\___\___| |_|_|_|  \___|_| |_|\__,_|
"#;
    println!("{}", color(colors, CYAN, art));
    println!("{}\n", color(colors, DIM, "        a dice suite for the terminal"));
}

pub fn rule(colors: bool, width: usize) {
    println!("{}", color(colors, DIM, &"─".repeat(width)));
}

pub fn header(colors: bool, title: &str) {
    println!();
    println!("{}", color(colors, BOLD, title));
    rule(colors, title.chars().count().max(24));
}

/// Six-sided pip faces. Values outside 1..=6 fall back to a numeric box.
fn face_lines(v: u32) -> [String; 5] {
    let pips: [[&str; 3]; 6] = [
        ["     ", "  ●  ", "     "],
        ["●    ", "     ", "    ●"],
        ["●    ", "  ●  ", "    ●"],
        ["●   ●", "     ", "●   ●"],
        ["●   ●", "  ●  ", "●   ●"],
        ["●   ●", "●   ●", "●   ●"],
    ];
    if (1..=6).contains(&v) {
        let p = pips[(v - 1) as usize];
        [
            "┌───────┐".to_string(),
            format!("│ {} │", p[0]),
            format!("│ {} │", p[1]),
            format!("│ {} │", p[2]),
            "└───────┘".to_string(),
        ]
    } else {
        [
            "┌───────┐".to_string(),
            "│       │".to_string(),
            format!("│{:^7}│", v),
            "│       │".to_string(),
            "└───────┘".to_string(),
        ]
    }
}

/// Render dice side by side. `held` marks dice to highlight (Yahtzee keeps).
pub fn draw_dice(values: &[u32], held: Option<&[bool]>, colors: bool) {
    if values.is_empty() {
        return;
    }
    let faces: Vec<[String; 5]> = values.iter().map(|v| face_lines(*v)).collect();
    for row in 0..5 {
        let mut line = String::new();
        for (i, f) in faces.iter().enumerate() {
            let is_held = held.map_or(false, |h| h.get(i).copied().unwrap_or(false));
            let code = if is_held { GREEN } else { YELLOW };
            line.push_str(&color(colors, code, &f[row]));
            line.push(' ');
        }
        println!("{line}");
    }
    let mut labels = String::new();
    for (i, _) in faces.iter().enumerate() {
        let is_held = held.map_or(false, |h| h.get(i).copied().unwrap_or(false));
        let tag = if is_held { format!("[{}] HELD", i + 1) } else { format!("[{}]     ", i + 1) };
        labels.push_str(&format!("{:<10}", tag));
    }
    if held.is_some() {
        println!("{}", color(colors, DIM, &labels));
    }
}

/// Draws a labelled row of dice (the Luck Bet table). Returns the number of
/// lines printed so an animation can rewind the cursor over them.
pub fn draw_table(values: &[u32], labels: &[char], hits: &[bool], colors: bool, gold: bool) -> usize {
    let faces: Vec<[String; 5]> = values.iter().map(|v| face_lines(*v)).collect();
    let base = if gold { YELLOW } else { CYAN };
    for row in 0..5 {
        let mut line = String::new();
        for (i, f) in faces.iter().enumerate() {
            let code = if hits.get(i).copied().unwrap_or(false) { GREEN } else { base };
            line.push_str(&color(colors, code, &f[row]));
            line.push(' ');
        }
        println!("{line}");
    }
    let mut tags = String::new();
    for (i, _) in faces.iter().enumerate() {
        let letter = labels.get(i).copied().unwrap_or('?');
        let hit = hits.get(i).copied().unwrap_or(false);
        let cell = format!("{:^9}", letter);
        tags.push_str(&color(colors, if hit { GREEN } else { BOLD }, &cell));
        tags.push(' ');
    }
    println!("{tags}");
    6
}

pub fn cursor_up(lines: usize) {
    print!("\x1b[{lines}A");
    let _ = io::stdout().flush();
}

pub fn hide_cursor() {
    print!("\x1b[?25l");
    let _ = io::stdout().flush();
}

pub fn show_cursor() {
    print!("\x1b[?25h");
    let _ = io::stdout().flush();
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

pub fn money(colors: bool, chips: i64, dollars: i64) -> String {
    format!(
        "{}  {}",
        color(colors, YELLOW, &format!("{chips} chips")),
        color(colors, GREEN, &format!("${dollars}"))
    )
}

pub fn prompt(msg: &str) -> String {
    print!("{msg}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let n = io::stdin().lock().read_line(&mut line).unwrap_or(0);
    if n == 0 {
        // EOF: treat as quit so piped input never loops forever.
        EOF.store(true, Ordering::Relaxed);
        println!();
        return "q".to_string();
    }
    line.trim().to_string()
}

pub fn prompt_usize(msg: &str, min: usize, max: usize, default: usize) -> usize {
    loop {
        let s = prompt(&format!("{msg} [{default}]: "));
        if s.is_empty() || eof_reached() {
            return default;
        }
        match s.parse::<usize>() {
            Ok(v) if v >= min && v <= max => return v,
            _ => println!("  ! enter a number between {min} and {max}"),
        }
    }
}

pub fn confirm(msg: &str) -> bool {
    let s = prompt(&format!("{msg} (y/n): ")).to_lowercase();
    s.starts_with('y')
}

pub fn pause() {
    let _ = prompt("\n  press Enter to continue...");
}

pub fn clear() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}
