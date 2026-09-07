//! The casino's screens.
//!
//! Every function here draws a *snapshot* and reads a keypress. Not one of
//! them advances a simulation — the manager's thread has already done that,
//! and will keep doing it whether these screens are open or not. That is
//! the whole contract, and it is why switching tables cannot disturb one:
//! switching only changes which snapshot gets asked for.
//!
//! Screens refresh on a timer and poll for keys rather than blocking on
//! one, so a table redraws as it plays rather than only when prodded.

use super::config;
use super::analytics;
use crate::casino;
use super::demand;
use super::event::Weight;
use super::instance::{Kind, Limit};

/// A named opening plan: what to call it, how it reads, and the tables it
/// puts on the floor.
type Preset = (&'static str, &'static str, &'static [(Kind, usize)]);
use super::manager::{FloorView, Manager, TableView};
use crate::ui::{self, pad_end, pad_start, thousands, widgets, Poll, Screen, Theme};
use std::time::Duration;

/// How often a live screen redraws. Four times a second is smooth enough to
/// read and cheap enough that a hundred background tables still get the
/// processor.
const REFRESH: Duration = Duration::from_millis(250);

/// How many lines of the event feed the floor screen shows.
const FEED_ROWS: usize = 6;

/// How many frames a notification stays on screen. At `REFRESH` a frame is
/// a quarter of a second, so this is a couple of seconds — long enough to
/// read, short enough not to sit on top of the floor.
const NOTICE_FRAMES: u32 = 8;

fn signed(theme: &Theme, n: i64) -> String {
    if n >= 0 {
        theme.win(&format!("+{}", thousands(n)))
    } else {
        theme.lose(&thousands(n))
    }
}

fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3_600, (s % 3_600) / 60)
    }
}

/// The opening screen: choose how big a floor to run, then start it.
/// Returns the tables to open, or `None` if the user backed out.
pub fn opening(screen: &mut Screen) -> Option<Vec<(Kind, usize)>> {
    let presets: [Preset; 4] = [
        ("a quiet night", "6 tables, a handful of regulars", &[(Kind::Slots, 2), (Kind::Roulette, 1), (Kind::Blackjack, 2), (Kind::Baccarat, 1)]),
        (
            "a busy floor",
            "20 tables across the whole building",
            &[
                (Kind::Slots, 6),
                (Kind::Roulette, 3),
                (Kind::Blackjack, 3),
                (Kind::Baccarat, 2),
                (Kind::VidPoker, 2),
                (Kind::Crash, 1),
                (Kind::Keno, 1),
                (Kind::Horses, 1),
                (Kind::BigSix, 1),
            ],
        ),
        ("one of everything", "every table in the building, once", &[]),
        ("a full house", "every table, three of each — 51 running at once", &[]),
    ];

    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "START THE CASINO");
    screen.blank();
    screen.line("  the floor runs itself: tables deal, patrons come and go, and the");
    screen.line("  books move — whether or not you are watching any of it.");
    screen.blank();
    for (i, (name, blurb, _)) in presets.iter().enumerate() {
        screen.line(&format!(
            "   {}  {}  {}",
            theme.paint(ui::theme::GOLD, &format!("[{}]", i + 1)),
            theme.accent(&format!("{name:<20}")),
            theme.dim(blurb)
        ));
    }
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('1', "…"), ('4', "pick a floor"), ('q', "not tonight")]));
    screen.present();

    let choice = ui::choose_key(&['1', '2', '3', '4', 'q'], 'q')?;
    let idx = choice.to_digit(10)? as usize;
    if idx == 0 || idx > presets.len() {
        return None;
    }
    Some(match idx {
        3 => Kind::ALL.iter().map(|k| (*k, 1)).collect(),
        4 => Kind::ALL.iter().map(|k| (*k, 3)).collect(),
        n => presets[n - 1].2.to_vec(),
    })
}

