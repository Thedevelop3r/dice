//! dice_arena — a casino console: Pig, Yahtzee, Chuck-a-Luck, Luck Bet,
//! Roulette, Blackjack, a knockout Tournament and a free-rolling Dice Lab.
//!
//! Everything on screen is driven by single keypresses — no typing a
//! letter and hitting Enter — and every screen is a hard clear-and-redraw,
//! so the terminal never fills up with scrollback. See `src/ui/` for the
//! whole presentation layer; game logic below never touches the terminal
//! directly.

mod dice;
mod economy;
mod games;
mod history;
mod rng;
mod shop;
mod stats;
mod ui;

use games::{blackjack, chuck, lab, luckbet, pig, roulette, tournament, ultra, yahtzee, Ctx, Player};
use rng::Rng;
use stats::Store;
use ui::menu::{choose_from, MenuItem};
use ui::widgets;
use ui::{Screen, Theme};

fn main() {
    let mut store = Store::load();
    let mut rng = match std::env::var("DICE_SEED").ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(seed) => Rng::from_seed(seed),
        None => Rng::new(),
    };
    let colors = store.get_i64("cfg.colors", 1) == 1;
    let mut screen = Screen::open(colors);

    if store.get_str("player.name", "").is_empty() {
        screen.begin();
        ui::header(&mut screen, "WELCOME TO THE ARENA");
        screen.blank();
        screen.line("  what should we call you?");
        let name = widgets::text_input(&mut screen, "Player", 18, |s, buf| {
            s.begin();
            ui::header(s, "WELCOME TO THE ARENA");
            s.blank();
            s.line("  what should we call you?");
            s.blank();
            s.line(&format!("  > {buf}█"));
            s.blank();
            s.line(&s.theme.dim("type a name, then press Enter"));
        });
        store.set_str("player.name", &name);
        let _ = store.save();
    }

    'app: loop {
        if ui::quit_requested() {
            break;
        }
        let name = store.get_str("player.name", "Player");
        let (chips, dollars) = {
            let w = economy::Wallet::new(&mut store);
            (w.chips(), w.dollars())
        };
        screen.set_colors(store.get_i64("cfg.colors", 1) == 1);
        let theme = screen.theme;

        screen.begin();
        draw_banner(&mut screen, &theme);
        screen.line(&format!("  welcome back, {}", theme.bold(&name)));
        screen.line(&format!("  {}", ui::money(&theme, chips, dollars)));

        let items = vec![
            MenuItem::new('1', "Pig", "press-your-luck race to a target score"),
            MenuItem::new('2', "Yahtzee", "13-category scorecard classic"),
            MenuItem::new('3', "Luck Bet", "8 dice A-H, back a letter and a face"),
            MenuItem::new('4', "Luck Bet: Turbo", "hands-free — press to spin the table"),
            MenuItem::new('5', "Chuck-a-Luck", "three-dice wagering"),
            MenuItem::new('6', "Roulette", "single-zero wheel, chips on the felt"),
            MenuItem::new('7', "Blackjack", "beat the dealer to 21"),
            MenuItem::new('8', "Tournament", "knockout bracket for a chip prize pool"),
            MenuItem::new('9', "Dice Lab", "roll any NdM+K, sample distributions"),
            MenuItem::new('0', "Ultra Casino Dice", "idle-screen spectacle — 8 AI players, 12 dice, fully automatic"),
            MenuItem::new('s', "Store", "chips, dollars and lucky charms"),
            MenuItem::new('i', "Stats", "your history across every table"),
            MenuItem::new('h', "History", "past Ultra Casino Dice sessions"),
            MenuItem::new('o', "Options", "name, colors, reset"),
            MenuItem::new('r', "Rules", "how each game is played"),
            MenuItem::new('q', "Quit", "cash out and leave"),
        ];
        let choice = choose_from(&mut screen, "THE FLOOR", &items);

        match choice {
            Some('1') => {
                let Some(players) = setup_players(&mut screen, &mut store, &mut rng, &name, 2, 6) else { continue 'app };
                let Some(target) = pick_amount(&mut screen, "target score", 20, 500, 100, 10, &[]) else { continue 'app };
                screen.begin();
                ui::header(&mut screen, "PIG");
                screen.line("  two-dice variant? a single 1 ends your turn, snake eyes wipes your");
                screen.line("  score, doubles pay double.");
                screen.line(&widgets::footer(&screen.theme, &[('y', "two-dice"), ('n', "classic")]));
                screen.present();
                let two_dice = ui::confirm_key(false);
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                pig::play(&mut ctx, players, pig::Config { target, two_dice });
            }
            Some('2') => {
                let Some(players) = setup_players(&mut screen, &mut store, &mut rng, &name, 1, 4) else { continue 'app };
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                yahtzee::play(&mut ctx, players);
            }
            Some('3') | Some('4') => {
                let turbo = choice == Some('4');
                let opponents = if turbo {
                    2
                } else {
                    match pick_amount(&mut screen, "CPU opponents", 0, 2, 2, 1, &[]) {
                        Some(v) => v as usize,
                        None => continue 'app,
                    }
                };
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                luckbet::play(&mut ctx, opponents, turbo);
            }
            Some('5') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                chuck::play(&mut ctx);
            }
            Some('6') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                roulette::play(&mut ctx);
            }
            Some('7') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                blackjack::play(&mut ctx);
            }
            Some('8') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                tournament::play(&mut ctx);
            }
            Some('9') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                lab::play(&mut ctx);
            }
            Some('0') => {
                let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
                ultra::play(&mut ctx);
            }
            Some('s') => shop::open(&mut store, &mut screen),
            Some('i') => show_stats(&mut store, &mut screen),
            Some('h') => show_history(&mut screen),
            Some('o') => settings(&mut store, &mut screen),
            Some('r') => rules(&mut screen),
            Some('q') | None => break,
            _ => {}
        }
    }

    let _ = store.save();
    drop(screen);
    println!("\n  thanks for playing — the house always keeps the lights on.\n");
}

