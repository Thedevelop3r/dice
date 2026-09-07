//! The operations centre.
//!
//! One screen an operator can stand in front of and see the whole
//! building: what is running, what it is making, what has just happened,
//! and who is coming through the door. Four panels, live, side by side.
//!
//! It is a *viewer*, and the same rule the rest of `ui` keeps applies here
//! with knobs on:
//!
//! - It takes one [`dashboard::Snapshot`] per frame, built under the
//!   simulation lock and drawn after the lock is released. Nothing here
//!   runs while the floor is held.
//! - It never advances, pauses, reseeds or restarts anything as a side
//!   effect of being open or being closed. Opening the dashboard is
//!   invisible from inside the simulation.
//! - The only things on this screen that touch the floor are the ones an
//!   operator asked for by name: change the speed, start a tournament,
//!   reseed, and the table controls the floor screen already had.
//!
//! Every panel is rendered into a list of lines for a stated width and
//! height, and the layout is chosen from the terminal's actual size — so a
//! narrow window gets one section at a time with tabs rather than four
//! sections with their right-hand halves cut off. Nothing is drawn wider
//! than the window it is going into.

use super::super::dashboard::{GameView, Layout, Section, Snapshot, Standing};
use super::super::manager::Manager;
use super::{duration, poll, signed, REFRESH};
use crate::casino::event::Weight;
use crate::casino::reception::Cage;
use crate::ui::{self, clip, pad_end, pad_start, thousands, widgets, Poll, Screen, Theme};

/// How many rows the header, the status bar and the panel titles cost. The
/// panels get whatever is left, and never less than a few lines.
const CHROME: usize = 7;

/// Where the keyboard is pointing, and where it was pointing in each of the
/// other sections.
///
/// One cursor per list rather than one shared one: moving to the books and
/// back must not lose your place on a floor of two hundred tables.
#[derive(Debug, Clone, Copy, Default)]
struct Focus {
    section: usize,
    game: usize,
    tourney: usize,
    guest: usize,
}

impl Focus {
    fn section(&self) -> Section {
        Section::ALL[self.section.min(Section::ALL.len() - 1)]
    }

    fn set(&mut self, s: Section) {
        self.section = s.index();
    }

    /// The cursor belonging to the section in focus, if it has a list.
    fn cursor(&mut self) -> Option<&mut usize> {
        match self.section() {
            Section::Games => Some(&mut self.game),
            Section::Financials => None,
            Section::Events => Some(&mut self.tourney),
            Section::Reception => Some(&mut self.guest),
        }
    }
}

/// The operations centre, live.
///
/// Returns when the operator leaves. Everything it was showing carries on
/// without it.
pub fn dashboard(manager: &Manager, screen: &mut Screen) {
    let mut focus = Focus::default();
    loop {
        // One picture, one moment, one lock acquisition. See
        // `Manager::dashboard` for why this is not four calls.
        let view = manager.dashboard();
        draw(screen, &view, &mut focus);
        screen.present();

        let keys = ['j', 'k', 'n', 'b', 'h', 'l', '\t', '\r', '\n', 'w', 'v', '1', '2', '3', '4', 's', 'r', 't', 'g', 'o'];
        match poll(REFRESH, &keys) {
            Poll::Leave => return,
            Poll::Pressed('j') | Poll::Pressed('n') => step(&mut focus, &view, 1),
            Poll::Pressed('k') | Poll::Pressed('b') => step(&mut focus, &view, -1),
            Poll::Pressed('l') | Poll::Pressed('\t') => {
                let next = focus.section().next();
                focus.set(next);
            }
            Poll::Pressed('h') => {
                let prev = focus.section().prev();
                focus.set(prev);
            }
            Poll::Pressed(c @ ('1' | '2' | '3' | '4')) => {
                let i = c.to_digit(10).unwrap_or(1) as usize - 1;
                focus.set(Section::ALL[i.min(Section::ALL.len() - 1)]);
            }
            Poll::Pressed('\r') | Poll::Pressed('\n') | Poll::Pressed('w') => open(manager, screen, &view, &focus),
            // Follow the action, from wherever the dashboard is pointed —
            // the same spectator screen the floor screen opens.
            Poll::Pressed('v') => {
                let start = manager
                    .most_interesting()
                    .map(|(id, _)| id)
                    .or_else(|| view.games.first().map(|g| g.id));
                if let Some(id) = start {
                    super::spectate(manager, screen, id);
                }
            }
            Poll::Pressed('s') => manager.cycle_speed(),
            Poll::Pressed('t') => super::tourney_ops::start(manager, screen),
            Poll::Pressed('g') => super::settings(manager, screen),
            Poll::Pressed('o') => {
                if let Some((kind, n, limit)) = super::open_more(screen) {
                    manager.open_at(kind, n, limit);
                }
            }
            // `r` is a forced refresh, and it is deliberately a no-op with
            // a redraw: the next line of the loop takes a fresh snapshot
            // anyway. There is nothing to invalidate, because nothing here
            // is cached — which is the property worth keeping.
            _ => {}
        }
    }
}

/// Moves the focused section's cursor, if it has a list to move it in.
fn step(focus: &mut Focus, view: &Snapshot, by: isize) {
    let len = match focus.section() {
        Section::Games => view.games.len(),
        Section::Financials => 0,
        Section::Events => view.tournaments.len(),
        Section::Reception => view.reception.guests.len(),
    };
    if len == 0 {
        return;
    }
    let Some(cursor) = focus.cursor() else { return };
    let at = (*cursor).min(len - 1) as isize;
    *cursor = (at + by).rem_euclid(len as isize) as usize;
}