/// The overview: every running table, live. This is the screen the spec
/// calls the active-game list, and it is also where tables are opened,
/// paused and closed.
///
/// Returns the id of a table to watch, if the user picked one.
pub fn floor(manager: &Manager, screen: &mut Screen) -> Option<u32> {
    let mut cursor = 0usize;
    // Where this screen has read up to on the feed, so a notification is
    // shown once and is never re-announced. Starting at the current cursor
    // means walking in does not replay the whole night at you.
    let mut seen = manager.feed_cursor();
    let mut notice: Option<(String, u32)> = None;
    loop {
        let view = manager.snapshot();

        // Anything published since the last look that clears the "worth
        // interrupting somebody for" bar gets announced. The bar itself is
        // `Weight::Major`, which the *simulation* set from the thresholds
        // in configuration — this screen does not know what a big win is.
        let (fresh, next) = manager.feed_since(seen, Weight::Major);
        seen = next;
        if let Some(last) = fresh.last() {
            notice = Some((last.event.describe(), NOTICE_FRAMES));
        }

        if view.tables.is_empty() {
            draw_empty(screen, &view);
        } else {
            cursor = cursor.min(view.tables.len() - 1);
            draw_floor(screen, &view, cursor);
        }
        if let Some((text, left)) = notice.take() {
            let theme = screen.theme;
            screen.line(&format!("  {} {}", theme.paint(ui::theme::GOLD, "▶"), theme.win(&text)));
            if left > 1 {
                notice = Some((text, left - 1));
            }
        }
        screen.present();

        match poll(REFRESH, &['w', 'o', 'p', 'x', 's', 'j', 'k', 'n', 'b', 'm', 'e', 'v', 't', 'c', 'r']) {
            Poll::Leave => return None,
            Poll::Pressed('w') => {
                if let Some(t) = view.tables.get(cursor) {
                    return Some(t.id);
                }
            }
            Poll::Pressed('j') | Poll::Pressed('n') if !view.tables.is_empty() => {
                cursor = (cursor + 1) % view.tables.len();
            }
            Poll::Pressed('k') | Poll::Pressed('b') if !view.tables.is_empty() => {
                cursor = (cursor + view.tables.len() - 1) % view.tables.len();
            }
            Poll::Pressed('p') => {
                if let Some(t) = view.tables.get(cursor) {
                    manager.toggle_pause(t.id);
                }
            }
            Poll::Pressed('x') => {
                if let Some(t) = view.tables.get(cursor) {
                    manager.close(t.id);
                }
            }
            Poll::Pressed('s') => manager.cycle_speed(),
            Poll::Pressed('m') => books(manager, screen),
            Poll::Pressed('e') => feed_screen(manager, screen),
            Poll::Pressed('t') => night(manager, screen),
            Poll::Pressed('c') => leaderboards(manager, screen),
            Poll::Pressed('r') => tourneys(manager, screen),
            Poll::Pressed('v') => {
                let start = manager.most_interesting().map(|(id, _)| id).or_else(|| view.tables.first().map(|t| t.id));
                if let Some(id) = start {
                    spectate(manager, screen, id);
                }
            }
            Poll::Pressed('o') => {
                if let Some((kind, n, limit)) = open_more(screen) {
                    manager.open_at(kind, n, limit);
                }
            }
            _ => {}
        }
    }
}

fn draw_empty(screen: &mut Screen, view: &FloorView) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "THE CASINO FLOOR");
    screen.blank();
    screen.line(&theme.dim("  every table has closed. the floor is still open — nothing is running on it."));
    screen.blank();
    screen.line(&format!("  the books stand at {} and {}", theme.win(&format!("${}", thousands(view.money))), theme.gold(&format!("{} chips", thousands(view.chips)))));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('o', "open some tables"), ('q', "back to the floor")]));
}

