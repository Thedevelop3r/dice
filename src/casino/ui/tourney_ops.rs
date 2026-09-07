//! Running a tournament from the operator's side.
//!
//! Three screens: setting one up, watching one play down, and reading what
//! it did afterwards. All three obey the same rule as every other screen
//! in this module — they draw snapshots and they never advance anything.
//!
//! The one place this touches the simulation is the last step of the setup
//! screen, and it touches it exactly once: it hands the manager a game, a
//! field size, a buy-in and a stack, and the manager does the rest on the
//! simulation thread. Everything after that — finding the runners, taking
//! their money at the cage, booking the house's cut, playing the rounds,
//! paying the pool out — happens inside the systems that already did all
//! of those things for the tournaments the floor starts by itself. There
//! is no second tournament engine here, and no screen in this file has
//! ever moved a chip.
//!
//! Leaving any of these screens does nothing to the tournament. It carries
//! on playing itself down while nobody is looking, which is the entire
//! point of the thing.

use super::super::dashboard::TournamentView;
use super::super::instance::Kind;
use super::super::manager::Manager;
use super::super::tournament::Stage;
use super::{duration, poll, signed, REFRESH};
use crate::ui::{self, clip, pad_end, pad_start, thousands, widgets, Poll, Screen};

/// Sets a tournament up and starts it.
///
/// The operator chooses the game, the size of the field, what it costs to
/// enter and what everybody starts with. The rake and the prize ladder are
/// deliberately *not* here: those are house policy and they live in
/// configuration, where a screen can read them but not invent them.
pub fn start(manager: &Manager, screen: &mut Screen) {
    let cfg = manager.config();
    let theme = screen.theme;

    // 1. Which game. Only the kinds the floor is actually running are
    //    offered, because a tournament borrows the same maths its cash
    //    tables use and a game nobody is playing is a strange thing to
    //    hold one in.
    let view = manager.dashboard();
    let mut kinds: Vec<Kind> = view.games.iter().map(|g| g.kind).collect();
    kinds.sort_by_key(|k| k.label());
    kinds.dedup();
    if kinds.is_empty() {
        kinds = Kind::ALL.to_vec();
    }
    let key_for = |i: usize| -> char {
        if i < 9 { char::from_digit(i as u32 + 1, 10).unwrap_or('1') } else { (b'a' + (i - 9) as u8) as char }
    };

    screen.begin();
    ui::header(screen, "START A TOURNAMENT");
    screen.blank();
    screen.line("  a fixed field, playing itself down to one winner. it runs on the");
    screen.line("  simulation thread, so it carries on whether or not you watch it.");
    screen.blank();
    for (i, k) in kinds.iter().enumerate() {
        screen.line(&format!(
            "   {}  {}",
            theme.paint(ui::theme::GOLD, &format!("[{}]", key_for(i))),
            theme.accent(k.label())
        ));
    }
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('1', "pick a game"), ('q', "not tonight")]));
    screen.present();

    let mut valid: Vec<char> = (0..kinds.len()).map(key_for).collect();
    valid.push('q');
    let Some(choice) = ui::choose_key(&valid, 'q') else { return };
    let Some(at) = valid.iter().position(|c| *c == choice) else { return };
    if at >= kinds.len() {
        return;
    }
    let kind = kinds[at];

    // 2. How many runners. The floor of the range is the configured
    //    minimum field — below that a tournament cannot be held at all.
    let Some(runners) = pick(
        screen,
        "HOW MANY RUNNERS",
        &format!("{} — a field is filled from the people the casino knows", kind.label()),
        cfg.tourney_min_field as i64,
        256,
        32,
        8,
        &[('s', 16), ('m', 64), ('l', 128), ('x', 256)],
        "runners",
    ) else {
        return;
    };

    // 3. What it costs to enter, and 4. what everybody starts with.
    let Some(buy_in) = pick(
        screen,
        "ENTRY FEE",
        &format!("in chips · the house takes {}% of every entry", cfg.tourney_rake),
        cfg.min_bet,
        cfg.tourney_buy_in * 20,
        cfg.tourney_buy_in,
        cfg.tourney_buy_in / 4,
        &[('h', cfg.tourney_buy_in), ('b', cfg.tourney_buy_in * 5)],
        "chips",
    ) else {
        return;
    };
    let Some(stack) = pick(
        screen,
        "STARTING STACK",
        "tournament chips · a score to play down, not money",
        cfg.tourney_ante * 10,
        cfg.tourney_stack * 20,
        cfg.tourney_stack,
        cfg.tourney_stack / 4,
        &[('h', cfg.tourney_stack), ('d', cfg.tourney_stack * 2)],
        "chips",
    ) else {
        return;
    };

    // What it comes to, before anybody is charged anything. The rake is
    // the configured percentage — this only multiplies it out.
    let rake_each = buy_in * cfg.tourney_rake / 100;
    let pool = (buy_in - rake_each) * runners;
    let house = rake_each * runners;
    screen.begin();
    ui::header(screen, "START A TOURNAMENT");
    screen.blank();
    screen.line(&format!("  game            {}", theme.accent(kind.label())));
    screen.line(&format!("  runners         {}", theme.accent(&runners.to_string())));
    screen.line(&format!("  entry           {} chips", theme.paint(ui::theme::GOLD, &thousands(buy_in))));
    screen.line(&format!("  starting stack  {}", theme.dim(&thousands(stack))));
    screen.blank();
    screen.line(&format!(
        "  {} × {} entries",
        theme.dim(&runners.to_string()),
        theme.dim(&thousands(buy_in))
    ));
    screen.line(&format!("  prize pool      {}", theme.win(&thousands(pool))));
    screen.line(&format!("  house takes     {} ({}%)", theme.paint(ui::theme::GOLD, &thousands(house)), cfg.tourney_rake));
    screen.blank();
    screen.line(&theme.dim(
        "  runners are drawn from the people the casino already knows. if the",
    ));
    screen.line(&theme.dim(
        "  building cannot field enough, more are minted into the roster proper —",
    ));
    screen.line(&theme.dim("  they keep their ids and their records afterwards."));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('y', "start it"), ('n', "cancel")]));
    screen.present();
    if !ui::confirm_key(false) {
        return;
    }

    // The one call that touches the floor. Everything past here belongs to
    // the simulation thread.
    let started = manager.start_tournament(kind, runners as usize, buy_in, stack);
    screen.begin();
    ui::header(screen, "START A TOURNAMENT");
    screen.blank();
    match started {
        Some(id) => {
            screen.line(&format!("  {}", theme.win("under way.")));
            screen.blank();
            screen.line(&theme.dim("  it plays itself down from here. the dashboard will show it."));
            screen.present();
            ui::sleep_ms(700);
            overview(manager, screen, id);
        }
        None => {
            screen.line(&format!("  {}", theme.lose("it could not be fielded.")));
            screen.blank();
            screen.line(&theme.dim(
                "  not enough people could be found or afford the entry. nobody was",
            ));
            screen.line(&theme.dim("  charged anything — every chip taken has been handed back."));
            screen.present();
            ui::pause(screen);
        }
    }
}