/// Opens whatever the focused section has selected.
///
/// Every one of these hands off to a screen that already existed. The
/// dashboard is a way *into* the detailed views, not a second set of them.
fn open(manager: &Manager, screen: &mut Screen, view: &Snapshot, focus: &Focus) {
    match focus.section() {
        Section::Games => {
            if let Some(g) = view.games.get(focus.game) {
                // The table is opened by id, from live state. It is not
                // recreated, restarted or reseeded — `watch` asks the
                // manager for a snapshot of the instance that has been
                // running all along.
                super::watch(manager, screen, g.id);
            }
        }
        Section::Financials => finances(manager, screen),
        Section::Events => {
            if let Some(t) = view.tournaments.get(focus.tourney) {
                super::tourney_ops::overview(manager, screen, t.id);
            } else {
                super::tourneys(manager, screen);
            }
        }
        Section::Reception => {
            if let Some(g) = view.reception.guests.get(focus.guest) {
                guest(manager, screen, g.id);
            }
        }
    }
}

// ---------------------------------------------------------------- drawing

fn draw(screen: &mut Screen, view: &Snapshot, focus: &mut Focus) {
    let theme = screen.theme;
    let (cols, rows) = screen.size();
    let width = (cols as usize).max(40);
    let layout = Layout::for_size(cols, rows);
    screen.begin();
    header(screen, view, width);

    // What is left after the header and the status bar. Panels are built
    // to fit it; nothing is drawn that the window cannot hold.
    let room = (rows as usize).saturating_sub(CHROME).max(6);

    match layout {
        Layout::Grid => {
            // Two columns, two rows of panels. The left column is the
            // wider one: the games list is the section with real columns
            // in it, and it is the one an operator reads first.
            let gap = 2;
            let left_w = (width - gap) * 5 / 9;
            let right_w = width - gap - left_w - 2;
            let half = (room / 2).max(3);
            let top = beside(
                &panel(screen, view, focus, Section::Games, left_w, half),
                &panel(screen, view, focus, Section::Financials, right_w, half),
                left_w,
                gap,
            );
            let bottom = beside(
                &panel(screen, view, focus, Section::Events, left_w, room - half),
                &panel(screen, view, focus, Section::Reception, right_w, room - half),
                left_w,
                gap,
            );
            for line in top.iter().chain(bottom.iter()) {
                screen.line(&clip(line, width));
            }
        }
        Layout::Stacked => {
            // Tall and narrow: all four, one above the other, each given
            // an honest slice of what there is.
            let each = (room / 4).max(3);
            for s in Section::ALL {
                for line in panel(screen, view, focus, s, width - 2, each) {
                    screen.line(&clip(&line, width));
                }
            }
        }
        Layout::Tabs => {
            // Too small for four at once, so one at a time — with the tabs
            // spelled out, because every section must still be reachable.
            let tabs: Vec<String> = Section::ALL
                .iter()
                .map(|s| {
                    let label = format!("[{}] {}", s.key(), short(*s));
                    if *s == focus.section() { theme.paint(ui::theme::GOLD, &label) } else { theme.dim(&label) }
                })
                .collect();
            screen.line(&clip(&format!("  {}", tabs.join("  ")), width));
            for line in panel(screen, view, focus, focus.section(), width - 2, room - 1) {
                screen.line(&clip(&line, width));
            }
        }
    }

    status(screen, focus, layout, width);
}

/// A short name for a section, for a tab strip that has no room for the
/// long one.
fn short(s: Section) -> &'static str {
    match s {
        Section::Games => "Games",
        Section::Financials => "Money",
        Section::Events => "Events",
        Section::Reception => "Cage",
    }
}

/// The strip across the top: the building, in one glance.
///
/// The figures come off the same snapshot as everything below, which comes
/// off the bank — so this can never disagree with the badge in the corner
/// or with the books screen.
fn header(screen: &mut Screen, view: &Snapshot, width: usize) {
    let theme = screen.theme;
    let g = &view.global;
    ui::header(screen, "CASINO DASHBOARD");
    let left = format!(
        "  {} · {} tables · {} in the building · {} tournaments",
        if g.running { theme.win("OPEN") } else { theme.lose("CLOSED") },
        theme.accent(&g.tables.to_string()),
        theme.accent(&g.crowd.to_string()),
        theme.paint(ui::theme::MAGENTA, &g.tourneys.to_string()),
    );
    let right = format!(
        "money {} · chips {}  ",
        theme.win(&format!("${}", thousands(g.money))),
        theme.paint(ui::theme::GOLD, &thousands(g.chips))
    );
    screen.line(&clip(&spread(&left, &right, width), width));
    screen.line(&clip(
        &theme.dim(&format!(
            "  speed {} · up {} · {} of casino time · {} people known · seed {}",
            crate::casino::config::speed_label(g.speed),
            duration(g.uptime),
            duration(g.sim_time),
            g.known,
            g.seed
        )),
        width,
    ));
}

