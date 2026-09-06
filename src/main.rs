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

use games::{
    baccarat, bigsix, bingo, blackjack, chuck, crash, floor, hilo, horses, keno, lab, luckbet, mines, pig, plinko, roulette, scratch, slots, threecard,
    tournament, ultra, vidpoker, war, yahtzee, Ctx, Player,
};
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

        // Two dozen tables will not fit on one readable screen, so the
        // floor is grouped the way a real one is and each room keeps its
        // own menu.
        let items = vec![
            MenuItem::new('1', "Dice Tables", "Pig, Yahtzee, Luck Bet, Chuck-a-Luck, the Tournament, the Lab"),
            MenuItem::new('2', "Card Tables", "Blackjack, Baccarat, Video Poker, Three Card, War, Hi-Lo"),
            MenuItem::new('3', "The Wheels", "Roulette and the Big Six money wheel"),
            MenuItem::new('4', "The Arcade", "slots, keno, bingo, plinko, mines, crash, scratch cards, the races"),
            MenuItem::new('5', "Idle Screens", "every table running itself — or the whole floor in turn"),
            MenuItem::new('s', "Store", "chips, dollars and lucky charms"),
            MenuItem::new('i', "Stats", "your history across every table"),
            MenuItem::new('h', "History", "past Ultra Casino Dice sessions"),
            MenuItem::new('c', "Casino", "the house's own wallet — profit and loss across every table"),
            MenuItem::new('o', "Options", "name, colors, reset"),
            MenuItem::new('r', "Rules", "how each game is played"),
            MenuItem::new('q', "Quit", "cash out and leave"),
        ];

        match choose_from(&mut screen, "THE FLOOR", &items) {
            Some('1') => dice_room(&mut store, &mut rng, &mut screen, &name),
            Some('2') => card_room(&mut store, &mut rng, &mut screen),
            Some('3') => wheel_room(&mut store, &mut rng, &mut screen),
            Some('4') => arcade_room(&mut store, &mut rng, &mut screen),
            Some('5') => idle_room(&mut store, &mut rng, &mut screen),
            Some('s') => shop::open(&mut store, &mut screen),
            Some('i') => show_stats(&mut store, &mut screen),
            Some('h') => show_history(&mut screen),
            Some('c') => show_house(&mut store, &mut screen),
            Some('o') => settings(&mut store, &mut screen),
            Some('r') => rules(&mut screen),
            Some('q') | None => break 'app,
            _ => {}
        }
    }

    let _ = store.save();
    drop(screen);
    println!("\n  thanks for playing — the house always keeps the lights on.\n");
}

/// Every table's stats prefix and the name it goes by, in floor order.
/// The Stats screen and the Casino books both read this list, so a new
/// table appears on both of them or on neither.
const TABLES: [(&str, &str); 23] = [
    ("Pig", "pig"),
    ("Yahtzee", "yahtzee"),
    ("Luck Bet", "luckbet"),
    ("Chuck-a-Luck", "chuck"),
    ("Tournament", "tourney"),
    ("Dice Lab", "lab"),
    ("Ultra Casino Dice", "ultra"),
    ("Blackjack", "blackjack"),
    ("Baccarat", "baccarat"),
    ("Video Poker", "vidpoker"),
    ("Three Card Poker", "threecard"),
    ("Casino War", "war"),
    ("Hi-Lo", "hilo"),
    ("Roulette", "roulette"),
    ("Big Six", "bigsix"),
    ("Slots", "slots"),
    ("Keno", "keno"),
    ("Bingo", "bingo"),
    ("Plinko", "plinko"),
    ("Mines", "mines"),
    ("Crash", "crash"),
    ("Scratch Cards", "scratch"),
    ("Horse Racing", "horses"),
];