/// A step through `widgets::number_picker` with this module's framing, so
/// the four setup questions read the same way.
#[allow(clippy::too_many_arguments)]
fn pick(
    screen: &mut Screen,
    title: &str,
    blurb: &str,
    min: i64,
    max: i64,
    default: i64,
    step: i64,
    presets: &[(char, i64)],
    unit: &str,
) -> Option<i64> {
    let hints: Vec<(char, &str)> = vec![('↑', "more"), ('↓', "less"), ('\r', "confirm"), ('q', "cancel")];
    widgets::number_picker(screen, min, max, default, step.max(1), presets, |s, value| {
        let theme = s.theme;
        s.begin();
        ui::header(s, title);
        s.blank();
        s.line(&format!("  {}", theme.dim(blurb)));
        s.blank();
        s.line(&format!("  > {} {}", theme.paint(ui::theme::GOLD, &thousands(value)), theme.dim(unit)));
        s.blank();
        s.line(&widgets::footer(&theme, &hints));
    })
}

/// One tournament, live — and once it is over, what it did.
///
/// Reads the tournament system directly by id every frame, so what is on
/// screen is the field as it stands rather than the field as it was when
/// this screen opened.
pub fn overview(manager: &Manager, screen: &mut Screen, id: u32) {
    let mut show_players = false;
    loop {
        let view = manager.dashboard();
        let Some(t) = view.tournaments.iter().find(|t| t.id == id) else {
            // Finished tournaments are kept for a while and then let go.
            // When one goes, so does this screen.
            return;
        };
        if t.finished() {
            complete(screen, t);
        } else if show_players {
            players(manager, screen, t);
        } else {
            running(screen, t);
        }
        screen.present();

        match poll(REFRESH, &['p', 's', '\r', '\n']) {
            Poll::Leave => return,
            Poll::Pressed('p') | Poll::Pressed('\r') | Poll::Pressed('\n') => show_players = !show_players,
            // Spectating a tournament means watching the floor it is
            // being played on — the same spectator screen, unchanged.
            Poll::Pressed('s') => {
                if let Some((table, _)) = manager.most_interesting() {
                    super::spectate(manager, screen, table);
                }
            }
            _ => {}
        }
    }
}