/// The bar along the bottom: what the keys do here.
fn status(screen: &mut Screen, focus: &Focus, layout: Layout, width: usize) {
    let theme = screen.theme;
    let open = match focus.section() {
        Section::Games => "inspect table",
        Section::Financials => "the books",
        Section::Events => "tournament",
        Section::Reception => "guest",
    };
    screen.blank();
    let mut hints = vec![
        ('j', "down"),
        ('k', "up"),
        ('l', "next section"),
        ('h', "prev"),
        ('\r', open),
        ('v', "spectate"),
        ('t', "tournament"),
        ('o', "open table"),
        ('s', "speed"),
        ('g', "settings"),
        ('q', "back"),
    ];
    if layout == Layout::Tabs {
        hints.insert(4, ('1', "sections"));
    }
    screen.line(&clip(&widgets::footer(&theme, &hints), width));
}

/// Puts `right` hard against the right-hand edge of a `width` line, with
/// `left` at the front. Both may be painted, so the gap is measured in
/// visible columns.
fn spread(left: &str, right: &str, width: usize) -> String {
    let used = ui::visible_len(left) + ui::visible_len(right);
    format!("{left}{}{right}", " ".repeat(width.saturating_sub(used)))
}

/// Lays two panels side by side, padding the left one to its column width
/// so the right one starts in the same place on every row.
fn beside(left: &[String], right: &[String], left_w: usize, gap: usize) -> Vec<String> {
    let rows = left.len().max(right.len());
    let blank = String::new();
    (0..rows)
        .map(|i| {
            let l = left.get(i).unwrap_or(&blank);
            let r = right.get(i).unwrap_or(&blank);
            format!("{}{}{}", pad_end(&clip(l, left_w), left_w), " ".repeat(gap), clip(r, 200))
        })
        .collect()
}

/// One section, rendered into lines for a stated box.
///
/// Every panel builds to a height and stops. A list longer than the box
/// says so on its last line rather than running off the bottom of it.
fn panel(screen: &Screen, view: &Snapshot, focus: &mut Focus, section: Section, width: usize, height: usize) -> Vec<String> {
    let theme = screen.theme;
    let focused = section == focus.section();
    let mut out = Vec::with_capacity(height);
    let title = format!("  {}. {}", section.key(), section.title());
    out.push(if focused { theme.paint(ui::theme::GOLD, &title) } else { theme.dim(&title) });
    let body = height.saturating_sub(1).max(1);
    let lines = match section {
        Section::Games => games(&theme, view, focus, focused, width, body),
        Section::Financials => financials(&theme, view, width, body),
        Section::Events => events(&theme, view, focus, focused, width, body),
        Section::Reception => reception(&theme, view, focus, focused, width, body),
    };
    out.extend(lines.into_iter().take(body));
    out
}

/// Section 1 — every table on the floor, live.
fn games(theme: &Theme, view: &Snapshot, focus: &mut Focus, focused: bool, width: usize, rows: usize) -> Vec<String> {
    let mut out = Vec::new();
    if view.games.is_empty() {
        out.push(theme.dim("     nothing is running — [o] opens a table"));
        return out;
    }
    // Wide enough for the whole row, or the essentials only. The columns
    // are dropped from the right, so what survives is what an operator
    // needs first: which table, how busy, and what is on it.
    let wide = width >= 84;
    out.push(theme.dim(&format!(
        "     {} {} {} {} {}{}",
        pad_end("ID", 5),
        pad_end("TABLE", 20),
        pad_start("ROUND", 7),
        pad_start("SEATS", 7),
        pad_start("POT", 11),
        if wide { format!(" {} {} {}", pad_start("FULL", 6), pad_start("HOUSE", 10), "  STATUS") } else { String::new() }
    )));

    // One line is kept back for the selected table's detail, which is
    // where the figures too long for a column go.
    let detail = rows >= 5;
    let room = rows.saturating_sub(if detail { 2 } else { 1 }).max(1);
    focus.game = focus.game.min(view.games.len() - 1);
    // Scrolled to keep the cursor in view: a floor of two hundred tables
    // must stay navigable in a panel eight lines tall.
    let first = focus.game.saturating_sub(room / 2).min(view.games.len().saturating_sub(room));
    for (i, g) in view.games.iter().enumerate().skip(first).take(room) {
        out.push(game_row(theme, g, focused && i == focus.game, wide));
    }
    if view.games.len() > room {
        out.push(theme.dim(&format!("     … {} of {} tables", room.min(view.games.len()), view.games.len())));
    }
    if detail && let Some(g) = view.games.get(focus.game) {
        // Everything about the selected table that would not fit in a
        // column: what it has taken since it opened, and the floor's own
        // reason for finding it worth watching.
        out.push(clip(
            &theme.dim(&format!(
                "     {} · {} staked here · house {} · {}{}",
                g.name,
                thousands(g.staked),
                signed(theme, g.take),
                g.why,
                if g.interest > 0 { format!(" ({})", g.interest) } else { String::new() }
            )),
            width,
        ));
    }
    out
}