/// The dice room: everything that settles on a pip face.
fn dice_room(store: &mut Store, rng: &mut Rng, screen: &mut Screen, name: &str) {
    loop {
        let items = vec![
            MenuItem::new('1', "Pig", "press-your-luck race to a target score"),
            MenuItem::new('2', "Yahtzee", "13-category scorecard classic"),
            MenuItem::new('3', "Luck Bet", "8 dice A-H, back a letter and a face"),
            MenuItem::new('4', "Luck Bet: Turbo", "hands-free — press to spin the table"),
            MenuItem::new('5', "Chuck-a-Luck", "three-dice wagering"),
            MenuItem::new('6', "Tournament", "knockout bracket for a chip prize pool"),
            MenuItem::new('7', "Dice Lab", "roll any NdM+K, sample distributions"),
            MenuItem::new('8', "Ultra Casino Dice", "the spectacle table — 8 AI players, 12 dice, fully automatic"),
            MenuItem::new('b', "back", "return to the floor"),
        ];
        match choose_from(screen, "DICE TABLES", &items) {
            Some('1') => {
                let Some(players) = setup_players(screen, store, rng, name, 2, 6) else { continue };
                let Some(target) = pick_amount(screen, "target score", 20, 500, 100, 10, &[]) else { continue };
                screen.begin();
                ui::header(screen, "PIG");
                screen.line("  two-dice variant? a single 1 ends your turn, snake eyes wipes your");
                screen.line("  score, doubles pay double.");
                screen.line(&widgets::footer(&screen.theme, &[('y', "two-dice"), ('n', "classic")]));
                screen.present();
                let two_dice = ui::confirm_key(false);
                let mut ctx = Ctx { rng, store, screen };
                pig::play(&mut ctx, players, pig::Config { target, two_dice });
            }
            Some('2') => {
                let Some(players) = setup_players(screen, store, rng, name, 1, 4) else { continue };
                let mut ctx = Ctx { rng, store, screen };
                yahtzee::play(&mut ctx, players);
            }
            Some(c @ ('3' | '4')) => {
                let turbo = c == '4';
                let opponents = if turbo {
                    2
                } else {
                    match pick_amount(screen, "CPU opponents", 0, 2, 2, 1, &[]) {
                        Some(v) => v as usize,
                        None => continue,
                    }
                };
                let mut ctx = Ctx { rng, store, screen };
                luckbet::play(&mut ctx, opponents, turbo);
            }
            Some('5') => chuck::play(&mut Ctx { rng, store, screen }),
            Some('6') => tournament::play(&mut Ctx { rng, store, screen }),
            Some('7') => lab::play(&mut Ctx { rng, store, screen }),
            Some('8') => ultra::play(&mut Ctx { rng, store, screen }),
            _ => return,
        }
    }
}

/// The card room. Every table here deals from `games::cards`.
fn card_room(store: &mut Store, rng: &mut Rng, screen: &mut Screen) {
    loop {
        let items = vec![
            MenuItem::new('1', "Blackjack", "beat the dealer to 21"),
            MenuItem::new('2', "Baccarat", "back the player, the banker or the tie"),
            MenuItem::new('3', "Video Poker", "jacks or better, one draw"),
            MenuItem::new('4', "Three Card Poker", "ante up against a dealer who needs queen high"),
            MenuItem::new('5', "Casino War", "high card wins — a tie means war"),
            MenuItem::new('6', "Hi-Lo", "call the next card, press your run or take the money"),
            MenuItem::new('b', "back", "return to the floor"),
        ];
        match choose_from(screen, "CARD TABLES", &items) {
            Some('1') => blackjack::play(&mut Ctx { rng, store, screen }),
            Some('2') => baccarat::play(&mut Ctx { rng, store, screen }),
            Some('3') => vidpoker::play(&mut Ctx { rng, store, screen }),
            Some('4') => threecard::play(&mut Ctx { rng, store, screen }),
            Some('5') => war::play(&mut Ctx { rng, store, screen }),
            Some('6') => hilo::play(&mut Ctx { rng, store, screen }),
            _ => return,
        }
    }
}

