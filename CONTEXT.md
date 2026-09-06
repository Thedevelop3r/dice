# Dice Arena — session context

Working notes for picking this project back up. Read this first; it covers
what the project is, what has been built, the conventions that hold it
together, and what is worth doing next.

**Repo:** `~/My-drive/Github-repos/dice`
**Run:** `cargo run --release` · **Test:** `cargo test` · **Lint:** `cargo clippy`
**Seeded session:** `DICE_SEED=42 cargo run --release`
**Save file:** `$XDG_DATA_HOME/dice_arena/save.conf` (plus `ultra_history.log` beside it)

---

## What it is

A casino console that runs entirely in the terminal. Twenty-four tables
across four rooms, every outcome animated, every table able to run itself
with nobody watching.

The two constraints that define the whole codebase:

1. **Zero external crates.** Its own PRNG (xoshiro256\*\*), its own raw
   terminal input via direct `termios(3)` FFI. `Cargo.toml` has no
   dependencies and must keep having none.
2. **Single keypresses.** No typing-then-Enter anywhere except numeric
   pickers (where Enter confirms) and the two free-text fields (names, the
   Dice Lab expression).

---

## Current state

| | |
|---|---|
| Source | 11,748 lines across 41 files |
| Tests | 167, all passing |
| Clippy | clean, zero warnings |
| Dependencies | none |
| Tables | 24 (8 dice, 6 card, 2 wheel, 8 arcade) |
| Idle screens | 17 tables + a floor-wide cycler |

---

## Session history

**Sessions 1–2 — the base game.** The UI layer (`src/ui/`), the economy,
the store, persistent stats, and the first nine tables: Pig, Yahtzee, Luck
Bet (+ Turbo), Chuck-a-Luck, Roulette, Blackjack, Tournament, Dice Lab, and
Ultra Casino Dice with its own history log.

**Session 3 — the house wallet.** Added `economy::House`, the casino's own
ledger: a mirror image of the player's wallet, updated at the settlement
point of every wagering game. Added the `[C] Casino` screen. Deliberately
excluded Store exchanges and item purchases, since those aren't a game's
outcome.

**Session 4 (this one) — the big expansion.** Five threads:

- **Optimised the render path.** Added `Theme::paint_into` so animation
  inner loops don't allocate a `String` per painted segment; removed the
  intermediate per-die `String` arrays; reused one scratch buffer across
  each roll animation instead of allocating per frame. Fixed a real bug: a
  `10` card rendered one column wider than every other card, shearing hands
  out of alignment.
- **3× dice and cards.** `ui::Scale` with three steps; `Big` is exactly
  three times `Small` on both axes (27×15 a die, 21×15 a card). The
  renderers measure the terminal and take the biggest that fits, so no call
  site picks a size.