/// A tournament still playing itself down.
fn running(screen: &mut Screen, t: &TournamentView) {
    let theme = screen.theme;
    let (cols, rows) = screen.size();
    let width = (cols as usize).max(40);
    screen.begin();
    ui::header(screen, &format!("TOURNAMENT T-{:03}", t.id));
    screen.blank();
    screen.line(&clip(&format!("  {}", theme.paint(ui::theme::MAGENTA, &t.name)), width));
    screen.blank();
    screen.line(&clip(
        &format!(
            "  game {} · {} · running {}",
            theme.accent(t.kind.label()),
            theme.paint(ui::theme::GOLD, &t.stage.label()),
            theme.dim(&duration(t.elapsed))
        ),
        width,
    ));
    screen.line(&clip(
        &format!(
            "  {} still in of {} · {} knocked out · round {}",
            theme.accent(&t.alive.to_string()),
            theme.dim(&t.entered.to_string()),
            theme.dim(&t.eliminated().to_string()),
            theme.dim(&t.round.to_string())
        ),
        width,
    ));
    screen.line(&clip(
        &format!(
            "  prize pool {} · entry {} · house took {}",
            theme.win(&thousands(t.pool)),
            theme.dim(&thousands(t.buy_in)),
            theme.paint(ui::theme::GOLD, &thousands(t.rake))
        ),
        width,
    ));
    screen.blank();
    screen.line(&theme.dim(&format!(
        "  stacks — biggest {} · average {} · shortest {}",
        thousands(t.biggest_stack),
        thousands(t.average_stack),
        thousands(t.smallest_stack)
    )));
    screen.blank();

    screen.line(&theme.dim(&format!("  {} {} {}", pad_end("  RANK", 8), pad_end("RUNNER", 26), pad_start("STACK", 12))));
    let room = (rows as usize).saturating_sub(16).max(4);
    for (i, e) in t.board.iter().take(room).enumerate() {
        let rank = format!("{}.", i + 1);
        let name = if i == 0 { theme.paint(ui::theme::GOLD, &pad_end(&e.name, 26)) } else { pad_end(&e.name, 26) };
        screen.line(&clip(
            &format!("  {} {} {}", pad_end(&rank, 8), name, pad_start(&thousands(e.stack), 12)),
            width,
        ));
    }
    if t.alive > t.board.len() {
        screen.line(&theme.dim(&format!("     … and {} more still in", t.alive - t.board.len())));
    }
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('p', "runners"), ('s', "spectate the floor"), ('q', "back")]));
}

/// Everybody in the field, with what the casino knows about them.
///
/// The tournament's own figures — stack, the round they went out in, the
/// prize they took — come off the tournament. Everything else comes off
/// the roster. Nothing on this screen is worked out from anything else.
fn players(manager: &Manager, screen: &mut Screen, t: &TournamentView) {
    let theme = screen.theme;
    let (cols, rows) = screen.size();
    let width = (cols as usize).max(40);
    screen.begin();
    ui::header(screen, &format!("T-{:03} — THE FIELD", t.id));
    screen.blank();
    screen.line(&clip(
        &theme.dim(&format!(
            "  {} entered · {} still in · {} knocked out · average stack {}",
            t.entered,
            t.alive,
            t.eliminated(),
            thousands(t.average_stack)
        )),
        width,
    ));
    screen.blank();
    screen.line(&theme.dim(&format!(
        "  {} {} {} {} {}",
        pad_end("  RUNNER", 24),
        pad_start("STACK", 11),
        pad_start("OUT IN", 8),
        pad_start("VISITS", 7),
        pad_start("CAREER", 12)
    )));

    // The whole field, off the tournament system itself — everybody who
    // ever entered, not just whoever is still standing. Ordered the way it
    // will finish: alive first by stack, then by how late they went out.
    let mut field = manager
        .tourney(t.id)
        .map(|full| full.field.clone())
        .unwrap_or_else(|| t.board.clone());
    field.sort_by(|a, b| {
        let rank = |e: &super::super::tournament::Entrant| e.out_in.unwrap_or(usize::MAX);
        rank(b).cmp(&rank(a)).then(b.stack.cmp(&a.stack)).then(a.patron.cmp(&b.patron))
    });

    let room = (rows as usize).saturating_sub(12).max(4);
    for (i, e) in field.iter().take(room).enumerate() {
        // Looked up by id in the roster the casino has always had. A
        // tournament entrant is a real person in this building, not a
        // stand-in that exists for the length of the event.
        let p = manager.patron(e.patron);
        let rank = format!("{}.", i + 1);
        let name = match e.out_in {
            Some(_) => theme.dim(&pad_end(&e.name, 19)),
            None => pad_end(&e.name, 19),
        };
        screen.line(&clip(
            &format!(
                "  {} {} {} {} {} {}",
                pad_end(&rank, 4),
                name,
                pad_start(&thousands(e.stack), 11),
                pad_start(
                    &match e.out_in {
                        Some(round) => theme.lose(&format!("round {round}")),
                        None => theme.win("still in"),
                    },
                    8
                ),
                pad_start(&p.as_ref().map(|p| p.lifetime.visits.to_string()).unwrap_or_else(|| "—".into()), 7),
                pad_start(
                    &p.as_ref().map(|p| signed(&theme, p.lifetime.net())).unwrap_or_else(|| theme.dim("—")),
                    12
                )
            ),
            width,
        ));
    }
    if field.len() > room {
        screen.line(&theme.dim(&format!("     … {} of {} runners", room, field.len())));
    }
    screen.blank();
    screen.line(&theme.dim(
        "  stack is this tournament's score; career is what they are up or down",
    ));
    screen.line(&theme.dim("  across every visit they have ever made to this building."));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('p', "standings"), ('s', "spectate the floor"), ('q', "back")]));
}

