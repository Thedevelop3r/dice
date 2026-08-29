//! dice_arena — a console dice suite: Pig, Yahtzee, Chuck-a-Luck and a dice lab.

mod dice;
mod economy;
mod games;
mod rng;
mod shop;
mod stats;
mod ui;

use games::{chuck, lab, luckbet, pig, tournament, yahtzee, Ctx, Difficulty, Player};
use rng::Rng;
use stats::Store;
use ui::*;

fn main() {
    let mut store = Store::load();
    let mut rng = match std::env::var("DICE_SEED").ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(seed) => Rng::from_seed(seed),
        None => Rng::new(),
    };

    if store.get_str("player.name", "").is_empty() {
        let name = ui::prompt("  what should we call you? ");
        store.set_str("player.name", if name.is_empty() || name == "q" { "Player" } else { &name });
        let _ = store.save();
    }

    loop {
        let colors = store.get_i64("cfg.colors", 1) == 1;
        ui::clear();
        ui::banner(colors);
        let name = store.get_str("player.name", "Player");
        println!("  welcome back, {}\n", color(colors, BOLD, &name));
        println!("   1. {}  {}", color(colors, CYAN, "Pig"), color(colors, DIM, "press your luck to the target score"));
        println!("   2. {}  {}", color(colors, CYAN, "Yahtzee"), color(colors, DIM, "13-category scorecard classic"));
        println!("   3. {}  {}", color(colors, CYAN, "Luck Bet"), color(colors, DIM, "8 dice A-H, back a letter and a face"));
        println!("   4. {}  {}", color(colors, CYAN, "Luck Bet: Turbo"), color(colors, DIM, "hands-free — Enter spins the table"));
        println!("   5. {}  {}", color(colors, CYAN, "Tournament"), color(colors, DIM, "knockout bracket for a chip prize pool"));
        println!("   6. {}  {}", color(colors, CYAN, "Chuck-a-Luck"), color(colors, DIM, "three-dice wagering"));
        println!("   7. {}  {}", color(colors, CYAN, "Dice Lab"), color(colors, DIM, "roll any NdM+K, sample distributions"));
        println!("   8. {}  {}", color(colors, BLUE, "Store"), color(colors, DIM, "chips, dollars and lucky charms"));
        println!("   9. {}", color(colors, BLUE, "Statistics"));
        println!("   s. {}", color(colors, BLUE, "Settings"));
        println!("   r. {}", color(colors, BLUE, "Rules"));
        println!("   q. quit");

        let (chips, dollars) = {
            let w = economy::Wallet::new(&mut store);
            (w.chips(), w.dollars())
        };
        println!("\n  wallet: {}", ui::money(colors, chips, dollars));

        let choice = ui::prompt("\n  choose: ");
        if choice.eq_ignore_ascii_case("8") {
            shop::open(&mut store, colors);
            continue;
        }
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, colors };
        match choice.to_lowercase().as_str() {
            "1" => {
                let players = setup_players(&mut ctx, &name, 2, 6);
                if players.len() < 2 {
                    continue;
                }
                let target = ui::prompt_usize("  target score", 20, 1000, 100) as i64;
                let two_dice = ui::confirm("  two-dice variant?");
                pig::play(&mut ctx, players, pig::Config { target, two_dice });
            }
            "2" => {
                let players = setup_players(&mut ctx, &name, 1, 4);
                if players.is_empty() {
                    continue;
                }
                yahtzee::play(&mut ctx, players);
            }
            "3" | "4" => {
                let turbo = choice == "4";
                let opponents = if turbo {
                    2
                } else {
                    ui::prompt_usize("  how many CPU opponents (0-2)", 0, 2, 2)
                };
                luckbet::play(&mut ctx, opponents, turbo);
            }
            "5" => tournament::play(&mut ctx),
            "6" => chuck::play(&mut ctx),
            "7" => lab::play(&mut ctx),
            "9" => show_stats(&mut ctx),
            "s" | "settings" => settings(&mut ctx),
            "r" | "rules" => rules(colors),
            "q" | "quit" | "exit" => {
                let _ = store.save();
                println!("\n  {}\n", color(colors, DIM, "thanks for playing."));
                return;
            }
            _ => {}
        }
    }
}

/// Builds the roster: the human player plus any humans/bots they add.
fn setup_players(ctx: &mut Ctx, name: &str, min: usize, max: usize) -> Vec<Player> {
    ui::header(ctx.colors, "PLAYERS");
    let total = ui::prompt_usize("  how many players", min, max, min.max(2));
    let mut players = vec![Player::human(name)];
    for i in 2..=total {
        let is_bot = ui::confirm(&format!("  is player {i} a computer?"));
        if is_bot {
            println!("   1. Easy   2. Normal   3. Hard");
            let d = Difficulty::from_index(ui::prompt_usize("  difficulty", 1, 3, 2));
            players.push(Player::bot(&format!("CPU-{}", i - 1), d));
        } else {
            let n = ui::prompt(&format!("  name for player {i}: "));
            let n = if n.is_empty() { format!("Player {i}") } else { n };
            players.push(Player::human(&n));
        }
    }
    players
}