fn game_row(theme: &Theme, g: &GameView, selected: bool, wide: bool) -> String {
    let marker = if selected { theme.paint(ui::theme::GOLD, " ▸ ") } else { "   ".to_string() };
    // A high-limit table wears its star in its own name; picking it out in
    // gold is how it reads at a glance on a full floor.
    let name = if selected || g.high_limit() {
        theme.paint(ui::theme::GOLD, &pad_end(&g.name, 20))
    } else {
        pad_end(&g.name, 20)
    };
    let seats = format!("{}/{}", g.seats, g.wanted);
    // A table that has not settled anything in several of its own round
    // times has stalled — which is a fact about *this* table's pace, not a
    // number of seconds this screen decided on.
    let quiet = g.idle_for > g.kind.pace() * 3;
    let status = match g.status {
        "paused" => theme.paint(ui::theme::YELLOW, "PAUSED"),
        "seating" => theme.dim("SEATING"),
        _ if quiet => theme.paint(ui::theme::YELLOW, &format!("QUIET {}", duration(g.idle_for))),
        _ => theme.win("ACTIVE"),
    };
    let head = format!(
        "{marker}{} {name} {} {} {}",
        pad_end(&format!("#{:02}", g.id), 5),
        pad_start(&thousands(g.round as i64), 7),
        pad_start(&seats, 7),
        pad_start(&thousands(g.pot), 11),
    );
    if !wide {
        return head;
    }
    let full = format!("{}%", g.utilization() / 10);
    format!(
        "{head} {} {}   {status}",
        pad_start(&theme.dim(&full), 6),
        pad_start(&signed(theme, g.house), 10)
    )
}

/// Section 2 — the books. Every number comes off the bank; not one of them
/// is worked out on this screen.
fn financials(theme: &Theme, view: &Snapshot, width: usize, _rows: usize) -> Vec<String> {
    let f = &view.financials;
    let badge = match f.standing() {
        Standing::Profitable => theme.win(Standing::Profitable.label()),
        Standing::Loss => theme.lose(Standing::Loss.label()),
        Standing::BreakEven => theme.paint(ui::theme::YELLOW, Standing::BreakEven.label()),
    };
    let mut out = vec![
        spread(
            &format!("     money   {}", theme.win(&format!("${}", thousands(f.money)))),
            &format!("{badge}  "),
            width,
        ),
        format!("     chips   {}", theme.paint(ui::theme::GOLD, &thousands(f.chips))),
        String::new(),
        theme.dim("     GROSS GAMING REVENUE, IN CHIPS"),
        format!(
            "     wagered {} · paid out {} · GGR {}",
            theme.accent(&thousands(f.handle)),
            theme.dim(&thousands(f.payouts)),
            signed(theme, f.ggr)
        ),
        format!(
            "     hold {} over {} bets · average bet {}",
            theme.paint(ui::theme::GOLD, &super::percent(f.hold)),
            theme.dim(&thousands(f.bets as i64)),
            theme.dim(&thousands(f.average_bet()))
        ),
        String::new(),
        theme.dim("     AT THE CAGE, IN CASH"),
        format!(
            "     buy-ins ${} · cash-outs ${} · net {}",
            thousands(f.bought_in),
            thousands(f.cashed_out),
            signed(theme, f.net_cage_flow())
        ),
        format!(
            "     costs ${} · {}",
            thousands(f.spent),
            match f.ngr >= 0 {
                true => theme.win(&format!("NET +${}", thousands(f.ngr))),
                false => theme.lose(&format!("NET -${}", thousands(-f.ngr))),
            }
        ),
    ];
    if let (Some((best, up)), Some((worst, down))) = (f.best_game, f.worst_game) {
        out.push(String::new());
        out.push(theme.dim(&format!(
            "     best {} {} · worst {} {}",
            best,
            signed(theme, up),
            worst,
            signed(theme, down)
        )));
    }
    out.push(theme.dim("     [enter] the whole ledger"));
    out
}

/// Section 3 — the tournaments, and what the room has been up to.
fn events(theme: &Theme, view: &Snapshot, focus: &mut Focus, focused: bool, width: usize, rows: usize) -> Vec<String> {
    let mut out = Vec::new();
    // Tournaments first: they are the thing on this panel that can be
    // selected and opened, so they must be where the cursor is.
    if view.tournaments.is_empty() {
        out.push(theme.dim("     no tournament on — [t] starts one"));
    } else {
        focus.tourney = focus.tourney.min(view.tournaments.len() - 1);
        let show = view.tournaments.len().min(3.max(rows / 3));
        for (i, t) in view.tournaments.iter().take(show).enumerate() {
            let marker = if focused && i == focus.tourney { theme.paint(ui::theme::GOLD, " ▸ ") } else { "   ".to_string() };
            out.push(format!(
                "{marker}{} {} {} {}",
                pad_end(&theme.paint(ui::theme::MAGENTA, &format!("T-{:03}", t.id)), 8),
                pad_end(t.kind.label(), 12),
                pad_start(&format!("{}/{}", t.alive, t.entered), 9),
                pad_end(&theme.dim(&t.stage.label()), 16),
            ));
            out.push(clip(
                &theme.dim(&format!(
                    "       pool {} · buy-in {} · {}{}",
                    thousands(t.pool),
                    thousands(t.buy_in),
                    duration(t.elapsed),
                    match &t.leader {
                        Some((who, stack)) => format!(" · leader {who} {}", thousands(*stack)),
                        None => String::new(),
                    }
                )),
                width,
            ));
        }
    }

    let left = rows.saturating_sub(out.len() + 1);
    if left >= 2 && !view.events.is_empty() {
        out.push(theme.dim("     ON THE FLOOR"));
        // Only what the *simulation* judged worth reading gets this far —
        // the snapshot carries nothing below `Notable`, so the per-round
        // traffic that would drown this panel never arrives at all.
        let start = view.events.len().saturating_sub(left.saturating_sub(1));
        for r in view.events.iter().skip(start) {
            let line = r.event.describe();
            let text = if r.weight >= Weight::Major {
                theme.paint(ui::theme::GOLD, &line)
            } else {
                theme.dim(&line)
            };
            out.push(clip(&format!("     {} {}", theme.dim(&duration(r.at)), text), width));
        }
    }
    out
}

