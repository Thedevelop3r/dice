//! The store: exchange in-game dollars for chips (and back), and buy items.
//! Every balance here is fictional in-game currency.

use crate::economy::{Item, Wallet, CHIPS_PER_DOLLAR, CHIPS_PER_DOLLAR_SELL};
use crate::stats::Store as Save;
use crate::ui::menu::{choose_from, MenuItem};
use crate::ui::{self, widgets, Screen};

pub fn open(save: &mut Save, screen: &mut Screen) {
    loop {
        let theme = screen.theme;
        let (chips, dollars) = {
            let w = Wallet::new(save);
            (w.chips(), w.dollars())
        };
        screen.begin();
        ui::header(screen, "THE STORE");
        screen.blank();
        screen.line(&format!("  balance: {}", ui::money(&theme, chips, dollars)));
        screen.line(&theme.dim(&format!("buy at {CHIPS_PER_DOLLAR} chips per $1 · cash out at {CHIPS_PER_DOLLAR_SELL} chips per $1")));

        let mut items = vec![
            MenuItem::new('1', "buy chips", "spend dollars for chips"),
            MenuItem::new('2', "cash out", "trade chips back for dollars"),
        ];
        let mut goods_keys = Vec::new();
        for (i, item) in Item::ALL.iter().enumerate() {
            let key = (b'3' + i as u8) as char;
            goods_keys.push(*item);
            let w = Wallet::new(save);
            let owned = if item.permanent() {
                if w.owns(*item) { " [OWNED]".to_string() } else { String::new() }
            } else {
                format!(" [x{}]", w.count(*item))
            };
            items.push(MenuItem::new(
                key,
                format!("{} — ${}", item.name(), item.price()),
                format!("{}{}", item.blurb(), owned),
            ));
        }
        items.push(MenuItem::new('b', "back", ""));

        match choose_from(screen, "STORE", &items) {
            Some('1') => buy_chips(save, screen),
            Some('2') => sell_chips(save, screen),
            Some('b') | None => return,
            Some(c) if c.is_ascii_digit() => {
                let idx = (c as u8).wrapping_sub(b'3') as usize;
                if let Some(item) = goods_keys.get(idx) {
                    buy_item(save, screen, *item);
                }
            }
            _ => {}
        }
    }
}

fn buy_chips(save: &mut Save, screen: &mut Screen) {
    let theme = screen.theme;
    let dollars = Wallet::new(save).dollars();
    if dollars <= 0 {
        message(screen, &theme.lose("no dollars to spend — cash some chips out first."));
        return;
    }
    let Some(n) = widgets::number_picker(screen, 1, dollars, dollars.min(5), 1, &[('m', dollars)], |s, v| {
        s.begin();
        ui::header(s, "BUY CHIPS");
        s.blank();
        s.line(&format!("  spend ${v} → {} chips", v * CHIPS_PER_DOLLAR));
        s.blank();
        s.line(&widgets::footer(&s.theme, &[('↑', "more"), ('↓', "less"), ('m', "max"), ('\u{23ce}', "confirm")]));
    }) else {
        return;
    };
    let mut w = Wallet::new(save);
    if w.spend_dollars(n) {
        let chips = n * CHIPS_PER_DOLLAR;
        w.add_chips(chips);
        save.bump("store.chips_bought", chips);
        message(screen, &screen.theme.win(&format!("+{chips} chips for ${n}")));
    }
    let _ = save.save();
}

fn sell_chips(save: &mut Save, screen: &mut Screen) {
    let theme = screen.theme;
    let chips = Wallet::new(save).chips();
    if chips < CHIPS_PER_DOLLAR_SELL {
        message(screen, &theme.lose(&format!("need at least {CHIPS_PER_DOLLAR_SELL} chips to cash out.")));
        return;
    }
    let max = chips / CHIPS_PER_DOLLAR_SELL;
    let Some(n) = widgets::number_picker(screen, 1, max, 1, 1, &[('m', max)], |s, v| {
        s.begin();
        ui::header(s, "CASH OUT");
        s.blank();
        s.line(&format!("  trade {} chips → ${v}", v * CHIPS_PER_DOLLAR_SELL));
        s.blank();
        s.line(&widgets::footer(&s.theme, &[('↑', "more"), ('↓', "less"), ('m', "max"), ('\u{23ce}', "confirm")]));
    }) else {
        return;
    };
    let cost = n * CHIPS_PER_DOLLAR_SELL;
    let mut w = Wallet::new(save);
    if w.spend_chips(cost) {
        w.add_dollars(n);
        save.bump("store.chips_sold", cost);
        message(screen, &screen.theme.win(&format!("-{cost} chips for ${n}")));
    }
    let _ = save.save();
}

fn buy_item(save: &mut Save, screen: &mut Screen, item: Item) {
    let theme = screen.theme;
    let mut w = Wallet::new(save);
    if item.permanent() && w.owns(item) {
        message(screen, &theme.dim("you already own that."));
        return;
    }
    if !w.spend_dollars(item.price()) {
        message(screen, &theme.lose(&format!("that costs ${} — you have ${}.", item.price(), w.dollars())));
        return;
    }
    w.grant(item, 1);
    save.bump("store.purchases", 1);
    let _ = save.save();
    message(screen, &screen.theme.win(&format!("bought {}", item.name())));
}

fn message(screen: &mut Screen, text: &str) {
    screen.blank();
    screen.line(&format!("  {text}"));
    ui::pause(screen);
}
