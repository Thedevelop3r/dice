//! Dice Lab — free rolling with `NdM+K` notation and a distribution sampler.

use super::Ctx;
use crate::dice::{self, Notation};
use crate::rng::Rng;
use crate::ui::{self, *};

pub fn play(ctx: &mut Ctx) {
    ui::header(ctx.colors, "DICE LAB");
    println!("  Roll anything: {}", color(ctx.colors, CYAN, "3d6, d20, 4d10+2"));
    println!("  {}", color(ctx.colors, DIM, "commands: sim <expr> <n> — sample a distribution | seed <n> | q — back"));

    loop {
        let input = ui::prompt("\n  roll> ");
        let lower = input.to_lowercase();
        if lower.is_empty() {
            continue;
        }
        if lower == "q" || lower == "quit" || lower == "back" {
            break;
        }

        let mut words = lower.split_whitespace();
        match words.next() {
            Some("seed") => {
                match words.next().and_then(|w| w.parse::<u64>().ok()) {
                    Some(seed) => {
                        *ctx.rng = Rng::from_seed(seed);
                        println!("  {}", color(ctx.colors, GREEN, &format!("rng reseeded to {seed} (reproducible)")));
                    }
                    None => println!("  ! usage: seed 12345"),
                }
            }
            Some("sim") => {
                let expr = words.next().unwrap_or("");
                let n: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(10_000);
                match Notation::parse(expr) {
                    Ok(spec) => simulate(ctx, spec, n.clamp(10, 1_000_000)),
                    Err(e) => println!("  ! {e}"),
                }
            }
            _ => match Notation::parse(&lower) {
                Ok(spec) => single_roll(ctx, spec),
                Err(e) => println!("  ! {e}  (try 3d6 or 2d10+1)"),
            },
        }
    }
}

fn single_roll(ctx: &mut Ctx, spec: Notation) {
    let values = dice::roll_n(ctx.rng, spec.count, spec.sides);
    ctx.store.bump("lab.rolls", spec.count as i64);
    if spec.sides == 6 && spec.count <= 8 {
        ui::draw_dice(&values, None, ctx.colors);
    } else {
        let list: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        println!("  [{}]", color(ctx.colors, YELLOW, &list.join(", ")));
    }
    let total = dice::total(&values, spec.modifier);
    let modtxt = match spec.modifier {
        0 => String::new(),
        m if m > 0 => format!(" (+{m})"),
        m => format!(" ({m})"),
    };
    println!(
        "  total: {}{}   min {} · max {}",
        color(ctx.colors, BOLD, &total.to_string()),
        color(ctx.colors, DIM, &modtxt),
        values.iter().min().copied().unwrap_or(0),
        values.iter().max().copied().unwrap_or(0)
    );
    ctx.store.record_best("lab.best_total", total);
    let _ = ctx.store.save();
}

fn simulate(ctx: &mut Ctx, spec: Notation, n: usize) {
    let lo = spec.count as i64 + spec.modifier;
    let hi = (spec.count * spec.sides) as i64 + spec.modifier;
    let span = (hi - lo + 1) as usize;
    let mut counts = vec![0usize; span];
    let mut sum_total: i64 = 0;

    ctx.store.bump("lab.simulated_rolls", (n as i64) * spec.count as i64);
    let _ = ctx.store.save();
    for _ in 0..n {
        let t = dice::total(&dice::roll_n(ctx.rng, spec.count, spec.sides), spec.modifier);
        counts[(t - lo) as usize] += 1;
        sum_total += t;
    }

    let mean = sum_total as f64 / n as f64;
    let expected = spec.count as f64 * (spec.sides as f64 + 1.0) / 2.0 + spec.modifier as f64;
    println!();
    let modtxt = if spec.modifier == 0 { String::new() } else { format!("{:+}", spec.modifier) };
    println!(
        "  {} samples of {}d{}{} — mean {:.3} (theory {:.3})",
        n, spec.count, spec.sides, modtxt, mean, expected
    );

    let peak = counts.iter().copied().max().unwrap_or(1).max(1);
    for (i, c) in counts.iter().enumerate() {
        let value = lo + i as i64;
        let width = (*c as f64 / peak as f64 * 40.0).round() as usize;
        let pct = *c as f64 / n as f64 * 100.0;
        println!(
            "   {:>4} {} {:>5.2}%",
            value,
            color(ctx.colors, CYAN, &"▇".repeat(width)),
            pct
        );
    }
}
