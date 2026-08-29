//! The store: exchange in-game dollars for chips (and back), and buy items.
//! Every balance here is fictional in-game currency.

use crate::economy::{Item, Wallet, CHIPS_PER_DOLLAR, CHIPS_PER_DOLLAR_SELL};
use crate::stats::Store as Save;
use crate::ui::{self, *};

pub fn open(save: &mut Save, colors: bool) {
    loop {
        ui::clear();
        ui::header(colors, "THE STORE");
        {
            let w = Wallet::new(save);
            println!("  balance: {}", ui::money(colors, w.chips(), w.dollars()));
        }
        println!(
            "  {}",
            color(colors, DIM, &format!("buy at {CHIPS_PER_DOLLAR} chips per $1 · cash out at {CHIPS_PER_DOLLAR_SELL} chips per $1"))
        );

        println!("\n  {}", color(colors, BOLD, "exchange"));
        println!("   1. buy chips with dollars");
        println!("   2. cash chips out for dollars");
        println!("\n  {}", color(colors, BOLD, "goods"));
        for (i, item) in Item::ALL.iter().enumerate() {
            let w = Wallet::new(save);
            let owned = if item.permanent() {
                if w.owns(*item) { " [OWNED]".to_string() } else { String::new() }
            } else {
                format!(" [x{}]", w.count(*item))
            };
            println!(
                "   {}. {:<16} {}   {}{}",
                i + 3,
                item.name(),
                color(colors, GREEN, &format!("{:>4}", format!("${}", item.price()))),
                color(colors, DIM, item.blurb()),
                color(colors, CYAN, &owned)
            );
        }
        println!("\n   b. back");

        let choice = ui::prompt("\n  choose: ").to_lowercase();
        match choice.as_str() {
            "1" => buy_chips(save, colors),
            "2" => sell_chips(save, colors),
            "b" | "q" | "" => return,
            other => match other.parse::<usize>() {
                Ok(n) if (3..3 + Item::ALL.len()).contains(&n) => buy_item(save, colors, Item::ALL[n - 3]),
                _ => {}
            },
        }
    }
}

fn buy_chips(save: &mut Save, colors: bool) {
    let dollars = Wallet::new(save).dollars();
    if dollars <= 0 {
        println!("  {}", color(colors, RED, "no dollars to spend — cash some chips out first."));
        ui::pause();
        return;
    }
    let n = ui::prompt_usize(&format!("  spend how many dollars (1-{dollars})"), 1, dollars as usize, 5.min(dollars as usize)) as i64;
    let mut w = Wallet::new(save);
    if w.spend_dollars(n) {
        let chips = n * CHIPS_PER_DOLLAR;
        w.add_chips(chips);
        println!("  {}", color(colors, GREEN, &format!("+{chips} chips for ${n}")));
        save.bump("store.chips_bought", chips);
    }
    let _ = save.save();
    ui::pause();
}

fn sell_chips(save: &mut Save, colors: bool) {
    let chips = Wallet::new(save).chips();
    if chips < CHIPS_PER_DOLLAR_SELL {
        println!(
            "  {}",
            color(colors, RED, &format!("need at least {CHIPS_PER_DOLLAR_SELL} chips to cash out."))
        );
        ui::pause();
        return;
    }
    let max = chips / CHIPS_PER_DOLLAR_SELL;
    let n = ui::prompt_usize(&format!("  cash out for how many dollars (1-{max})"), 1, max as usize, 1) as i64;
    let cost = n * CHIPS_PER_DOLLAR_SELL;
    let mut w = Wallet::new(save);
    if w.spend_chips(cost) {
        w.add_dollars(n);
        println!("  {}", color(colors, GREEN, &format!("-{cost} chips for ${n}")));
        save.bump("store.chips_sold", cost);
    }
    let _ = save.save();
    ui::pause();
}

fn buy_item(save: &mut Save, colors: bool, item: Item) {
    let mut w = Wallet::new(save);
    if item.permanent() && w.owns(item) {
        println!("  {}", color(colors, DIM, "you already own that."));
        ui::pause();
        return;
    }
    if !w.spend_dollars(item.price()) {
        println!("  {}", color(colors, RED, &format!("that costs ${} — you have ${}.", item.price(), w.dollars())));
        ui::pause();
        return;
    }
    w.grant(item, 1);
    println!("  {}", color(colors, GREEN, &format!("bought {}", item.name())));
    save.bump("store.purchases", 1);
    let _ = save.save();
    ui::pause();
}