/// Section 4 — the front desk: who is in, and what has crossed the counter.
fn reception(theme: &Theme, view: &Snapshot, focus: &mut Focus, focused: bool, width: usize, rows: usize) -> Vec<String> {
    let r = &view.reception;
    let mut out = vec![
        format!(
            "     in {} · entered {} · left {} · chips in play {}",
            theme.accent(&r.visitors.to_string()),
            theme.dim(&r.entered.to_string()),
            theme.dim(&r.left.to_string()),
            theme.paint(ui::theme::GOLD, &thousands(r.chips_in_play))
        ),
        format!(
            "     buy-ins ${} · cash-outs ${} · net {}",
            thousands(r.bought_in),
            thousands(r.cashed_out),
            signed(theme, r.net_flow())
        ),
    ];
    if width >= 70 {
        out.push(theme.dim(&format!(
            "     {} chips issued · {} taken back · at {}/{} to the dollar",
            thousands(r.chips_out),
            thousands(r.chips_in),
            crate::economy::CHIPS_PER_DOLLAR,
            crate::economy::CHIPS_PER_DOLLAR_SELL
        )));
        if r.biggest_buy.0 > 0 || r.biggest_cash.0 > 0 {
            out.push(theme.dim(&format!(
                "     biggest in ${} {} · biggest out ${} {}",
                thousands(r.biggest_buy.0),
                r.biggest_buy.1,
                thousands(r.biggest_cash.0),
                r.biggest_cash.1
            )));
        }
    }

    // The room is split between who is here and what just happened at the
    // counter. Both shrink together on a small panel rather than one
    // pushing the other out of it.
    let room = rows.saturating_sub(out.len());
    let guests = (room / 2).max(1);
    if !r.guests.is_empty() {
        out.push(theme.dim(&format!(
            "     {} {} {} {}",
            pad_end("GUEST", 20),
            pad_end("WHERE", 18),
            pad_start("CHIPS", 10),
            pad_start("NET", 9)
        )));
        focus.guest = focus.guest.min(r.guests.len() - 1);
        let show = guests.saturating_sub(1).max(1);
        let first = focus.guest.saturating_sub(show / 2).min(r.guests.len().saturating_sub(show));
        for (i, g) in r.guests.iter().enumerate().skip(first).take(show) {
            let marker = if focused && i == focus.guest { theme.paint(ui::theme::GOLD, " ▸ ") } else { "   ".to_string() };
            // A guest of the house's top standing is picked out, because
            // that is the one thing about a name an operator reads first.
            let name = if view.is_top_tier(g.tier) {
                theme.paint(ui::theme::MAGENTA, &pad_end(&g.name, 20))
            } else {
                pad_end(&g.name, 20)
            };
            out.push(clip(
                &format!(
                    "{marker}{} {} {} {}",
                    name,
                    pad_end(&theme.dim(&g.where_now), 18),
                    pad_start(&thousands(g.chips), 10),
                    pad_start(&signed(theme, g.net), 9)
                ),
                width,
            ));
        }
    }

    let left = rows.saturating_sub(out.len());
    if left >= 2 && !r.activity.is_empty() {
        out.push(theme.dim("     AT THE CAGE"));
        let start = r.activity.len().saturating_sub(left.saturating_sub(1));
        for v in r.activity.iter().skip(start) {
            // The wording is this screen's; the *kind* is the desk's. A
            // movement the desk marked worth a look is picked out, and the
            // rest stay quiet.
            let said = match v.what {
                Cage::Standing => format!("{} — {}", v.what.label(), v.note),
                Cage::BuyIn | Cage::BigBuyIn => format!("{} — ${} for {}", v.what.label(), thousands(v.cash), thousands(v.chips)),
                Cage::CashOut | Cage::BigCashOut => {
                    format!("{} — {} chips for ${}", v.what.label(), thousands(v.chips), thousands(v.cash))
                }
            };
            let what = if v.what.notable() {
                theme.paint(ui::theme::GOLD, &said)
            } else {
                theme.dim(&said)
            };
            let who = if width >= 70 {
                pad_end(&format!("#{} {}", v.patron, v.who), 22)
            } else {
                pad_end(&format!("#{}", v.patron), 7)
            };
            out.push(clip(&format!("     {} {} {}", theme.dim(&duration(v.at)), who, what), width));
        }
    }
    out
}

// ------------------------------------------------------------ drill-downs