fn draw_floor(screen: &mut Screen, view: &FloorView, cursor: usize) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "THE CASINO FLOOR");
    screen.blank();
    screen.line(&format!(
        "  {} · {} tables · {} rounds · {} bets · open {} · speed {}",
        if view.running { theme.win("OPEN") } else { theme.lose("CLOSED") },
        theme.accent(&view.tables.len().to_string()),
        theme.accent(&thousands(view.rounds as i64)),
        theme.dim(&thousands(view.bets as i64)),
        theme.dim(&duration(view.running_for)),
        theme.paint(ui::theme::GOLD, &config::speed_label(view.speed))
    ));
    screen.line(&theme.dim(&format!(
        "  {} in the building · {} people known · {}",
        view.crowd,
        view.known,
        view
            .by_tier
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| format!("{n} {}", view.cfg.tier_name(i)))
            .collect::<Vec<_>>()
            .join(" · ")
    )));
    // Whatever is going on in the building tonight, said plainly at the
    // top where it belongs — it is the reason the numbers below look the
    // way they do.
    if !view.weather.is_empty() {
        let going: Vec<String> = view.weather.iter().map(|g| theme.paint(ui::theme::GOLD, g.what.label())).collect();
        screen.line(&format!("  tonight: {}", going.join(theme.dim(" · ").as_str())));
    }
    if view.speed != config::SPEED_UNIT {
        // At anything but real time the two clocks diverge, and the one the
        // simulation actually runs on is the one worth showing.
        screen.line(&theme.dim(&format!("  {} of casino time has passed", duration(view.sim_time))));
    }
    let (collected, paid, bought, cashed) = view.totals;
    screen.line(&format!(
        "  tables {} · cage {} · together {}",
        signed(&theme, view.table_profit),
        signed(&theme, view.cage_profit),
        signed(&theme, view.table_profit + view.cage_profit)
    ));
    screen.line(&theme.dim(&format!(
        "  took {} chips, paid {} · sold ${} of chips, bought back ${}",
        thousands(collected),
        thousands(paid),
        thousands(bought),
        thousands(cashed)
    )));
    if !view.by_table.is_empty() {
        // The five tables earning the most, so it is obvious at a glance
        // where the night's money is actually coming from.
        let best: Vec<String> = view.by_table.iter().take(5).map(|(k, n)| format!("{k} {}", signed(&theme, *n))).collect();
        screen.line(&theme.dim(&format!("  earning: {}", best.join(" · "))));
    }
    screen.blank();
    screen.line(&theme.dim(&format!(
        "  {:<24} {:>5} {:>7}  {:<9} {:>10}  {}",
        "TABLE", "SEATS", "ROUND", "STATUS", "TAKE", "LAST ROUND"
    )));

    // Only as many rows as the window has, scrolled to keep the cursor in
    // view — a floor of two hundred tables must still be navigable.
    let (_, rows) = screen.size();
    // The feed gets a fixed strip at the bottom; the table list takes what
    // is left. Both shrink together on a small terminal rather than one
    // pushing the other off the screen.
    let feed_lines = if view.feed.is_empty() { 0 } else { FEED_ROWS.min(view.feed.len()) + 2 };
    let room = (rows as usize).saturating_sub(12 + feed_lines).max(4);
    let first = cursor.saturating_sub(room / 2).min(view.tables.len().saturating_sub(room));
    for (i, t) in view.tables.iter().enumerate().skip(first).take(room) {
        let marker = if i == cursor { theme.paint(ui::theme::GOLD, " ▸ ") } else { "   ".to_string() };
        let last = match &t.last {
            Some(r) => format!("pot {} · house {}", thousands(r.pot), signed(&theme, r.house())),
            None => theme.dim("dealing in...").to_string(),
        };
        // A high-limit table already wears a star in its name; picking it
        // out in gold as well is how it reads at a glance on a full floor.
        let name = if i == cursor || t.limit == Limit::High {
            theme.paint(ui::theme::GOLD, &pad_end(&t.name, 24))
        } else {
            pad_end(&t.name, 24)
        };
        screen.line(&format!(
            "{marker}{name} {:>5} {:>7}  {}  {}  {last}",
            t.seats,
            t.round,
            pad_end(&theme.dim(t.status), 9),
            pad_start(&signed(&theme, t.take), 10)
        ));
    }
    if view.tables.len() > room {
        screen.line(&theme.dim(&format!("   … {} of {} shown", room, view.tables.len())));
    }
    draw_feed(screen, view, FEED_ROWS);
    screen.blank();
    screen.line(&widgets::footer(
        &theme,
        &[
            ('j', "next"),
            ('k', "prev"),
            ('w', "watch"),
            ('o', "open"),
            ('p', "pause"),
            ('x', "close"),
            ('v', "spectate"),
            ('e', "feed"),
            ('t', "the night"),
            ('c', "customers"),
            ('r', "tourneys"),
            ('m', "books"),
            ('s', "speed"),
            ('q', "back"),
        ],
    ));
}

/// How a seat's run reads: this visit, and — for somebody who has been in
/// before — what they are up or down across every visit they have made.
/// The second figure is the one that could not exist before people were
/// persistent, so it is worth the column.
fn run_column(p: &crate::casino::patron::Patron, bet: &str) -> String {
    let this = format!("{}%", p.luck());
    let head = if bet.is_empty() { this } else { format!("{bet} · {this}") };
    if p.lifetime.visits > 1 {
        let career = p.lifetime.net();
        let sign = if career >= 0 { "+" } else { "-" };
        format!("{head} · career {sign}{} ({}%)", thousands(career.abs()), p.lifetime_luck())
    } else {
        head
    }
}

/// The strip of recent goings-on at the foot of the floor screen.
///
/// What reaches it is decided by the simulation, not here: the snapshot
/// carries only events the floor judged `Notable` or better, so the
/// per-round traffic that would drown this panel never arrives in the first
/// place. Events worth interrupting somebody for are marked, using the
/// thresholds from configuration rather than any number written down in the
/// drawing code.
fn draw_feed(screen: &mut Screen, view: &FloorView, rows: usize) {
    if view.feed.is_empty() {
        return;
    }
    let theme = screen.theme;
    screen.blank();
    screen.line(&theme.dim("  ON THE FLOOR"));
    let start = view.feed.len().saturating_sub(rows);
    for r in view.feed.iter().skip(start) {
        let stamp = theme.dim(&duration(r.at));
        let line = r.event.describe();
        if r.weight >= Weight::Major {
            screen.line(&format!("  {} {}", stamp, theme.paint(ui::theme::GOLD, &line)));
        } else {
            screen.line(&format!("  {} {}", stamp, theme.dim(&line)));
        }
    }
}