fn show_stats(ctx: &mut Ctx) {
    ui::clear();
    ui::header(ctx.colors, "STATISTICS");
    let rows = [
        ("Wallet", "econ"),
        ("Pig", "pig"),
        ("Yahtzee", "yahtzee"),
        ("Luck Bet", "luckbet"),
        ("Tournament", "tourney"),
        ("Chuck-a-Luck", "chuck"),
        ("Dice Lab", "lab"),
        ("Store", "store"),
        ("Inventory", "inv"),
    ];
    let mut any = false;
    for (label, key) in rows {
        let entries: Vec<(String, String)> = ctx
            .store
            .entries()
            .filter(|(k, _)| k.starts_with(&format!("{key}.")) && *k != "econ.init")
            .map(|(k, v)| (k.split_once('.').unwrap().1.to_string(), v.clone()))
            .collect();
        if entries.is_empty() {
            continue;
        }
        any = true;
        println!("\n  {}", color(ctx.colors, BOLD, label));
        for (k, v) in entries {
            println!("   {:<14} {}", k, color(ctx.colors, CYAN, &v));
        }
    }
    if !any {
        println!("\n  {}", color(ctx.colors, DIM, "no games played yet."));
    }
    println!("\n  {}", color(ctx.colors, DIM, &format!("saved at {}", ctx.store.path_display())));
    ui::pause();
}

fn settings(ctx: &mut Ctx) {
    loop {
        ui::clear();
        ui::header(ctx.colors, "SETTINGS");
        let colors_on = ctx.store.get_i64("cfg.colors", 1) == 1;
        println!("   1. name    : {}", ctx.store.get_str("player.name", "Player"));
        println!("   2. colors  : {}", if colors_on { "on" } else { "off" });
        println!("   3. reset statistics");
        println!("   b. back");
        match ui::prompt("\n  choose: ").to_lowercase().as_str() {
            "1" => {
                let n = ui::prompt("  new name: ");
                if !n.is_empty() {
                    ctx.store.set_str("player.name", &n);
                }
            }
            "2" => ctx.store.set_i64("cfg.colors", if colors_on { 0 } else { 1 }),
            "3" => {
                if ui::confirm("  wipe all stats and bankrolls?") {
                    ctx.store.reset();
                    println!("  {}", color(ctx.colors, GREEN, "stats cleared."));
                    ui::pause();
                }
            }
            _ => {
                let _ = ctx.store.save();
                return;
            }
        }
        let _ = ctx.store.save();
    }
}

fn rules(colors: bool) {
    ui::clear();
    ui::header(colors, "RULES");
    println!(
        "\n  {}\n   Roll to build a turn total, hold to bank it. Roll a 1 and the turn\n   total is gone. In the two-dice variant a single 1 ends the turn, snake\n   eyes wipes your whole score, and doubles pay double. First to the\n   target score wins.\n",
        color(colors, BOLD, "PIG")
    );
    println!(
        "  {}\n   Thirteen rounds, three rolls each: keep dice between rolls, then\n   commit the hand to one open category. 63+ in the upper section earns\n   a 35-point bonus; each extra Yahtzee after the first is worth 100.\n",
        color(colors, BOLD, "YAHTZEE")
    );
    println!(
        "  {}\n   Stake chips on a face (pays 1:1, 2:1 or 3:1 by how many of the three\n   dice show it), on HIGH (11-17) or LOW (4-10) at even money — both lose\n   to any triple — or on any triple at 30:1.\n",
        color(colors, BOLD, "CHUCK-A-LUCK")
    );
    println!(
        "  {}\n   Eight dice sit on the table, named A to H. Each player backs one\n   letter and the face they think it will show (6 by default), then the\n   table is rolled. A hit pays 5:1 from the house (6:1 with a VIP pass);\n   a miss loses the stake and rakes a fifth of it into the jackpot. Hit\n   your number while three or more dice show it and you sweep the\n   jackpot too. Up to three players — the other two can be CPUs.\n   TURBO plays the identical game hands-free: press Enter and the table\n   spins at 20 frames a second for three seconds before it settles.\n",
        color(colors, BOLD, "LUCK BET")
    );
    println!(
        "  {}\n   Pay a chip buy-in, get drawn into a single-elimination Pig bracket.\n   Your matches are played out; the rest of the bracket is simulated.\n   The champion takes 70% of the pool and the runner-up 30%.\n",
        color(colors, BOLD, "TOURNAMENT")
    );
    println!(
        "  {}\n   Chips are the table currency and dollars are the in-game cash — all\n   of it fictional. Buy chips at 10 per $1, cash out at 12 chips per $1,\n   and spend dollars on charms, insurance, reroll tokens, a VIP pass\n   (6:1 Luck Bet payouts) or gold dice.\n",
        color(colors, BOLD, "THE STORE")
    );
    println!(
        "  {}\n   Type dice notation like 4d6+2 to roll, `sim 2d6 50000` to plot the\n   distribution, or `seed 42` for reproducible rolls.\n",
        color(colors, BOLD, "DICE LAB")
    );
    println!("  {}", color(colors, DIM, "tip: set DICE_SEED=<n> in the environment to seed the whole session."));
    ui::pause();
}