/// The books, one level down.
///
/// This is a *summary* of the accounting, not a second copy of it: every
/// figure is asked of the bank and the analytics, and the full ledger —
/// every movement, every expense, what the room is playing — is one more
/// keypress away on the screen that already drew it. Nothing here adds
/// anything up that was not already added up.
fn finances(manager: &Manager, screen: &mut Screen) {
    loop {
        let view = manager.dashboard();
        let f = &view.financials;
        let theme = screen.theme;
        let (cols, rows) = screen.size();
        let width = (cols as usize).max(40);
        screen.begin();
        ui::header(screen, "CASINO FINANCIALS");
        screen.blank();

        let badge = match f.standing() {
            Standing::Profitable => theme.win(Standing::Profitable.label()),
            Standing::Loss => theme.lose(Standing::Loss.label()),
            Standing::BreakEven => theme.paint(ui::theme::YELLOW, Standing::BreakEven.label()),
        };
        screen.line(&clip(
            &spread(
                &format!(
                    "  money {} · chips {}",
                    theme.win(&format!("${}", thousands(f.money))),
                    theme.paint(ui::theme::GOLD, &thousands(f.chips))
                ),
                &format!("{badge}  "),
                width,
            ),
            width,
        ));
        screen.blank();

        screen.line(&theme.dim("  PROFITABILITY"));
        row(screen, &theme, "gross gaming revenue", &signed(&theme, f.ggr), "chips", width);
        row(screen, &theme, "player payouts", &thousands(f.payouts), "chips", width);
        row(screen, &theme, "operating expenses", &format!("${}", thousands(f.spent)), "cash", width);
        row(
            screen,
            &theme,
            "net",
            &match f.ngr >= 0 {
                true => theme.win(&format!("+${}", thousands(f.ngr))),
                false => theme.lose(&format!("-${}", thousands(-f.ngr))),
            },
            "cash",
            width,
        );
        screen.blank();

        screen.line(&theme.dim("  ACTIVITY"));
        row(screen, &theme, "total wagered", &thousands(f.handle), "chips", width);
        row(screen, &theme, "total paid out", &thousands(f.payouts), "chips", width);
        row(screen, &theme, "rounds dealt", &thousands(f.rounds as i64), "", width);
        row(screen, &theme, "average bet", &thousands(f.average_bet()), "chips", width);
        row(screen, &theme, "at the tables", &signed(&theme, f.table_profit), "chips", width);
        row(screen, &theme, "at the cage", &signed(&theme, f.cage_profit), "cash", width);
        screen.blank();

        screen.line(&theme.dim("  THE CAGE"));
        row(screen, &theme, "player buy-ins", &format!("${}", thousands(f.bought_in)), "cash", width);
        row(screen, &theme, "player cash-outs", &format!("${}", thousands(f.cashed_out)), "cash", width);
        row(screen, &theme, "net cage flow", &signed(&theme, f.net_cage_flow()), "cash", width);
        screen.blank();

        // Which games carried the night, off the analytics that were
        // written once per round rather than recomputed here.
        if !f.games.is_empty() && rows > 30 {
            screen.line(&theme.dim(&format!(
                "  {} {} {} {}",
                pad_end("  GAME", 16),
                pad_start("HANDLE", 14),
                pad_start("WIN", 14),
                pad_start("HOLD", 9)
            )));
            for (game, t) in f.games.iter().take(6) {
                screen.line(&clip(
                    &format!(
                        "  {} {} {} {}",
                        pad_end(game, 16),
                        pad_start(&thousands(t.handle), 14),
                        pad_start(&signed(&theme, t.ggr()), 14),
                        pad_start(&theme.dim(&super::percent(t.hold())), 9)
                    ),
                    width,
                ));
            }
            screen.blank();
        }
        if let (Some((best, up)), Some((worst, down))) = (f.best_game, f.worst_game) {
            screen.line(&clip(
                &format!(
                    "  most profitable {} · least profitable {}",
                    theme.win(&format!("{best} {}", thousands(up))),
                    theme.lose(&format!("{worst} {}", thousands(down)))
                ),
                width,
            ));
        }
        if !f.by_expense.is_empty() {
            let bills: Vec<String> = f.by_expense.iter().map(|(k, v)| format!("{k} ${}", thousands(*v))).collect();
            screen.line(&clip(&theme.dim(&format!("  costs: {}", bills.join(" · "))), width));
        }
        if !f.shape.is_empty() {
            // The night's handle, bucket by bucket — the shape of it,
            // straight off the series the analytics already keep.
            let peak = f.shape.iter().copied().max().unwrap_or(0).max(1);
            let bars: String = f
                .shape
                .iter()
                .map(|h| ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][((h * 7 / peak).clamp(0, 7)) as usize])
                .collect();
            screen.line(&clip(&theme.dim(&format!("  the night so far: {bars}")), width));
        }

        screen.blank();
        screen.line(&widgets::footer(&theme, &[('m', "the whole ledger"), ('q', "back")]));
        screen.present();
        match poll(REFRESH, &['m']) {
            Poll::Leave => return,
            // The full books — movements, expenses and what the room is
            // playing — on the screen that has always drawn them.
            Poll::Pressed('m') => super::books(manager, screen),
            _ => {}
        }
    }
}

/// A label, a figure and what the figure is in. Every line of the
/// financial screens is one of these, so the columns line up.
fn row(screen: &mut Screen, theme: &Theme, label: &str, value: &str, unit: &str, width: usize) {
    screen.line(&clip(
        &format!("  {} {} {}", pad_end(label, 24), pad_start(value, 16), theme.dim(unit)),
        width,
    ));
}