/// What a finished tournament did.
fn complete(screen: &mut Screen, t: &TournamentView) {
    let theme = screen.theme;
    let (cols, _) = screen.size();
    let width = (cols as usize).max(40);
    screen.begin();
    screen.line(&ui::double_rule(&theme, width.min(78)));
    screen.line(&ui::center(&theme.paint(ui::theme::GOLD, "TOURNAMENT COMPLETE"), 19, width.min(78)));
    screen.line(&ui::double_rule(&theme, width.min(78)));
    screen.blank();
    screen.line(&clip(
        &format!(
            "  {} · {} · {} runners · {}",
            theme.paint(ui::theme::MAGENTA, &t.name),
            theme.accent(t.kind.label()),
            t.entered,
            theme.dim(&duration(t.elapsed))
        ),
        width,
    ));
    screen.blank();

    screen.line(&theme.dim("  FINAL STANDINGS"));
    if t.paid.is_empty() {
        screen.line(&theme.dim("  nobody was paid — it was called off before it began"));
    } else {
        for pay in t.paid.iter() {
            let name = if pay.place == 1 {
                theme.paint(ui::theme::GOLD, &pad_end(&pay.name, 26))
            } else {
                pad_end(&pay.name, 26)
            };
            screen.line(&clip(
                &format!("  {} {} {}", pad_start(&format!("{}.", pay.place), 5), name, pad_start(&theme.win(&thousands(pay.prize)), 12)),
                width,
            ));
        }
    }
    screen.blank();

    screen.line(&theme.dim("  WHAT IT CAME TO"));
    screen.line(&clip(&format!("  prize pool      {}", theme.win(&thousands(t.pool))), width));
    screen.line(&clip(
        &format!(
            "  entries         {} · {} at {} each",
            theme.dim(&thousands(t.buy_in * t.entered as i64)),
            t.entered,
            thousands(t.buy_in)
        ),
        width,
    ));
    screen.line(&clip(&format!("  house revenue   {}", theme.paint(ui::theme::GOLD, &thousands(t.rake))), width));
    screen.line(&clip(&format!("  rounds played   {}", theme.dim(&t.round.to_string())), width));
    screen.line(&clip(&format!("  knocked out     {}", theme.dim(&t.eliminated().to_string())), width));
    screen.blank();
    screen.line(&theme.dim(
        "  the pool was paid out in full — the last place paid absorbs the rounding,",
    ));
    screen.line(&theme.dim("  so exactly what went in came out again."));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('p', "the field"), ('q', "back")]));
}

/// Whether a tournament is one an operator would call active.
#[allow(dead_code, reason = "read by the dashboard's tournament count")]
pub fn is_live(stage: Stage) -> bool {
    stage != Stage::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_finished_tournament_is_not_live() {
        assert!(is_live(Stage::Registering));
        assert!(is_live(Stage::Running(8)));
        assert!(is_live(Stage::FinalTable));
        assert!(!is_live(Stage::Done));
    }
}