- **Extracted two shared modules.** `games/cards.rs` (one deck, shoe and
  both hand rankings — Blackjack's private deck is gone) and
  `games/table.rs` (the four beats every wagering table shares).
- **Fourteen new tables**, listed below.
- **Idle modes** for seventeen tables plus `games/floor.rs`, the cycler
  that walks them all.

Also restructured the dashboard into rooms — 24 tables did not fit on one
readable menu — and paginated the Rules screen for the same reason.

### The fourteen new tables

| Table | Key | The interesting bit |
|---|---|---|
| Baccarat | `baccarat` | Real third-card rules; banker pays 1:1 less 5% |
| Video Poker | `vidpoker` | Jacks or better; pair rank read ace-high |
| Three Card Poker | `threecard` | Straight beats flush at three cards; ante bonus pays win or lose |
| Casino War | `war` | Surrender vs. war is a genuine decision, both worse than even |
| Hi-Lo | `hilo` | Every call priced off true odds, derived not tabled |
| Big Six | `bigsix` | 54-segment money wheel, scroll worked backwards from the drawn stop |
| Slots | `slots` | Real weighted reel strips — odds live in `strip()`, not a constant |
| Keno | `keno` | Standard paytable; quick-pick with re-pick before staking |
| Bingo | `bingo` | Paid on *how fast* the first line lands (flat-rate bingo is unpriceable) |
| Plinko | `plinko` | Multipliers in hundredths; three risk profiles, same RTP |
| Mines | `mines` | Multiplier is the actual reciprocal of the survival odds |
| Crash | `crash` | Live cash-out mid-animation; survival curve is `0.99 / x` |
| Scratch Cards | `scratch` | Prize decided at press, panels laid out to show it |
| Horse Racing | `horses` | Odds first, chances derived from them, so every runner returns the same share |

---

## File map

```
src/
  main.rs          609  dashboard, the four rooms, idle room, Stats, Casino, Rules, Options
  economy.rs       265  Wallet (player) + House (casino ledger) + Item
  stats.rs         113  the key=value save file
  rng.rs            58  xoshiro256** + SplitMix64 seeding
  dice.rs          101  NdM+K notation parsing
  shop.rs          135  the Store
  history.rs        68  Ultra Casino Dice's append-only session log
  ui/
    mod.rs         249  Screen, Scale, fit_scale, term_size, header/pause/money
    input.rs       412  raw termios FFI, read_key, poll_leave_signal, poll_action
    theme.rs       119  the palette + paint/paint_into
    dice_art.rs    447  pip faces at three sizes, roll animations
    card_art.rs    281  cards at three sizes, block-art suits
    menu.rs         80  the boxed keyed menu
    widgets.rs     143  number_picker, text_input, footer, banner, bar
  games/
    mod.rs         109  Ctx, Difficulty, Player, pick_difficulty
    cards.rs       443  shared deck/shoe/hand rankings  ← every card table
    table.rs       244  shared wagering furniture       ← every wagering table
    floor.rs        88  the attract-mode registry + cycler
    <24 game files>
```

---

## How to add a table

One new file in `src/games/`, one `pub mod` line, one menu entry, one row
in `main.rs`'s `TABLES`, one Rules entry. That is the whole surface.

```rust
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "yourgame";     // stats prefix AND house ledger key
const TITLE: &str = "YOUR GAME";

pub fn play(ctx: &mut Ctx) {
    loop {
        let (bank, restaked) = table::open_bank(ctx);          // restakes a broke player
        let Some(stake) = table::stake(ctx, TITLE, "…", bank, 10) else { break };
        if !Wallet::new(ctx.store).spend_chips(stake) { continue }

        // …animate, decide the outcome…

        let delta = table::settle(ctx, KEY, stake, payout);     // pays, books the house, saves
        table::verdict(ctx, delta);
        if !table::again(ctx, "another round") { break }
    }
    table::cash_out(ctx, TITLE);
}

pub fn idle(ctx: &mut Ctx) { /* … */ }
```

`table::settle(ctx, key, staked, gross_payout)` does everything: pays the
player, books the exact mirror image to `House`, records
wins/losses/pushes/best_win, and saves. **`payout` is gross** — `0` is a
total loss, `stake` is a push. Getting this wrong is the easiest way to
silently break the ledger.

Then register the idle function in `floor::ATTRACT` and it joins the cycler.

---

## House style — the non-negotiables

- **Everything that settles animates first.** A result that simply appears
  is wrong. Ease it: fast at first, slowing to a stop.
- **Where an outcome is drawn before the animation** (the wheel's segment,
  the race winner), the animation is worked *backwards* from it so what you
  watch is genuinely what you get. Never fake the reverse.
- **Every frame is a full redraw.** `screen.begin()` … `screen.present()`.
- **Never touch the terminal directly.** No `println!`, no escape codes
  outside `src/ui/`.
- **Never hard-code a die or card size.** Iterate what the block functions
  return.
- **Comments explain *why*.** Trade-offs, rules-of-the-game, the reason a
  constant is that number. Never `// increment the counter`.
- **Prose is plain lowercase.** `"you win 40 chips"`, `"the house takes
  it"`. Headers are the only uppercase. No emoji beyond the suit and pip
  glyphs already in use.
- **Payouts are priced, not guessed.** Every new table's RTP is pinned by a
  test against the odds it claims. Follow that — it is the single most
  valuable convention in the codebase.

---

## Invariants and gotchas

- **Integer money throughout.** No floats in payouts. Multipliers that need
  fractions are held in hundredths (`170` = 1.7×) and applied as
  `stake * mult / 100`. Floats appear only inside tests.
- **The house ledger is zero-sum by construction.** `house.balance` always
  equals `house.collected - house.paid`. Exactly one `House::record` per
  settled bet.
- **Idle modes touch nothing persistent.** No `Wallet`, no `House`, no
  `bump`. Every chip on an idle screen is fictional twice over. Keep it
  that way — it is why the Casino books can be trusted.
- **Idle loops must hold through `table::idle_hold`**, never `sleep_ms`.
  That is what makes `Q` responsive *and* what lets the floor cycler
  time-slice a table that has no idea it is being cycled (via a
  thread-local deadline in `table.rs`).
- **Ultra Casino Dice is excluded from the cycler** on purpose: it polls
  `poll_leave_signal` directly rather than `idle_hold`, so it would ignore
  the budget and never hand the floor back.
- **`Store::blank()` and `Screen::headless()` are `#[cfg(test)]` only.**
  They exist so tests can build a whole `Ctx` without touching the real
  save file or stdout.
- **Menu key collisions.** The idle room hands tables `1`–`9` then `a`–`h`,
  keeping `0`, `u` and `z` outside that range for the cycler, Ultra and
  back. Adding an eighteenth attract table needs that scheme revisited.
- **Two screens now paginate** (History, Rules). Anything else that grows
  past a screen needs the same treatment.

---

## Verifying changes

`cargo test` covers the maths. It cannot cover the thing this app mostly
is — what appears on screen. For that, drive a real pty with tmux:

```bash
tmux kill-session -t v 2>/dev/null
tmux new-session -d -s v -x 200 -y 50 \
  "cd ~/My-drive/Github-repos/dice && DICE_SEED=5 ./target/release/dice_arena; sleep 30"
sleep 2
tmux send-keys -t v "4"          # a keypress
sleep 1
tmux capture-pane -pt v | head -30
```

Notes that will save time:

- A 200×50 pane is wide enough for the 3× faces. Narrower windows exercise
  the fall-back sizes, which is worth testing deliberately.
- `table::cash_out` ends on `ui::pause`, so leaving a table eats one extra
  keypress. Budget for it when scripting a path.
- `capture-pane` occasionally returns blank mid-redraw. Sleep and retry
  rather than concluding something broke.
- Always verify at least one full round of any table you touch, and check
  the Casino screen afterwards — it is the fastest way to catch a payout
  wired up backwards.

---

## Stats keys by table

`table::settle` adds `wins` / `losses` / `pushes` / `best_win` and
`house_pl` automatically, so the newer tables list only their extras.

| Table | Key | Extra keys |
|---|---|---|
| Pig | `pig` | busts, rolls, turns |
| Yahtzee | `yahtzee` | games, rolls, wins, best |
| Luck Bet | `luckbet` | losses, rolls, rounds, sweeps, wins, best_jackpot, best_win |
| Chuck-a-Luck | `chuck` | losses, rolls, wins |
| Tournament | `tourney` | entries, titles, best_prize |
| Dice Lab | `lab` | rolls, simulated_rolls, best_total |
| Ultra Casino Dice | `ultra` | hits, rounds, sessions |
| Blackjack | `blackjack` | hands, wins, losses, pushes |
| Baccarat | `baccarat` | hands |
| Video Poker | `vidpoker` | hands |
| Three Card Poker | `threecard` | hands, bonuses |
| Casino War | `war` | hands, wars |
| Hi-Lo | `hilo` | rounds, calls |
| Roulette | `roulette` | spins, wins, losses |
| Big Six | `bigsix` | spins |
| Slots | `slots` | spins |
| Keno | `keno` | games, hits |
| Bingo | `bingo` | cards, lines, fastest_line |
| Plinko | `plinko` | drops |
| Mines | `mines` | rounds, best_run |
| Crash | `crash` | rounds, best_mult |
| Scratch Cards | `scratch` | cards |
| Horse Racing | `horses` | races |

---

## Known gaps

1. **Four tables have no idle mode** — Pig, Yahtzee, Tournament, Dice Lab.
   They are the multiplayer and utility screens, so the attract pattern
   (a fictional punter placing bets) doesn't map cleanly. Bot-vs-bot would
   work for Pig and Tournament; Yahtzee and the Lab need a different idea.
2. **Plinko's slot labels are cramped.** `3.5x1.7x1.0x` runs together
   because the cells are exactly as wide as the labels. Alignment with the
   pegs is correct; only the spacing reads badly.
3. **The save file has no version field.** Any future change to the key
   layout will silently misread old saves.
4. **RTP claims are point estimates in tests.** Correct, but a change to a
   paytable could drift the real return without failing anything if the
   test's tolerance is loose.

---

## Next steps

Roughly in the order I would take them.

### 1. A headless RTP harness (highest value)

This app is now fourteen paytables deep, and its central promise is that
every one of them is honestly priced. Right now each table proves that its
own way, in its own test. A single harness — run N rounds of every table
against a seeded RNG with no rendering, report the realised return — would
turn that into one table of numbers you can eye in a second, and catch a
paytable edit that quietly moves a game to 1.4× return.

It needs each game's settlement maths callable without its UI. Most are
already pure functions (`payout_mult`, `payout`, `multiplier`); the work is
a common trait or a small registry over them.

### 2. Close the four idle gaps

Pig and Tournament are cheap — `pig::simulate` already runs a headless
match, so a bot-vs-bot display loop is mostly rendering. Yahtzee needs an
AI that fills a scorecard, which is more interesting work. The Dice Lab
could idle by cycling distribution plots, which would actually look good.

### 3. Save-file versioning

Add `save.version` now, while the layout is fresh, plus a migration hook.
Cheap today, and the alternative is silently corrupting people's stats the
first time a key gets renamed.

### 4. Split `main.rs`

609 lines, and the four room menus are the bulk of it. They belong in
something like `ui/dashboard.rs`, leaving `main.rs` as wiring. Worth doing
before a fifth room appears.

### 5. Product depth, if you want the game to grow rather than the codebase

- **A progressive jackpot** shared across the arcade tables, fed by a rake,
  paid on a rare trigger. Luck Bet already has a per-table version of this,
  so the pattern exists.
- **A session summary on quit** — biggest win, worst run, net against the
  house. The data is all in the save file already.
- **Achievements/milestones**, which the `record_best` keys are already
  most of the way toward.
- **A theme picker.** `ui/theme.rs` was built for this and still has only
  one theme; a colour-blind-safe palette would be a genuine accessibility
  win, not just a cosmetic one.

### 6. Smaller polish

- Widen Plinko's slot cells (gap 2).
- Make the floor cycler's 24-second slot configurable in Options.
- Handle a terminal resize mid-animation — currently the next frame simply
  re-fits, which mostly works but can tear one frame.