/// The two tables that settle on a spinning board.
fn wheel_room(store: &mut Store, rng: &mut Rng, screen: &mut Screen) {
    loop {
        let items = vec![
            MenuItem::new('1', "Roulette", "single-zero wheel, chips on the felt"),
            MenuItem::new('2', "Big Six", "the money wheel — 54 segments past a pointer"),
            MenuItem::new('b', "back", "return to the floor"),
        ];
        match choose_from(screen, "THE WHEELS", &items) {
            Some('1') => roulette::play(&mut Ctx { rng, store, screen }),
            Some('2') => bigsix::play(&mut Ctx { rng, store, screen }),
            _ => return,
        }
    }
}

/// Everything that is neither dice nor cards.
fn arcade_room(store: &mut Store, rng: &mut Rng, screen: &mut Screen) {
    loop {
        let items = vec![
            MenuItem::new('1', "Slots", "three weighted reels, stopping left to right"),
            MenuItem::new('2', "Keno", "cover up to ten spots, twenty balls come out"),
            MenuItem::new('3', "Bingo", "a 75-ball card — the faster your line, the more it pays"),
            MenuItem::new('4', "Plinko", "drop a ball through twelve rows of pegs"),
            MenuItem::new('5', "Mines", "turn over tiles and get out before you find one"),
            MenuItem::new('6', "Crash", "cash out before the multiplier goes"),
            MenuItem::new('7', "Scratch Cards", "nine panels, three of a kind pays"),
            MenuItem::new('8', "Horse Racing", "six runners at fixed odds"),
            MenuItem::new('b', "back", "return to the floor"),
        ];
        match choose_from(screen, "THE ARCADE", &items) {
            Some('1') => slots::play(&mut Ctx { rng, store, screen }),
            Some('2') => keno::play(&mut Ctx { rng, store, screen }),
            Some('3') => bingo::play(&mut Ctx { rng, store, screen }),
            Some('4') => plinko::play(&mut Ctx { rng, store, screen }),
            Some('5') => mines::play(&mut Ctx { rng, store, screen }),
            Some('6') => crash::play(&mut Ctx { rng, store, screen }),
            Some('7') => scratch::play(&mut Ctx { rng, store, screen }),
            Some('8') => horses::play(&mut Ctx { rng, store, screen }),
            _ => return,
        }
    }
}