fn draw_banner(screen: &mut Screen, theme: &Theme) {
    let art = [
        " ██████╗ ██╗ ██████╗███████╗     █████╗ ██████╗ ███████╗███╗   ██╗ █████╗ ",
        " ██╔══██╗██║██╔════╝██╔════╝    ██╔══██╗██╔══██╗██╔════╝████╗  ██║██╔══██╗",
        " ██║  ██║██║██║     █████╗      ███████║██████╔╝█████╗  ██╔██╗ ██║███████║",
        " ██║  ██║██║██║     ██╔══╝      ██╔══██║██╔══██╗██╔══╝  ██║╚██╗██║██╔══██║",
        " ██████╔╝██║╚██████╗███████╗    ██║  ██║██║  ██║███████╗██║ ╚████║██║  ██║",
        " ╚═════╝ ╚═╝ ╚═════╝╚══════╝    ╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═══╝╚═╝  ╚═╝",
    ];
    for line in art {
        screen.line(&theme.paint(ui::theme::GOLD, line));
    }
    screen.line(&theme.dim("            ♠ ♥ ♦ ♣   every table live, every roll animated   ♣ ♦ ♥ ♠"));
    screen.blank();
}

/// A key-driven numeric picker with a consistent header/footer, for the
/// handful of places a bounded quantity is genuinely needed (target
/// score, player counts, bracket picks feed through their own screens).
pub fn pick_amount(screen: &mut Screen, label: &str, min: i64, max: i64, default: i64, step: i64, presets: &[(char, i64, &str)]) -> Option<i64> {
    let preset_keys: Vec<(char, i64)> = presets.iter().map(|(k, v, _)| (*k, *v)).collect();
    widgets::number_picker(screen, min, max, default, step, &preset_keys, |s, value| {
        let theme = s.theme;
        s.begin();
        ui::header(s, &label.to_uppercase());
        s.blank();
        s.line(&format!("   {}", theme.paint(ui::theme::GOLD, &format!("{value}"))));
        s.blank();
        s.line(&format!("  {}  {}", theme.paint(ui::theme::GOLD, "[↑/→]"), theme.dim("increase")));
        s.line(&format!("  {}  {}", theme.paint(ui::theme::GOLD, "[↓/←]"), theme.dim("decrease")));
        s.line(&format!("  {}  {}", theme.paint(ui::theme::GOLD, "[0-9]"), theme.dim("type a value")));
        for (k, _, l) in presets {
            s.line(&format!("  {}  {}", theme.paint(ui::theme::GOLD, &format!("[{}]", k.to_ascii_uppercase())), theme.dim(l)));
        }
        s.blank();
        s.line(&widgets::footer(&theme, &[('\u{23ce}', "confirm"), ('\u{238b}', "cancel")]));
    })
}