/// One guest's visit, in full.
///
/// Every figure is the roster's — this screen looks somebody up by id and
/// reads what the casino already remembers about them. It writes nothing.
fn guest(manager: &Manager, screen: &mut Screen, id: u64) {
    loop {
        let view = manager.dashboard();
        let Some(g) = view.reception.guests.iter().find(|g| g.id == id).cloned() else {
            // They cashed out and went home while being looked at, which
            // is a thing that is allowed to happen.
            return;
        };
        let theme = screen.theme;
        let (cols, _) = screen.size();
        let width = (cols as usize).max(40);
        screen.begin();
        ui::header(screen, &format!("GUEST #{}", g.id));
        screen.blank();
        screen.line(&clip(&format!("  {}", theme.bold(&g.name)), width));
        screen.line(&clip(&theme.dim(&format!("  plays like a {}", g.style)), width));
        screen.blank();
        screen.line(&clip(
            &format!(
                "  standing        {}",
                theme.paint(ui::theme::GOLD, view.tier_name(g.tier))
            ),
            width,
        ));
        screen.line(&clip(&format!("  where           {}", theme.accent(&g.where_now)), width));
        screen.blank();
        screen.line(&theme.dim("  THIS VISIT"));
        screen.line(&clip(
            &format!(
                "  brought to the cage   {}",
                theme.dim(&format!("{} chips", thousands(g.bought)))
            ),
            width,
        ));
        screen.line(&clip(&format!("  holding now           {}", theme.paint(ui::theme::GOLD, &thousands(g.chips))), width));
        screen.line(&clip(
            &format!(
                "  worth at the counter  {}",
                theme.dim(&format!("about ${}", thousands(g.cash_value())))
            ),
            width,
        ));
        screen.line(&clip(&format!("  up or down            {}", signed(&theme, g.net)), width));
        screen.line(&clip(
            &format!(
                "  rounds played         {} · staked {}",
                theme.dim(&thousands(g.rounds as i64)),
                theme.dim(&thousands(g.staked))
            ),
            width,
        ));
        screen.blank();
        screen.line(&theme.dim("  EVERY VISIT THEY HAVE EVER MADE"));
        screen.line(&clip(
            &format!(
                "  visits {} · career {}",
                theme.accent(&g.visits.to_string()),
                signed(&theme, g.lifetime_net)
            ),
            width,
        ));
        screen.blank();
        screen.line(&clip(
            &theme.dim(&format!(
                "  chips are sold at {} to the dollar and bought back at {} — the spread is the cage's",
                crate::economy::CHIPS_PER_DOLLAR,
                crate::economy::CHIPS_PER_DOLLAR_SELL
            )),
            width,
        ));
        screen.blank();
        screen.line(&widgets::footer(&theme, &[('q', "back to the dashboard")]));
        screen.present();
        if let Poll::Leave = poll(REFRESH, &[]) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casino::dashboard::Layout;

    #[test]
    fn the_layout_follows_the_window_rather_than_a_fixed_width() {
        assert_eq!(Layout::for_size(200, 60), Layout::Grid, "a big window shows all four side by side");
        assert_eq!(Layout::for_size(70, 60), Layout::Stacked, "tall and narrow stacks them");
        assert_eq!(Layout::for_size(70, 18), Layout::Tabs, "a small window tabs between them");
        assert_eq!(Layout::for_size(200, 14), Layout::Tabs, "so does a very short one");
    }

    #[test]
    fn a_clipped_line_never_exceeds_the_window() {
        let theme = Theme::new(true);
        let painted = format!("  {} and then some more text", theme.win("+1,000"));
        for w in [4usize, 10, 20, 40] {
            assert!(ui::visible_len(&clip(&painted, w)) <= w, "clipped to {w} but still too wide");
        }
    }

    #[test]
    fn two_panels_sit_side_by_side_without_drifting() {
        let left = vec!["a".to_string(), "bbbb".to_string()];
        let right = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        let joined = beside(&left, &right, 8, 2);
        assert_eq!(joined.len(), 3, "the taller column decides the height");
        // Every right-hand column starts in the same place, whatever the
        // left-hand line happened to hold — including where there was no
        // left-hand line at all.
        for (i, line) in joined.iter().enumerate() {
            let cols: Vec<char> = line.chars().collect();
            assert_eq!(cols[10], right[i].chars().next().unwrap(), "row {i} drifted");
        }
    }

    /// A dashboard with something in every section, so the panels have
    /// real rows to lay out rather than empty-state lines.
    fn busy() -> Snapshot {
        use crate::casino::dashboard::{FinancialView, GuestView, ReceptionView, TournamentView};
        use crate::casino::instance::{Kind, Limit};
        use crate::casino::reception::{Cage, Visit};
        use crate::casino::tournament::{Entrant, Payday, Stage};
        use std::time::Duration;

        let mut s = Snapshot {
            tiers: vec!["guest", "regular", "high roller", "whale"],
            financials: FinancialView {
                money: 1_842_500,
                chips: 684_200,
                handle: 4_821_000,
                payouts: 4_338_500,
                ggr: 482_500,
                hold: 1_001,
                bets: 5_726,
                rounds: 1_204,
                spent: 42_000,
                ngr: 69_300,
                by_expense: vec![("overhead", 24_000), ("staffing", 18_000)],
                bought_in: 128_400,
                cashed_out: 94_200,
                table_profit: 482_500,
                cage_profit: 34_200,
                best_game: Some(("slots", 182_400)),
                worst_game: Some(("baccarat", -41_000)),
                games: Vec::new(),
                shape: vec![10, 90, 40, 70, 20],
                ..FinancialView::default()
            },
            ..Snapshot::default()
        };
        for i in 0..24u32 {
            s.games.push(GameView {
                id: i + 1,
                kind: Kind::Blackjack,
                name: format!("Blackjack #{i} ★"),
                round: 482 + i as u64,
                seats: 5,
                wanted: 7,
                status: if i % 3 == 0 { "paused" } else { "running" },
                limit: if i % 4 == 0 { Limit::High } else { Limit::House },
                pot: 12_500,
                house: -2_400,
                staked: 1_284_000,
                take: 84_200,
                idle_for: Duration::from_secs(i as u64),
                why: "a whale is winning",
                interest: 420,
            });
        }
        for i in 0..3u32 {
            s.tournaments.push(TournamentView {
                id: i + 1,
                name: format!("Blackjack Tournament #{i}"),
                kind: Kind::Blackjack,
                stage: if i == 0 { Stage::Done } else { Stage::Running(32) },
                round: 3,
                alive: 32,
                entered: 128,
                pool: 125_000,
                rake: 12_800,
                buy_in: 1_000,
                elapsed: Duration::from_secs(6_138),
                leader: Some(("Marcus".into(), 182_400)),
                board: (0..12)
                    .map(|n| Entrant { patron: n, name: format!("Runner {n}"), stack: 100_000 - n as i64, out_in: None })
                    .collect(),
                paid: vec![Payday { patron: 1, name: "Marcus".into(), place: 1, prize: 62_500 }],
                biggest_stack: 182_400,
                smallest_stack: 1_200,
                average_stack: 40_000,
            });
        }
        s.reception = ReceptionView {
            visitors: 47,
            entered: 182,
            left: 135,
            bought_in: 128_400,
            cashed_out: 94_200,
            chips_out: 1_284_000,
            chips_in: 942_000,
            chips_in_play: 684_200,
            biggest_buy: (75_000, "Marcus".into()),
            biggest_cash: (41_000, "Daniel".into()),
            activity: (0..24)
                .map(|n| Visit {
                    at: Duration::from_secs(n * 7),
                    patron: n,
                    who: format!("Guest {n}"),
                    what: [Cage::BuyIn, Cage::CashOut, Cage::BigBuyIn, Cage::Standing][(n % 4) as usize],
                    chips: 50_000,
                    cash: 5_000,
                    note: "high roller".into(),
                })
                .collect(),
            guests: (0..24)
                .map(|n| GuestView {
                    id: n,
                    name: format!("Guest {n}"),
                    style: "a whale",
                    tier: (n % 4) as usize,
                    where_now: format!("Blackjack #{n}"),
                    chips: 25_000,
                    bought: 50_000,
                    net: -1_200,
                    visits: 4,
                    rounds: 18,
                    staked: 142_000,
                    lifetime_net: 16_200,
                })
                .collect(),
        };
        s
    }

    #[test]
    fn no_panel_ever_draws_wider_than_the_box_it_was_given() {
        // The promise the spec calls "never allow text to blindly
        // overflow", checked at every width a person might actually use —
        // including ones far too small for the content.
        let screen = Screen::headless();
        let view = busy();
        for width in [30usize, 40, 52, 64, 80, 100, 120, 200] {
            for height in [3usize, 6, 10, 20, 40] {
                for section in Section::ALL {
                    let mut focus = Focus::default();
                    focus.set(section);
                    let lines = panel(&screen, &view, &mut focus, section, width, height);
                    assert!(lines.len() <= height, "{section:?} drew {} lines into a box of {height}", lines.len());
                    for line in lines.iter() {
                        assert!(
                            ui::visible_len(&clip(line, width)) <= width,
                            "{section:?} overflowed a {width}-column box"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_empty_casino_draws_every_section_without_panicking() {
        // The other end of the same promise: a dashboard opened before
        // anything has happened must still be a dashboard.
        let screen = Screen::headless();
        let view = Snapshot::default();
        for section in Section::ALL {
            let mut focus = Focus::default();
            focus.set(section);
            let lines = panel(&screen, &view, &mut focus, section, 80, 12);
            assert!(!lines.is_empty(), "{section:?} drew nothing at all");
        }
    }

    #[test]
    fn a_cursor_past_the_end_of_a_shrinking_list_is_pulled_back() {
        // Tables close under a dashboard all the time. The cursor must
        // follow the list down rather than index off the end of it.
        let screen = Screen::headless();
        let mut view = busy();
        let mut focus = Focus::default();
        focus.game = 23;
        focus.guest = 23;
        focus.tourney = 2;
        view.games.truncate(2);
        view.tournaments.truncate(1);
        view.reception.guests.truncate(1);
        for section in Section::ALL {
            focus.set(section);
            let _ = panel(&screen, &view, &mut focus, section, 80, 12);
        }
        assert!(focus.game < 2);
        assert!(focus.guest < 1 + 1);
        assert!(focus.tourney < 1);
    }

    #[test]
    fn stepping_wraps_around_and_never_indexes_off_an_empty_list() {
        let mut focus = Focus::default();
        let view = busy();
        focus.set(Section::Games);
        step(&mut focus, &view, -1);
        assert_eq!(focus.game, view.games.len() - 1, "up from the top wraps to the bottom");
        step(&mut focus, &view, 1);
        assert_eq!(focus.game, 0, "and back round again");

        // An empty floor: stepping must be a no-op, not a panic.
        let empty = Snapshot::default();
        for section in Section::ALL {
            focus.set(section);
            step(&mut focus, &empty, 1);
            step(&mut focus, &empty, -1);
        }
    }

    #[test]
    fn the_focus_keeps_a_cursor_per_section() {
        let mut f = Focus::default();
        f.set(Section::Games);
        if let Some(c) = f.cursor() {
            *c = 4;
        }
        f.set(Section::Reception);
        assert_eq!(f.cursor().copied(), Some(0), "a fresh section starts at the top");
        f.set(Section::Games);
        assert_eq!(f.cursor().copied(), Some(4), "and going back keeps your place");
        f.set(Section::Financials);
        assert!(f.cursor().is_none(), "the books are not a list");
    }
}