/// The idle screens: any single table left running itself, or the whole
/// floor cycled in turn. Nothing in here touches the wallet or the house
/// ledger — every chip on screen is fictional twice over.
fn idle_room(store: &mut Store, rng: &mut Rng, screen: &mut Screen) {
    // Tables get 1-9 then a-h; the cycler and Ultra sit outside that range
    // so they can never collide with a table key.
    let key_for = |i: usize| -> char {
        if i < 9 {
            char::from_digit(i as u32 + 1, 10).unwrap()
        } else {
            (b'a' + (i - 9) as u8) as char
        }
    };
    loop {
        let mut items = vec![
            MenuItem::new('0', "The whole floor", "every table in turn, a slot each — the full screensaver"),
            MenuItem::new('u', "Ultra Casino Dice", "the original spectacle table — 8 AI players, 12 dice"),
        ];
        for (i, t) in floor::ATTRACT.iter().enumerate() {
            items.push(MenuItem::new(key_for(i), t.label, "watch this table run itself"));
        }
        items.push(MenuItem::new('z', "back", "return to the floor"));

        match choose_from(screen, "IDLE SCREENS", &items) {
            Some('0') => {
                games::table::idle_budget(None);
                floor::cycle(&mut Ctx { rng, store, screen });
            }
            Some('u') => ultra::play(&mut Ctx { rng, store, screen }),
            Some(c) => {
                let Some(i) = (0..floor::ATTRACT.len()).find(|i| key_for(*i) == c) else { return };
                floor::attract_one(&mut Ctx { rng, store, screen }, i);
            }
            None => return,
        }
    }
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
    let mut rows: Vec<(&str, &str)> = vec![("Wallet", "econ")];
    rows.extend_from_slice(&TABLES);
    rows.push(("Store", "store"));
    rows.push(("Inventory", "inv"));
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

/// The casino's own side of the ledger — the mirror image of the
/// player's wallet. See `economy::House` for exactly what feeds it: every
/// wagering game, and nothing else (no Store exchanges or purchases).
fn show_house(store: &mut Store, screen: &mut Screen) {
    let theme = screen.theme;
    screen.begin();
    ui::header(screen, "THE CASINO'S BOOKS");
    screen.blank();

    let balance = economy::House::balance(store);
    let (collected, paid) = economy::House::totals(store);
    let balance_str = if balance >= 0 { theme.win(&format!("+{balance} chips")) } else { theme.lose(&format!("{balance} chips")) };
    screen.line(&format!("  house balance: {balance_str}"));
    screen.line(&theme.dim(if collected == 0 && paid == 0 {
        "  the house hasn't settled a bet yet."
    } else if balance >= 0 {
        "  the house is ahead across every table."
    } else {
        "  players are collectively up on the house."
    }));
    screen.blank();
    screen.line(&format!("  collected from losing bets   {}", theme.paint(ui::theme::GREEN, &format!("{collected} chips"))));
    screen.line(&format!("  paid out on winning bets     {}", theme.paint(ui::theme::RED, &format!("{paid} chips"))));

    let mut any = false;
    screen.blank();
    screen.line(&theme.bold("by table"));
    for (label, key) in TABLES {
        // Only the tables that actually settle wagers book to the ledger,
        // so the presence of a `house_pl` key is what puts a row here —
        // Pig and the Dice Lab keep stats but never take a bet.
        let pl_key = format!("{key}.house_pl");
        if !store.entries().any(|(k, _)| *k == pl_key) {
            continue;
        }
        any = true;
        let pl = store.get_i64(&pl_key, 0);
        let pl_str = if pl >= 0 { theme.win(&format!("+{pl}")) } else { theme.lose(&format!("{pl}")) };
        screen.line(&format!("   {:<18} {:>10} chips", label, pl_str));
    }
    if !any {
        screen.line(&theme.dim("   no games played yet."));
    }
    screen.blank();
    screen.line(&theme.dim("  covers every wagering table in the building, plus unclaimed Ultra"));
    screen.line(&theme.dim("  Casino Dice pots — but not Store currency exchanges, item purchases,"));
    screen.line(&theme.dim("  or anything an idle screen does on its own."));
    ui::pause(screen);
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
    let entries: [(&str, &str); 23] = [
        ("PIG", "Roll to build a turn total, hold to bank it. Roll a 1 and the turn total is gone. In the two-dice variant a single 1 ends the turn, snake eyes wipes your whole score, and doubles pay double. First to the target score wins."),
        ("YAHTZEE", "Thirteen rounds, three rolls each: keep dice between rolls, then commit the hand to one open category. 63+ in the upper section earns a 35-point bonus; each extra Yahtzee after the first is worth 100."),
        ("CHUCK-A-LUCK", "Stake chips on a face (pays 1:1, 2:1 or 3:1 by how many of the three dice show it), on HIGH (11-17) or LOW (4-10) at even money — both lose to any triple — or on any triple at 30:1."),
        ("LUCK BET", "Eight dice sit on the table, named A to H. Each player backs one letter and the face they think it will show, then the table is rolled. A hit pays 5:1 (6:1 with a VIP pass); a miss rakes a fifth of the stake into the jackpot. Land 3+ copies of your number and sweep the jackpot too. TURBO plays hands-free."),
        ("TOURNAMENT", "Pay a chip buy-in, get drawn into a single-elimination Pig bracket. Your matches are played out; the rest of the bracket is simulated. Champion takes 70% of the pool, runner-up 30%."),
        ("DICE LAB", "Roll dice notation like 4d6+2, `sim 2d6 50000` to plot the distribution, or `seed 42` for reproducible rolls."),
        ("ULTRA CASINO DICE", "An unattended spectacle table: 12 dice, 8 computer players, 10 rounds, no input required. Each round every seat backs a letter and calls a face, the table rolls for 7 seconds, and whoever called it right splits the pot — losers' stakes are exactly what winners collect, with one exception: a round nobody calls right pays its whole pot to the house. Press Q to let the round in progress finish. Every round and every session is logged to the History screen."),
        ("BLACKJACK", "Beat the dealer's hand without going over 21. Face cards count 10, aces count 11 or 1. Dealer stands on 17. Blackjack (an ace + a ten-card on the deal) pays 3:2."),
        ("BACCARAT", "Back the player, the banker or the tie, then watch a hand nobody makes a decision in — the third-card rules are fixed. Cards count their pips, tens and courts count nothing, and only the last digit of a total matters. Player and banker pay 1:1, the banker less a 5% commission; the tie pays 8:1 and pushes the other two."),
        ("VIDEO POKER", "Jacks or better. Five cards, hold any of them, draw once. A pair only pays from jacks up; from there it climbs through two pair, trips, straight, flush, full house, quads and the straight flush to a royal at 800:1."),
        ("THREE CARD POKER", "Ante, look at three cards, then fold or match your ante to play on. The dealer needs queen high or better to play at all — if they cannot, your ante pays and the play bet comes back. Beat a qualified dealer and both bets pay even money. A straight, trips or straight flush also pays an ante bonus whether you win or lose."),
        ("CASINO WAR", "High card wins, aces high, even money. On a tie you can surrender for half your ante, or go to war: match the ante, burn three cards and deal again. Win the war and the raise pays even money while the ante pushes; tie again and the raise pays 2:1."),
        ("HI-LO", "A card is turned over; call whether the next one is higher or lower. Every correct call multiplies your stake by the true odds of that call less a small margin, so calling higher on a two is nearly free and calling higher on a king pays richly. Cash out whenever you like — one wrong call takes the lot, and a tie counts as wrong."),
        ("ROULETTE", "Single-zero wheel. Straight-up numbers pay 35:1, and the outside bets (red/black, odd/even, high/low, dozens) pay even money or 2:1."),
        ("BIG SIX", "The money wheel: 54 segments running past a pointer. Back the 1 (24 segments, 1:1), the 2 (15, 2:1), the 5 (7, 5:1), the 10 (4, 10:1), the 20 (2, 20:1), or either of the two single segments — the joker and the house star — at 40:1."),
        ("SLOTS", "Three weighted reels stopping left to right on one payline. Three of a kind pays by symbol, from 8x for three BARs up to 120x for three sevens. Two sevens anywhere pay 5x and a single seven pays 2x, so a broken line can still come home."),
        ("KENO", "Cover between one and ten spots on an eighty-number board, then twenty balls are drawn. What you are paid depends on both how many you covered and how many came out: cover one and a single hit pays 3x, cover ten and you need five before anything pays at all — but all ten pays 10,000x."),
        ("BINGO", "A 75-ball card with a free centre square. Forty balls are called and the payout is on how quickly your first line lands: by ball 15 it pays 30x, by 20 it pays 10x, by 25 4x, by 30 2x, and by 40 just your stake back. No line in forty balls and the card is dead."),
        ("PLINKO", "Drop a ball through twelve rows of pegs into one of thirteen slots. Low, medium and high risk change how sharply the prizes climb toward the edges — up to 220x on the high board — but all three return the same share of your stake over time."),
        ("MINES", "Choose how many mines hide under twenty-five tiles, then turn them over one at a time. Every safe tile raises your multiplier by the true odds of having got that far. Cash out whenever you like; turn over a mine and the round is gone."),
        ("CRASH", "A multiplier climbs away from 1.00x and stops dead at a point drawn before the round starts. Press C to take the money before it does. The chance of surviving to any multiplier is 0.99 divided by it, so 2x comes home just under half the time."),
        ("SCRATCH CARDS", "Buy a card and scratch nine panels. Three matching symbols pays that symbol's prize, from 1x up to 1,000x for three stars. The prize is decided when the card is printed, which is why the panels always tell the truth about what it is worth."),
        ("HORSE RACING", "Six runners at fixed odds from 2:1 to 20:1. Back one and watch the race. The odds come first and each runner's chance is derived from them, so every horse on the card returns the same share of your stake over time."),
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