/// Builds the roster: the human player plus any humans/bots they add.
fn setup_players(screen: &mut Screen, store: &mut Store, rng: &mut Rng, name: &str, min: usize, max: usize) -> Option<Vec<Player>> {
    let total = pick_amount(screen, "how many players", min as i64, max as i64, min.max(2) as i64, 1, &[])? as usize;
    let mut players = vec![Player::human(name)];
    for i in 2..=total {
        screen.begin();
        ui::header(screen, &format!("PLAYER {i}"));
        screen.blank();
        screen.line(&widgets::footer(&screen.theme, &[('h', "human"), ('c', "computer")]));
        screen.present();
        let is_bot = ui::choose_key(&['h', 'c'], 'h') == Some('c');
        if is_bot {
            let mut ctx = Ctx { rng, store, screen };
            let d = games::pick_difficulty(&mut ctx, &format!("PLAYER {i} DIFFICULTY"));
            players.push(Player::bot(&format!("CPU-{}", i - 1), d));
        } else {
            screen.begin();
            ui::header(screen, &format!("PLAYER {i}"));
            let n = widgets::text_input(screen, &format!("Player {i}"), 16, |s, buf| {
                s.begin();
                ui::header(s, &format!("PLAYER {i}"));
                s.blank();
                s.line(&format!("  name: {buf}█"));
            });
            players.push(Player::human(&n));
        }
    }
    Some(players)
}

fn show_stats(store: &mut Store, screen: &mut Screen) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "STATISTICS");
    let rows = [
        ("Wallet", "econ"),
        ("Pig", "pig"),
        ("Yahtzee", "yahtzee"),
        ("Luck Bet", "luckbet"),
        ("Chuck-a-Luck", "chuck"),
        ("Roulette", "roulette"),
        ("Blackjack", "blackjack"),
        ("Tournament", "tourney"),
        ("Dice Lab", "lab"),
        ("Ultra Casino Dice", "ultra"),
        ("Store", "store"),
        ("Inventory", "inv"),
    ];
    let mut any = false;
    for (label, key) in rows {
        let entries: Vec<(String, String)> = store
            .entries()
            .filter(|(k, _)| k.starts_with(&format!("{key}.")) && *k != "econ.init")
            .map(|(k, v)| (k.split_once('.').unwrap().1.to_string(), v.clone()))
            .collect();
        if entries.is_empty() {
            continue;
        }
        any = true;
        screen.blank();
        screen.line(&theme.bold(label));
        for (k, v) in entries {
            screen.line(&format!("   {:<16} {}", k, theme.paint(ui::theme::CYAN, &v)));
        }
    }
    if !any {
        screen.blank();
        screen.line(&theme.dim("no games played yet."));
    }
    screen.blank();
    screen.line(&theme.dim(&format!("saved at {}", store.path_display())));
    ui::pause(screen);
}

/// A simple paginated reader for `history.rs`'s plain-text Ultra Casino
/// Dice log — most recent entries first, `N`/`P` to page, any other key
/// to leave.
fn show_history(screen: &mut Screen) {
    let theme = screen.theme;
    let mut lines = history::read_all();
    lines.reverse();
    if lines.is_empty() {
        screen.begin();
        ui::header(screen, "HISTORY");
        screen.blank();
        screen.line(&theme.dim("  no sessions played yet — try Ultra Casino Dice from the floor."));
        ui::pause(screen);
        return;
    }

    let (_, rows) = screen.size();
    let page_size = (rows as usize).saturating_sub(10).clamp(8, 30);
    let total_pages = lines.len().div_ceil(page_size).max(1);
    let mut page = 0usize;
    loop {
        screen.begin();
        ui::header(screen, "HISTORY");
        screen.blank();
        let start = page * page_size;
        let end = (start + page_size).min(lines.len());
        for line in &lines[start..end] {
            screen.line(&format!("  {}", theme.dim(line)));
        }
        screen.blank();
        screen.line(&theme.dim(&format!("page {}/{total_pages} · {} lines · {}", page + 1, lines.len(), history::path_display())));
        screen.line(&widgets::footer(&theme, &[('n', "next page"), ('p', "prev page"), ('q', "back")]));
        screen.present();
        match ui::choose_key(&['n', 'p', 'q'], 'q') {
            Some('n') => page = (page + 1).min(total_pages.saturating_sub(1)),
            Some('p') => page = page.saturating_sub(1),
            _ => return,
        }
    }
}

