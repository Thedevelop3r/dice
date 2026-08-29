# Dice Arena

A casino console in Rust — **zero external crates**, including its own PRNG
and its own raw-terminal input (direct `termios(3)` FFI, no `crossterm`).

```
cargo run --release
```

## Controls

Every screen in the app is driven by **single keypresses** — there is no
typing a letter and pressing Enter. The terminal is put into raw,
no-echo mode on launch, so the instant you press a key it acts:

- Menus and in-game choices: press the bracketed key shown, e.g. `[R]oll`,
  `[1] Pig`, `[Y]es` — the action fires immediately.
- Numeric pickers (a target score, a chip stake): `↑/→` and `↓/←` step the
  value, digits type an exact number, and any preset keys shown (like
  `[M]` for all-in) jump straight there. `Enter` confirms, `Esc` cancels —
  these are the only two places Enter is used, since there's no way to
  know a typed number is "done" without a terminator.
- Anywhere a name is typed (yours, a second human player's), type and
  press `Enter` to confirm.
- `Ctrl+C` leaves the table/menu you're in immediately and cleanly closes
  the app, saving as it goes.
- Ultra Casino Dice is the one exception: it's a hands-off spectacle table
  that runs itself. The only key that does anything is `Q`, which lets the
  round in progress finish and then returns you to the floor.

The whole app runs in the terminal's alternate screen buffer, so nothing
scrolls into your shell's history — every screen is a hard clear-and-redraw,
and your prompt looks exactly as it did before you ran it once you quit.

## Games

| | |
|---|---|
| **Pig** | Press-your-luck race to a target score. 2–6 players, any mix of humans and bots, classic (1d6) or two-dice variant (snake eyes wipes your score, doubles pay double). |
| **Yahtzee** | Full 13-category scorecard, three rolls a turn with dice holding (press 1-5 to toggle a hold, `R` to reroll, `S` to stand), upper-section 63/35 bonus, 100-point bonus Yahtzees. 1–4 players, hot-seat or vs. AI. |
| **Chuck-a-Luck** | Three-dice wagering with a bankroll that persists between sessions. Bet a face (1:1/2:1/3:1), HIGH/LOW at evens, or any triple at 30:1. |
| **Luck Bet** | Eight dice named **A–H**. Up to three players (you + 2 CPUs) each back a letter and the face they think it lands on — 6 by default. A hit pays 5:1 (6:1 with a VIP pass); a miss loses the stake and rakes a fifth of it into a carried-over jackpot. Hit your number while three or more dice show it and you sweep the jackpot too. |
| **Luck Bet: Turbo** | The identical game, hands-free. One key spins the table at 20 frames a second for three seconds before it settles; bets repeat automatically. |
| **Roulette** | Single-zero wheel. Straight-up numbers pay 35:1; red/black, odd/even, high/low pay evens; the three dozens pay 2:1. The wheel spins through the same decelerating animation as every dice reveal. |
| **Blackjack** | Heads-up against the dealer with a real 52-card shoe. Hit, stand, or double down; dealer stands on all 17s; blackjack pays 3:2. |
| **Tournament** | Chip buy-in, single-elimination Pig bracket (4 or 8 entrants, three tiers). Your matches are played out, the rest of the bracket is simulated. Champion takes 70% of the pool, runner-up 30%. |
| **Store** | Exchange in-game dollars for chips (10 per $1) and back (12 chips per $1), and buy reroll tokens, insurance chits, lucky charms, a VIP pass or a gold dice skin. |
| **Dice Lab** | Roll any `NdM+K` (`3d6`, `d20`, `4d10+2`), plot a distribution with `sim 2d6 50000`, and pin the RNG with `seed 42`. The one screen that still takes typed text — there's no sensible key for an arbitrary expression. |
| **Ultra Casino Dice** | An unattended spectacle table — 12 dice, 8 computer players, 10 rounds, zero input required. Every round each seat backs a letter and calls a face, the table spins for a full 7 seconds, and whoever called it right splits the pot — losers' stakes are exactly what winners collect, with one exception: a round nobody calls right pays its whole pot to the house. Seats decide to stay or cash out between rounds, and anyone who leaves is replaced by a fresh AI player with a new random name and bankroll. A finished session shows its final standings, then a new one starts on its own — press `Q` to let the current round finish and return to the floor instead. Every round and every session lands in the **History** screen. |
| **Casino** | Not a table — the house's own books. A running balance of the house's total profit and loss across every wagering game, with a per-table breakdown, updated live as bets settle. |

## The roll

Every dice reveal in the app — Pig, Yahtzee, Chuck-a-Luck, Luck Bet, the
Dice Lab, Roulette's wheel — plays the same house animation: ten frames
of tumbling values over roughly a second, easing from a quick flicker to
a settle, before landing on the real, already-determined result. Ultra
Casino Dice's table spins to its own, longer rhythm — 20 frames a second
for a full 7 seconds — matching the scale of a 12-die, 8-player table.

## Economy

Chips are the table currency, dollars are the in-game cash — **all of it
fictional**, with no real money involved anywhere. One shared wallet spans
every wagering game and persists between sessions. Go broke and the house
stakes you again, so the game is never a dead end.

### The house

The casino keeps a wallet too. Every time a wagering game settles — Chuck-a-Luck,
Luck Bet, Roulette, Blackjack, Tournament, and unclaimed Ultra Casino Dice pots
— the house's balance moves by exactly the opposite of whatever the player's
wallet just did, so the two ledgers are always mirror images of each other.
Store currency exchanges and item purchases don't touch it: those move chips
around, but they aren't a game's outcome. The **Casino** screen (`[C]` on the
dashboard) shows the house's running balance, lifetime collected/paid totals,
and a per-table breakdown; the same per-table numbers also show up as a
`house_pl` line under each game's own section on the **Stats** screen.

## Architecture

The UI is a separate layer (`src/ui/`) from every game's rules — a new
game only ever needs a `Ctx` (RNG + save file + `Screen`) and the widgets
in `src/ui/widgets.rs`, `menu.rs`, `dice_art.rs` and `card_art.rs`. That's
what let Roulette and Blackjack land as new files that touch nothing in
the games that came before them, and it's the seam for whatever's added
next — new tables, new animations, a new color theme in `src/ui/theme.rs`.

- `src/ui/input.rs` — raw single-keypress terminal input (direct termios
  FFI on Unix; a line-buffered fallback elsewhere).
- `src/ui/mod.rs` — the `Screen`: alternate-screen session, buffered
  clear-and-redraw frames, headers, panels.
- `src/ui/theme.rs` — the casino color palette.
- `src/ui/dice_art.rs` / `card_art.rs` — pip-face and playing-card
  rendering, and the roll animation.
- `src/ui/menu.rs` / `widgets.rs` — the boxed keyed menu, the numeric
  stepper, text input, progress bars, banners.
- `src/games/*.rs` — pure game logic, one file per game, each drawing
  only through the `Ctx`/`Screen` it's handed.
- `src/economy.rs` — `Wallet`, the player's chips/dollars/inventory, and
  `House`, the casino's own mirror-image ledger, recorded explicitly at
  each game's settlement point rather than hooked generically into
  `Wallet` (which is also used for Store exchanges that shouldn't count
  as house winnings).
- `src/history.rs` — a plain-text, append-only log of Ultra Casino Dice
  sessions, kept separate from `stats::Store`'s `key=value` settings file
  since it only ever grows. Read back by the dashboard's History screen.

## Features

- ASCII pip faces with held-dice highlighting, colored output (toggleable
  in Options)
- Three AI difficulties — Easy is erratic, Normal holds at 20, Hard uses
  the near-optimal "hold at 25 minus banked" rule and pushes when behind
- Persistent stats, high scores and settings in
  `$XDG_DATA_HOME/dice_arena/save.conf`, plus a plain-text Ultra Casino
  Dice history log at `$XDG_DATA_HOME/dice_arena/ultra_history.log`
- Reproducible sessions: `DICE_SEED=42 cargo run`
- Persistent inventory: consumables (reroll, insurance, charm) and
  permanent upgrades (VIP payouts, gold dice)
- A live house ledger (`Casino` screen) tracking the casino's own
  profit and loss, mirror-image to the player's wallet, across every
  wagering table
- Handles EOF and Ctrl+C cleanly, so the whole app is scriptable

## Tests

```
cargo test
```

Covers dice-notation parsing, roll uniformity, seeded reproducibility,
every Yahtzee scoring category, Luck Bet hit/sweep resolution, Roulette's
payout table, Blackjack's hand totals (soft aces, busts, the shoe),
Ultra Casino Dice's pot math (proportional splits between tied winners,
zero-sum payouts, stake bounds), and the house ledger's zero-sum
invariant (its balance always equals lifetime collected minus paid,
across settlements and games).