/// The whole feed, as a screen rather than a strip.
///
/// The filter is the point. A floor publishes millions of things an hour
/// and almost none of them are worth a person's attention; the weight a
/// publisher gave an event is what decides whether it can appear here at
/// all, and this screen only chooses between "worth reading" and "worth
/// interrupting somebody for".
fn feed_screen(manager: &Manager, screen: &mut Screen) {
    let mut only_big = false;
    loop {
        let min = if only_big { Weight::Major } else { Weight::Notable };
        let (_, rows) = screen.size();
        let room = (rows as usize).saturating_sub(9).max(4);
        let records = manager.feed(room, min);
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, "ON THE FLOOR");
        screen.blank();
        screen.line(&format!(
            "  showing {} · {} on the feed so far",
            if only_big { theme.paint(ui::theme::GOLD, "only the big ones") } else { theme.accent("everything worth reading") },
            theme.dim(&thousands(records.len() as i64))
        ));
        screen.blank();
        if records.is_empty() {
            screen.line(&theme.dim("  nothing yet — open some tables and give it a moment"));
        }
        for r in records.iter() {
            let stamp = theme.dim(&pad_start(&duration(r.at), 8));
            let line = r.event.describe();
            if r.weight >= Weight::Major {
                screen.line(&format!("  {} {}", stamp, theme.paint(ui::theme::GOLD, &line)));
            } else {
                screen.line(&format!("  {} {}", stamp, line));
            }
        }
        screen.blank();
        screen.line(&widgets::footer(&theme, &[('b', "big ones only"), ('a', "everything"), ('q', "back")]));
        screen.present();
        match poll(REFRESH, &['b', 'a']) {
            Poll::Leave => return,
            Poll::Pressed('b') => only_big = true,
            Poll::Pressed('a') => only_big = false,
            _ => {}
        }
    }
}

/// The night so far: how it has been going, and which games carried it.
///
/// Every figure here is a sum of counters that were incremented once, when
/// the thing they count happened — see `casino::analytics`. Changing the
/// span does not recompute anything; it adds up a different handful of
/// buckets.
fn night(manager: &Manager, screen: &mut Screen) {
    let mut span = analytics::Span::LastTenMinutes;
    loop {
        let view = manager.snapshot();
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, "THE NIGHT SO FAR");
        screen.blank();

        let t = view.spans.iter().find(|(s, _)| *s == span).map(|(_, t)| *t).unwrap_or_default();
        screen.line(&format!("  {}", theme.paint(ui::theme::GOLD, span.label())));
        screen.line(&format!(
            "  {} staked over {} bets · average {} a bet",
            theme.accent(&thousands(t.handle)),
            theme.dim(&thousands(t.bets as i64)),
            theme.dim(&thousands(t.average_bet()))
        ));
        screen.line(&format!(
            "  the house kept {} — a hold of {}",
            signed(&theme, t.ggr()),
            theme.paint(ui::theme::GOLD, &percent(t.hold()))
        ));
        screen.line(&format!(
            "  {} sat down, {} got up · biggest single hit {}",
            theme.dim(&t.arrivals.to_string()),
            theme.dim(&t.departures.to_string()),
            theme.accent(&thousands(t.biggest_win))
        ));
        screen.blank();

        // The shape of the recent past, as a bar per bucket. A picture of
        // whether the room is picking up or dying off.
        if view.shape.iter().any(|h| *h > 0) {
            let peak = view.shape.iter().copied().max().unwrap_or(1).max(1);
            screen.line(&theme.dim("  how busy it has been, oldest first"));
            for h in view.shape.iter() {
                let width = (h * 48 / peak).max(0) as usize;
                screen.line(&format!(
                    "  {} {}",
                    pad_start(&thousands(*h), 10),
                    theme.dim(&"\u{2588}".repeat(width))
                ));
            }
            screen.blank();
        }

        screen.line(&theme.dim(&format!(
            "  {:<14} {:>14} {:>12} {:>9} {:>12}",
            "GAME", "STAKED", "HOUSE WIN", "HOLD", "BIGGEST HIT"
        )));
        if view.games.is_empty() {
            screen.line(&theme.dim("  nothing has been played yet"));
        }
        for (game, g) in view.games.iter().take(10) {
            screen.line(&format!(
                "  {} {} {} {} {}",
                pad_end(game, 14),
                pad_start(&thousands(g.handle), 14),
                pad_start(&signed(&theme, g.ggr()), 12),
                pad_start(&percent(g.hold()), 9),
                pad_start(&thousands(g.biggest_win), 12)
            ));
        }

        screen.blank();
        screen.line(&widgets::footer(&theme, &[('t', "change the span"), ('q', "back")]));
        screen.present();
        match poll(REFRESH, &['t']) {
            Poll::Leave => return,
            Poll::Pressed('t') => span = span.next(),
            _ => {}
        }
    }
}