fn settings(store: &mut Store, screen: &mut Screen) {
    loop {
        let colors_on = store.get_i64("cfg.colors", 1) == 1;
        screen.set_colors(colors_on);
        screen.begin();
        ui::header(screen, "OPTIONS");
        screen.blank();
        screen.line(&format!("  name    : {}", store.get_str("player.name", "Player")));
        screen.line(&format!("  colors  : {}", if colors_on { "on" } else { "off" }));
        screen.blank();
        let items = vec![
            MenuItem::new('n', "change name", ""),
            MenuItem::new('c', "toggle colors", ""),
            MenuItem::new('x', "reset statistics", "wipes stats and bankroll"),
            MenuItem::new('b', "back", ""),
        ];
        match choose_from(screen, "SETTINGS", &items) {
            Some('n') => {
                screen.begin();
                ui::header(screen, "OPTIONS");
                let n = widgets::text_input(screen, &store.get_str("player.name", "Player"), 18, |s, buf| {
                    s.begin();
                    ui::header(s, "OPTIONS");
                    s.blank();
                    s.line(&format!("  new name: {buf}█"));
                });
                store.set_str("player.name", &n);
            }
            Some('c') => store.set_i64("cfg.colors", if colors_on { 0 } else { 1 }),
            Some('x') => {
                screen.begin();
                ui::header(screen, "OPTIONS");
                screen.blank();
                screen.line("  wipe all stats and bankroll?");
                screen.line(&widgets::footer(&screen.theme, &[('y', "wipe it"), ('n', "cancel")]));
                screen.present();
                if ui::confirm_key(false) {
                    store.reset();
                    screen.blank();
                    screen.line(&screen.theme.win("stats cleared."));
                    screen.present();
                    ui::sleep_ms(500);
                }
            }
            _ => {
                let _ = store.save();
                return;
            }
        }
        let _ = store.save();
    }
}

fn rules(screen: &mut Screen) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "RULES");
    let entries: [(&str, &str); 9] = [
        ("PIG", "Roll to build a turn total, hold to bank it. Roll a 1 and the turn total is gone. In the two-dice variant a single 1 ends the turn, snake eyes wipes your whole score, and doubles pay double. First to the target score wins."),
        ("YAHTZEE", "Thirteen rounds, three rolls each: keep dice between rolls, then commit the hand to one open category. 63+ in the upper section earns a 35-point bonus; each extra Yahtzee after the first is worth 100."),
        ("CHUCK-A-LUCK", "Stake chips on a face (pays 1:1, 2:1 or 3:1 by how many of the three dice show it), on HIGH (11-17) or LOW (4-10) at even money — both lose to any triple — or on any triple at 30:1."),
        ("LUCK BET", "Eight dice sit on the table, named A to H. Each player backs one letter and the face they think it will show, then the table is rolled. A hit pays 5:1 (6:1 with a VIP pass); a miss rakes a fifth of the stake into the jackpot. Land 3+ copies of your number and sweep the jackpot too. TURBO plays hands-free."),
        ("ROULETTE", "Single-zero wheel. Straight-up numbers pay 35:1, split/street/corner bets pay accordingly, and the outside bets (red/black, odd/even, high/low, dozens, columns) pay even money or 2:1."),
        ("BLACKJACK", "Beat the dealer's hand without going over 21. Face cards count 10, aces count 11 or 1. Dealer stands on 17. Blackjack (an ace + a ten-card on the deal) pays 3:2."),
        ("TOURNAMENT", "Pay a chip buy-in, get drawn into a single-elimination Pig bracket. Your matches are played out; the rest of the bracket is simulated. Champion takes 70% of the pool, runner-up 30%."),
        ("DICE LAB", "Roll dice notation like 4d6+2, `sim 2d6 50000` to plot the distribution, or `seed 42` for reproducible rolls."),
        ("ULTRA CASINO DICE", "An unattended spectacle table: 12 dice, 8 computer players, 10 rounds, no input required. Each round every seat backs a letter and calls a face, the table rolls for 7 seconds, and whoever called it right splits the pot — there's no house, so losers' stakes are exactly what winners collect. Seats decide to stay or cash out between rounds; anyone who leaves is replaced by a new AI player. Press Q at any time to let the round in progress finish and return to the floor; otherwise a finished session shows its final standings and a new one starts on its own. Every round and every session is logged to the History screen."),
    ];
    for (title, body) in entries {
        screen.blank();
        screen.line(&theme.bold(title));
        for line in wrap(body, 74) {
            screen.line(&format!("   {line}"));
        }
    }
    screen.blank();
    screen.line(&theme.dim("tip: set DICE_SEED=<n> in the environment to seed the whole session."));
    ui::pause(screen);
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.chars().count() + word.chars().count() + 1 > width {
            lines.push(cur.clone());
            cur.clear();
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}
