//! Dice Lab — free rolling with `NdM+K` notation and a distribution sampler.
//! The one screen in the app that still takes typed text — there's no
//! sensible way to "press a key" for an arbitrary expression like `4d10+2`.

use super::Ctx;
use crate::dice::{self, Notation};
use crate::rng::Rng;
use crate::ui::{self, dice_art, widgets};

pub fn play(ctx: &mut Ctx) {
    loop {
        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "DICE LAB");
        ctx.screen.blank();
        ctx.screen.line(&format!("  roll anything: {}", theme.paint(ui::theme::CYAN, "3d6, d20, 4d10+2")));
        ctx.screen.line(&theme.dim("  sim <expr> <n> samples a distribution · seed <n> pins the RNG · Esc to leave"));
        ctx.screen.blank();
        let input = widgets::text_input(ctx.screen, "", 40, |s, buf| {
            let theme = s.theme;
            s.begin();
            ui::header(s, "DICE LAB");
            s.blank();
            s.line(&format!("  roll anything: {}", theme.paint(ui::theme::CYAN, "3d6, d20, 4d10+2")));
            s.line(&theme.dim("  sim <expr> <n> samples a distribution · seed <n> pins the RNG · Esc to leave"));
            s.blank();
            s.line(&format!("  roll> {buf}█"));
        });
        if ui::quit_requested() {
            return;
        }
        let lower = input.to_lowercase();
        if lower.is_empty() {
            continue;
        }

        let mut words = lower.split_whitespace();
        match words.next() {
            Some("seed") => match words.next().and_then(|w| w.parse::<u64>().ok()) {
                Some(seed) => {
                    *ctx.rng = Rng::from_seed(seed);
                    flash(ctx, &theme.win(&format!("rng reseeded to {seed} (reproducible)")));
                }
                None => flash(ctx, "usage: seed 12345"),
            },
            Some("sim") => {
                let expr = words.next().unwrap_or("");
                let n: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(10_000);
                match Notation::parse(expr) {
                    Ok(spec) => simulate(ctx, spec, n.clamp(10, 1_000_000)),
                    Err(e) => flash(ctx, &format!("! {e}")),
                }
            }
            _ => match Notation::parse(&lower) {
                Ok(spec) => single_roll(ctx, spec),
                Err(e) => flash(ctx, &format!("! {e}  (try 3d6 or 2d10+1)")),
            },
        }
    }
}

fn flash(ctx: &mut Ctx, msg: &str) {
    ctx.screen.blank();
    ctx.screen.line(&format!("  {msg}"));
    ctx.screen.present();
    ui::sleep_ms(900);
}

fn single_roll(ctx: &mut Ctx, spec: Notation) {
    let animated = spec.sides == 6 && spec.count <= 8;
    let theme = ctx.theme();
    let values = if animated {
        let seed = vec![1u32; spec.count as usize];
        let held = vec![false; spec.count as usize];
        dice_art::animate_roll(ctx.screen, ctx.rng, spec.sides, &seed, &held, |screen, values| {
            screen.begin();
            ui::header(screen, "DICE LAB");
            screen.blank();
            for line in dice_art::dice_block(&theme, values, None) {
                screen.line(&line);
            }
        })
    } else {
        dice::roll_n(ctx.rng, spec.count, spec.sides)
    };
    ctx.store.bump("lab.rolls", spec.count as i64);

    let total = dice::total(&values, spec.modifier);
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "DICE LAB");
    ctx.screen.blank();
    if animated {
        for line in dice_art::dice_block(&theme, &values, None) {
            ctx.screen.line(&line);
        }
    } else {
        let list: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        ctx.screen.line(&format!("  [{}]", theme.paint(ui::theme::YELLOW, &list.join(", "))));
    }
    let modtxt = match spec.modifier {
        0 => String::new(),
        m if m > 0 => format!(" (+{m})"),
        m => format!(" ({m})"),
    };
    ctx.screen.blank();
    ctx.screen.line(&format!(
        "  total: {}{}   min {} · max {}",
        theme.bold(&total.to_string()),
        theme.dim(&modtxt),
        values.iter().min().copied().unwrap_or(0),
        values.iter().max().copied().unwrap_or(0)
    ));
    ctx.store.record_best("lab.best_total", total);
    let _ = ctx.store.save();
    ctx.screen.present();
    ui::sleep_ms(1400);
}

fn simulate(ctx: &mut Ctx, spec: Notation, n: usize) {
    let theme = ctx.theme();
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

    ctx.screen.begin();
    ui::header(ctx.screen, "DICE LAB — distribution");
    ctx.screen.blank();
    let modtxt = if spec.modifier == 0 { String::new() } else { format!("{:+}", spec.modifier) };
    ctx.screen.line(&format!("  {n} samples of {}d{}{modtxt} — mean {mean:.3} (theory {expected:.3})", spec.count, spec.sides));
    ctx.screen.blank();

    let (_, term_rows) = ctx.screen.size();
    let max_rows = (term_rows as usize).saturating_sub(10).max(6);
    let peak = counts.iter().copied().max().unwrap_or(1).max(1);
    let step = (span + max_rows - 1) / max_rows.max(1);
    let mut i = 0;
    while i < span {
        let bucket_count: usize = counts[i..(i + step).min(span)].iter().sum();
        let value_lo = lo + i as i64;
        let value_hi = lo + (i + step).min(span) as i64 - 1;
        let width = (bucket_count as f64 / (peak * step) as f64 * 40.0).round() as usize;
        let pct = bucket_count as f64 / n as f64 * 100.0;
        let label = if step == 1 { format!("{value_lo:>4}") } else { format!("{value_lo:>4}-{value_hi:<4}") };
        ctx.screen.line(&format!("   {label} {} {:>5.2}%", theme.paint(ui::theme::CYAN, &"▇".repeat(width.max(if bucket_count > 0 { 1 } else { 0 }))), pct));
        i += step;
    }
    ctx.screen.present();
    ui::pause(ctx.screen);
}