/// Who the house's customers actually are.
///
/// Three different questions, deliberately kept apart: who puts the most
/// through, who is up on the house, and who keeps coming back. They are
/// rarely the same people, and the third list is the one that could not
/// exist at all before patrons became persistent.
fn leaderboards(manager: &Manager, screen: &mut Screen) {
    loop {
        let view = manager.snapshot();
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, "THE HOUSE'S CUSTOMERS");
        screen.blank();
        screen.line(&format!(
            "  {} people known · {} in the building",
            theme.accent(&view.known.to_string()),
            theme.dim(&view.crowd.to_string())
        ));
        screen.blank();

        screen.line(&theme.dim(&format!("  BIGGEST SPENDERS{:<8} {:>14} {:>9}", "", "PUT THROUGH", "VISITS")));
        if view.top_turnover.is_empty() {
            screen.line(&theme.dim("  nobody has played a hand yet"));
        }
        for (name, staked, visits) in view.top_turnover.iter() {
            screen.line(&format!(
                "  {} {} {}",
                pad_end(name, 24),
                pad_start(&thousands(*staked), 14),
                pad_start(&visits.to_string(), 9)
            ));
        }
        screen.blank();

        screen.line(&theme.dim(&format!("  UP ON THE HOUSE{:<9} {:>14} {:>9}", "", "AHEAD BY", "RUN")));
        if view.top_winners.is_empty() {
            screen.line(&theme.dim("  the house is beating everybody, for now"));
        }
        for (name, net, luck) in view.top_winners.iter() {
            screen.line(&format!(
                "  {} {} {}",
                pad_end(name, 24),
                pad_start(&theme.win(&thousands(*net)), 14),
                pad_start(&format!("{luck}%"), 9)
            ));
        }
        screen.blank();

        screen.line(&theme.dim(&format!("  REGULARS{:<16} {:>14} {:>9}", "", "VISITS", "")));
        if view.top_regulars.is_empty() {
            screen.line(&theme.dim("  nobody has been in yet"));
        }
        for (name, visits, style) in view.top_regulars.iter() {
            screen.line(&format!(
                "  {} {} {}",
                pad_end(name, 24),
                pad_start(&visits.to_string(), 14),
                pad_start(&theme.dim(style), 16)
            ));
        }

        screen.blank();
        screen.line(&widgets::footer(&theme, &[('q', "back to the floor")]));
        screen.present();
        if let Poll::Leave = poll(REFRESH, &[]) {
            return;
        }
    }
}

/// Something went wrong with the saved casino, said plainly.
///
/// Shown instead of silently laying out a new floor over the top of one
/// somebody may have hours in — if a save cannot be read, that is worth
/// stopping for.
pub fn trouble(screen: &mut Screen, what: &str) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "THE SAVED CASINO");
    screen.blank();
    screen.line(&format!("  {}", theme.lose(what)));
    screen.blank();
    screen.line(&theme.dim("  the old file has been left exactly where it is, in case it can be repaired"));
    screen.line(&theme.dim(&format!("  it is at {}", casino::save::path().display())));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('\u{23ce}', "open a new casino")]));
    screen.present();
    let _ = ui::input::read_key();
}

/// The tournaments, live.
///
/// A tournament is not a table you can sit at, so it gets its own screen
/// rather than a row on the floor: a field playing itself down, the
/// standings, and — when it is over — who got paid what.
fn tourneys(manager: &Manager, screen: &mut Screen) {
    loop {
        let view = manager.snapshot();
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, "TOURNAMENTS");
        screen.blank();
        if view.tourneys.is_empty() {
            screen.line(&theme.dim("  none on at the moment — one starts up every so often"));
            screen.line(&theme.dim(&format!(
                "  entry {} chips, {}% to the house, {} to start with",
                thousands(view.cfg.tourney_buy_in),
                view.cfg.tourney_rake,
                thousands(view.cfg.tourney_stack)
            )));
        }
        for t in view.tourneys.iter().take(3) {
            screen.line(&format!(
                "  {} · {} · {}",
                theme.accent(&t.name),
                theme.paint(ui::theme::GOLD, &t.stage.label()),
                theme.dim(&format!("{} entered · pool {}", t.entered(), thousands(t.pool)))
            ));
            if !t.paid.is_empty() {
                for pay in t.paid.iter().take(6) {
                    screen.line(&format!(
                        "    {} {} {}",
                        pad_start(&format!("{}.", pay.place), 5),
                        pad_end(&pay.name, 24),
                        pad_start(&theme.win(&thousands(pay.prize)), 12)
                    ));
                }
            } else {
                for e in t.standings(8) {
                    screen.line(&format!("    {} {}", pad_end(&e.name, 26), pad_start(&thousands(e.stack), 12)));
                }
            }
            screen.blank();
        }
        screen.line(&widgets::footer(&theme, &[('q', "back to the floor")]));
        screen.present();
        if let Poll::Leave = poll(REFRESH, &[]) {
            return;
        }
    }
}

/// The house's own books, live.
///
/// Two currencies, kept apart the whole way down, because they are two
/// different things: **chips** are the float the tables move about, and
/// **cash** is what the house actually ends the night with. The gaming win
/// is a chip figure; the bottom line is a cash one. Adding them together
/// would count the same money twice, since chips only ever leave the
/// building through the cage.
fn books(manager: &Manager, screen: &mut Screen) {
    loop {
        let view = manager.snapshot();
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, "THE HOUSE BOOKS");
        screen.blank();

        screen.line(&theme.dim("  AT THE TABLES — in chips"));
        screen.line(&format!(
            "  handle {} · paid out {} · gross win {}",
            theme.accent(&thousands(view.handle)),
            theme.dim(&thousands(view.payouts)),
            signed(&theme, view.ggr)
        ));
        screen.line(&format!(
            "  hold {} of everything staked, across {} bets",
            theme.paint(ui::theme::GOLD, &percent(view.hold)),
            theme.dim(&thousands(view.bets as i64))
        ));
        screen.blank();

        let (_, _, bought, cashed) = view.totals;
        screen.line(&theme.dim("  AT THE CAGE — in cash"));
        screen.line(&format!(
            "  sold ${} of chips · bought back ${} · kept {}",
            thousands(bought),
            thousands(cashed),
            signed(&theme, view.cage_profit)
        ));
        screen.blank();

        screen.line(&theme.dim("  WHAT THE BUILDING COSTS — in cash"));
        if view.by_expense.is_empty() {
            screen.line(&theme.dim("  nothing billed yet — the first period has not come round"));
        } else {
            for (kind, spent) in view.by_expense.iter() {
                screen.line(&format!("  {} {}", pad_end(&theme.dim(kind), 14), pad_start(&thousands(*spent), 12)));
            }
        }
        screen.line(&format!(
            "  {} spent in all · every {} of casino time: ${} on the building, ${} a table",
            theme.dim(&format!("${}", thousands(view.spent))),
            theme.dim(&duration(view.cfg.expense_period)),
            view.cfg.overhead_per_period,
            view.cfg.table_cost_per_period
        ));
        screen.blank();

        screen.line(&format!(
            "  {} — the cash the cage kept, less what the doors cost to keep open",
            match view.ngr >= 0 {
                true => theme.win(&format!("NET +${}", thousands(view.ngr))),
                false => theme.lose(&format!("NET -${}", thousands(-view.ngr))),
            }
        ));
        screen.blank();

        screen.line(&theme.dim(&format!(
            "  WHAT THE ROOM IS PLAYING — floor occupancy {}",
            percent(view.occupancy * 10)
        )));
        if view.demand.is_empty() {
            screen.line(&theme.dim("  nothing open, so the room has no opinion yet"));
        } else {
            screen.line(&theme.dim(&format!("  {:<14} {:>8} {:>10} {:>8}  {}", "GAME", "TABLES", "FULL", "MOOD", "")));
            for (game, st) in view.demand.iter().take(8) {
                let mood = if st.appeal >= demand::NEUTRAL {
                    theme.paint(ui::theme::GOLD, &format!("+{}%", (st.appeal - demand::NEUTRAL) / 10))
                } else {
                    theme.dim(&format!("{}%", (st.appeal - demand::NEUTRAL) / 10))
                };
                screen.line(&format!(
                    "  {} {} {} {}",
                    pad_end(game, 14),
                    pad_start(&st.tables.to_string(), 8),
                    pad_start(&percent(st.utilization() * 10), 10),
                    pad_start(&mood, 8)
                ));
            }
        }
        screen.blank();

        screen.line(&theme.dim(&format!("  {:<12} {:>12} {:>16}  {}", "MOVEMENT", "COUNT", "VOLUME", "IN")));
        for (m, count, volume) in view.movements.iter() {
            screen.line(&format!(
                "  {} {} {}  {}",
                pad_end(m.label(), 12),
                pad_start(&thousands(*count as i64), 12),
                pad_start(&thousands(*volume), 16),
                theme.dim(if m.in_dollars() { "cash" } else { "chips" })
            ));
        }

        screen.blank();
        screen.line(&widgets::footer(&theme, &[('q', "back to the floor")]));
        screen.present();
        if let Poll::Leave = poll(REFRESH, &[]) {
            return;
        }
    }
}

/// Hundredths of a percent, as a percentage: `415` reads `4.15%`.
fn percent(hundredths: i64) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let v = hundredths.abs();
    format!("{sign}{}.{:02}%", v / 100, v % 100)
}

/// Adds tables to a running floor without disturbing anything already on it.
fn open_more(screen: &mut Screen) -> Option<(Kind, usize, Limit)> {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "OPEN A TABLE");
    screen.blank();
    // Kinds get 1-9 then a-h, the same scheme the idle room uses.
    let key_for = |i: usize| -> char {
        if i < 9 {
            char::from_digit(i as u32 + 1, 10).unwrap()
        } else {
            (b'a' + (i - 9) as u8) as char
        }
    };
    for (i, k) in Kind::ALL.iter().enumerate() {
        let (lo, hi) = k.seats();
        screen.line(&format!(
            "   {}  {}  {}",
            theme.paint(ui::theme::GOLD, &format!("[{}]", key_for(i).to_ascii_uppercase())),
            theme.accent(&format!("{:<18}", k.label())),
            theme.dim(&format!("{lo}-{hi} {}s · a round every {}s", k.seat_word(), k.pace().as_millis() / 1000))
        ));
    }
    screen.blank();
    screen.line(&theme.dim("  a high-limit table seats only the house's best customers"));
    screen.line(&widgets::footer(&theme, &[('1', "…"), ('h', "pick a table"), ('q', "back")]));
    screen.present();

    let mut keys: Vec<char> = (0..Kind::ALL.len()).map(key_for).collect();
    keys.push('q');
    let c = ui::choose_key(&keys, 'q')?;
    let idx = (0..Kind::ALL.len()).find(|i| key_for(*i) == c)?;
    let kind = Kind::ALL[idx];

    let n = widgets::number_picker(screen, 1, 25, 1, 1, &[('m', 25)], |s, v| {
        let theme = s.theme;
        s.begin();
        ui::header(s, "OPEN A TABLE");
        s.blank();
        s.line(&format!("  how many {} tables?", theme.accent(kind.label())));
        s.blank();
        s.line(&format!("   {}", theme.paint(ui::theme::GOLD, &v.to_string())));
        s.blank();
        s.line(&widgets::footer(&theme, &[('↑', "more"), ('↓', "fewer"), ('m', "25"), ('\u{23ce}', "open"), ('\u{238b}', "cancel")]));
    })?;

    // House or high limit. Asked after the count so the common case is
    // still two keys and Enter.
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "OPEN A TABLE");
    screen.blank();
    screen.line(&format!("  {} × {}", theme.accent(kind.label()), theme.paint(ui::theme::GOLD, &n.to_string())));
    screen.blank();
    screen.line(&format!("   {}  {}", theme.paint(ui::theme::GOLD, "[H]"), theme.accent("house limit — open to anybody")));
    screen.line(&format!(
        "   {}  {}",
        theme.paint(ui::theme::GOLD, "[V]"),
        theme.accent("high limit ★ — the top tier only, and they can bet like it")
    ));
    screen.blank();
    screen.line(&widgets::footer(&theme, &[('h', "house"), ('v', "high limit"), ('q', "cancel")]));
    screen.present();
    let limit = match ui::choose_key(&['h', 'v', 'q'], 'q')? {
        'v' => Limit::High,
        _ => Limit::House,
    };
    Some((kind, n as usize, limit))
}

/// Watches one running table, live. `←`/`→` move to the neighbouring table
/// without stopping anything: the tables the user is not looking at carry
/// on exactly as before.
/// Spectator mode: the floor picks what you look at.
///
/// It holds on a table for a configured spell, then moves to whichever
/// table is most worth watching — Phase 12's "follow the action", with the
/// score itself living in `casino::interest` where it can be tuned.
///
/// The one rule this screen has to keep is that **it never pauses
/// anything**. Watching is watching; the tables it is not showing carry on
/// exactly as they were, and so does the one it is. The `z` key still
/// pauses a table, because that is the user explicitly asking for it.
pub fn spectate(manager: &Manager, screen: &mut Screen, start_at: u32) {
    let mut id = start_at;
    let mut held = Duration::ZERO;
    let mut following = true;
    let mut why = None;
    loop {
        let ids = manager.ids();
        if ids.is_empty() {
            return;
        }
        let hold = manager.config().spectate_for;
        if following && (held >= hold || !ids.contains(&id)) {
            if let Some((best, interest)) = manager.most_interesting() {
                id = best;
                why = Some(interest);
            }
            held = Duration::ZERO;
        }
        if !ids.contains(&id) {
            id = ids[0];
        }
        let Some(view) = manager.table(id) else { return };
        let at = ids.iter().position(|i| *i == id).unwrap_or(0);
        draw_table(screen, &view, at + 1, ids.len());
        let theme = screen.theme;
        let left = hold.saturating_sub(held);
        screen.line(&if following {
            let reason = why.map(|i: super::interest::Interest| i.why.label()).unwrap_or("ticking over");
            theme.dim(&format!("  following the action — {reason} · moving on in {}", duration(left)))
        } else {
            theme.dim("  holding on this table — [f] to follow the action again")
        });
        screen.line(&widgets::footer(
            &theme,
            &[('f', "follow"), ('h', "hold"), ('n', "next"), ('z', "pause table"), ('q', "back")],
        ));
        screen.present();

        match poll(REFRESH, &['f', 'h', 'n', 'p', 'z']) {
            Poll::Leave => return,
            Poll::Pressed('f') => {
                following = true;
                held = hold; // move at once rather than serving out the spell
            }
            Poll::Pressed('h') => following = false,
            Poll::Pressed('n') => {
                id = ids[(at + 1) % ids.len()];
                following = false;
            }
            Poll::Pressed('p') => {
                id = ids[(at + ids.len() - 1) % ids.len()];
                following = false;
            }
            // The only thing on this screen that touches the simulation,
            // and only because the user asked for it by name.
            Poll::Pressed('z') => manager.toggle_pause(id),
            _ => held += REFRESH,
        }
    }
}

pub fn watch(manager: &Manager, screen: &mut Screen, start_at: u32) {
    let mut id = start_at;
    loop {
        let ids = manager.ids();
        if ids.is_empty() {
            return;
        }
        if !ids.contains(&id) {
            // The table closed under us — step to whatever is nearest.
            id = ids[0];
        }
        let Some(view) = manager.table(id) else { return };
        let at = ids.iter().position(|i| *i == id).unwrap_or(0);
        draw_table(screen, &view, at + 1, ids.len());
        screen.present();

        match poll(REFRESH, &['n', 'p', 'f', 'h', 'l', 'z']) {
            Poll::Leave => return,
            Poll::Pressed('f') => return,
            Poll::Pressed('n') | Poll::Pressed('l') => id = ids[(at + 1) % ids.len()],
            Poll::Pressed('p') | Poll::Pressed('h') => id = ids[(at + ids.len() - 1) % ids.len()],
            Poll::Pressed('z') => manager.toggle_pause(id),
            _ => {}
        }
    }
}

fn draw_table(screen: &mut Screen, t: &TableView, position: usize, total: usize) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, &t.name.to_uppercase());
    screen.blank();
    screen.line(&format!(
        "  round {} · {} {}s · {} · open {} · {} through the door",
        theme.accent(&thousands(t.round as i64)),
        t.seats,
        t.kind.seat_word(),
        theme.dim(t.status),
        theme.dim(&duration(t.open_for)),
        t.seen
    ));
    screen.line(&format!(
        "  {} table · bets up to {} · {}",
        theme.dim(t.limit.label()),
        theme.dim(&thousands(t.limit.ceiling(&t.cfg))),
        if t.interest.score > 0 {
            theme.paint(ui::theme::GOLD, t.interest.why.label())
        } else {
            theme.dim(t.interest.why.label())
        }
    ));
    screen.line(&format!(
        "  {} staked here · the house is {}",
        theme.dim(&thousands(t.staked)),
        signed(&theme, t.take)
    ));
    screen.blank();

    screen.line(&theme.dim(&format!(
        "  {:<22} {:<14} {:<12} {:>6} {:>8} {:>9} {:>9}  {}",
        "AT THE TABLE", "STYLE", "STANDING", "VISITS", "STAKE", "STACK", "NET", "RUN"
    )));
    let last = t.last.as_ref();
    for p in &t.patrons {
        let seat = last.and_then(|r| r.seats.iter().find(|s| s.name == p.name));
        let (staked, back) = seat.map(|s| (s.staked, s.returned)).unwrap_or((0, 0));
        let bet = seat.map(|s| s.bet.clone()).unwrap_or_default();
        // A regular is worth pointing out — this is the visible proof
        // that the people are not conjured up per seat.
        let tier = p.tier(&t.cfg);
        let standing = if tier >= t.cfg.top_tier() {
            theme.paint(ui::theme::GOLD, &pad_end(t.cfg.tier_name(tier), 12))
        } else {
            pad_end(&theme.dim(t.cfg.tier_name(tier)), 12)
        };
        let _ = back;
        screen.line(&format!(
            "  {:<22} {} {} {:>6} {:>8} {:>9} {}  {}",
            p.name,
            pad_end(&theme.dim(p.style()), 14),
            standing,
            p.lifetime.visits,
            if staked > 0 { thousands(staked) } else { "—".into() },
            thousands(p.chips),
            pad_start(&signed(&theme, p.net()), 9),
            theme.dim(&run_column(p, &bet))
        ));
    }

    screen.blank();
    screen.line(&theme.dim("  recent rounds"));
    for r in t.history.iter().rev().take(6) {
        screen.line(&format!(
            "   {:>6}  pot {:>9}  paid {:>9}  house {}",
            format!("#{}", r.number),
            thousands(r.pot),
            thousands(r.paid),
            signed(&theme, r.house())
        ));
    }
    if t.history.is_empty() {
        screen.line(&theme.dim("   nothing settled yet"));
    }

    screen.blank();
    screen.line(&theme.dim(&format!("  table {position} of {total} on the floor — the rest are still running")));
    screen.line(&widgets::footer(&theme, &[('p', "prev table"), ('n', "next table"), ('z', "pause"), ('f', "floor"), ('q', "back")]));
}

/// Redraw-and-poll: waits up to `window` for one of `keys`, so a live screen
/// stays responsive without ever blocking on a keypress.
fn poll(window: Duration, keys: &[char]) -> Poll {
    let start = std::time::Instant::now();
    loop {
        match ui::poll_action(keys) {
            Poll::Nothing => {}
            other => return other,
        }
        if start.elapsed() >= window {
            return Poll::Nothing;
        }
        ui::sleep_ms(20);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_reads_the_way_money_is_written() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(125_430), "125,430");
        assert_eq!(thousands(-82_500), "-82,500");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_read_as_a_person_would_say_them() {
        assert_eq!(duration(Duration::from_secs(9)), "9s");
        assert_eq!(duration(Duration::from_secs(75)), "1m 15s");
        assert_eq!(duration(Duration::from_secs(3_725)), "1h 02m");
    }

    #[test]
    fn colour_codes_do_not_count_toward_a_lines_width() {
        let plain = Theme::new(false);
        let colour = Theme::new(true);
        let a = plain.win("+1,000");
        let b = colour.win("+1,000");
        assert!(b.len() > a.len(), "the colour version should carry escapes");
        assert_eq!(ui::visible_len(&a), ui::visible_len(&b), "escapes leaked into the visible width");
        assert_eq!(ui::visible_len(&b), 6);
    }
}
