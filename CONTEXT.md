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
| Source | 21,267 lines across 58 files |
| Tests | 355, all passing |
| Starting balances | 10,000 chips / $9,000 — the user's own tuning in `economy.rs` |
| Clippy | clean, zero warnings |
| Dependencies | none |
| Tables | 24 (8 dice, 6 card, 2 wheel, 8 arcade) |
| Idle screens | 17 tables + a floor-wide cycler |
| Background casino | a real simulation thread; 51 tables at 0.2% CPU |
| Autonomous roadmap | Phases 0-17 and 20 done — see `ROADMAP.md` |

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

**Session 4 — the big expansion.** Five threads:

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

**Session 5 — the house auditor.** Built `games/audit.rs`, the harness that
plays every table headlessly and holds its realised return to a declared
band. Getting there meant giving each table a `simulate(rng) -> (staked,
returned)` entry point that runs through the *real* settlement code, which
in turn meant two worthwhile refactors: every draw function that only ever
needed the RNG now takes `&mut Rng` rather than a whole `Ctx` (and the tests
that used to fake a `Ctx` got simpler), and Baccarat now resolves its hand
first and replays it — the same outcome-first discipline the wheel and the
race already used. Blackjack's settlement came out of `play` into
`settle_hand` for the same reason.

The harness paid for itself immediately: **Keno was badly mispriced.** Its
return ran from 75% on a one-spot board down to **39.8% on a ten-spot** —
against 93–99% everywhere else in the building, which quietly made the big
boards a trap. Retuned against the exact hypergeometric odds, in hundredths
so a one-spot board can be priced honestly at all, every board now returns
90–93%. The exact check lives in `keno.rs` and cannot drift.

**Session 6 (most recent) — the living casino.** `[A] Start Casino` on the
dashboard opens a floor that runs itself on a **real OS thread**, entirely
independently of whatever screen is showing. This is the first genuinely
concurrent thing in the codebase.

The five pieces map onto the brief one-for-one, and the separation is the
point rather than an accident:

- `casino/bank.rs` — the economy manager. Money and chips are genuinely
  separate: chips move bet by bet at the tables, money moves only at the
  cage. Because the cage sells chips at 10/$1 and buys them back at 12/$1
  (the Store's own long-standing spread), the house takes a cut on every
  visit before a single bet is struck — which is why the two numbers move
  independently and why watching them is interesting.
- `casino/patron.rs` — simulated players. Four traits (nerve, appetite,
  discipline, read), each the average of two rolls so the extremes are rare.
  **No trait bends an outcome.** Appetite changes *which real bet* a patron
  takes off the table's ladder, nerve changes the stake, discipline decides
  when they walk. `luck` is measured after the fact, never rolled — letting a
  patron be born lucky would quietly undo `games::audit`.
- `casino/instance.rs` — one table: seats, round clock, bounded history.
  Rounds resolve through each game's own `simulate` entry point, so a
  background table *is* the real game with the animation taken away.
- `casino/manager.rs` — the simulation thread. Owns every instance, advances
  the ones whose round is due, hands out read-only snapshots.
- `casino/ui.rs` — screens that draw snapshots and nothing else.

The badge in the top-right is a lock-free `ui::Badge` the simulation thread
writes once per tick, and `Screen::present` stamps into row 1 with an
absolute cursor move. That is why it stays current on *every* screen —
including deep inside a hand of blackjack the user is playing themselves —
without a single other screen knowing it exists.

Verified live: watching Slots #1 it ran 3→14; hopping to Slots #2 found it
already at round 16 having run unwatched; hopping back found #1 at 17, still
holding its patrons and history. Fifty-one tables run at 0.2% CPU and 3.5 MB
across two threads.

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
  casino/                  ← the background simulation (session 6-7)
    mod.rs          40  what the nine pieces are and why they are separate
    config.rs      411  every tunable: limits, tiers, thresholds, speed, costs
    demand.rs      371  what the room is in the mood for + occupancy
    analytics.rs   435  the night in bucketed time ranges + per-game figures
    happening.rs   420  things that happen to the room: rates and costs only
    tournament.rs  600  a fixed field playing down to one winner
    interest.rs    339  how worth watching a table is, and why
    clock.rs       248  simulated time, permille speed, the `Every` period
    event.rs       460  the event bus: Event, Weight, Record, Feed
    sim.rs          45  the borrow bundle a table gets for one call
    bank.rs        471  the economy manager: movements, GGR/NGR, expenses
    patron.rs      751  archetypes, traits, presence, lifetime record
    roster.rs      402  everyone the casino knows, seated or not
    instance.rs    845  one running table, its limit, the Kind → game mapping
    manager.rs    1762  simulation thread, lifecycle, bills, mood, tourneys
    ui.rs         1043  floor, feed, spectator, night, customers, books, tourneys
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

### The casino simulation

- **The UI is a viewer, never a driver.** Nothing under `casino/ui.rs` may
  advance a simulation, and nothing above `manager.rs` may hold game state.
  Break this and "the table kept running while I was away" stops being true.
- **The lock is never held across a draw or a keypress** — only for a tick or
  a snapshot copy. The UI cannot stall the floor and the floor cannot stall
  the UI.
- **Tables are paced by the clock, not by tick count** (`Instance::next_at`),
  so pace survives a slow or busy machine. A paused table has its `next_at`
  pushed forward each tick so resuming it does not fire a burst of rounds.
- **Total chips are conserved.** Buy-in, settlement and cash-out are all
  transfers, so the tray plus every stack in the room is a constant — pinned
  by `chips_are_only_ever_moved_never_created`. If that ever fails, the
  casino is printing money.
- **Patron traits must never touch an outcome.** They pick bets and sizes;
  the table's audited maths does the rest.
- **`simulate` is now shipped, not test-only.** The casino drives real games
  through it. The plain `simulate` on tables the casino reaches via
  `simulate_at` (baccarat, bigsix, chuck, horses, keno, plinko, roulette) is
  still `#[cfg(test)]` — only the audit uses those.
- **Padding coloured text needs `ui::pad_end` / `ui::pad_start`.**
  `format!("{:<9}")` counts escape bytes as characters and shears columns —
  the same bug the `10` card had.

---

### The autonomous-simulation layer (session 7)

`ROADMAP.md` is the companion document for the 24-phase roadmap: the Phase 0
audit of what already existed, the dependency order the phases are actually
being built in, and a progress table. **Read it before picking up the next
phase** — it is written so a fresh session does not have to re-derive where
anything lives.

Four things landed, in the order the dependency graph demanded:

1. **`casino/config.rs`** — every tunable in one struct held by `Floor`:
   bet limits, tier thresholds, buy-in range, event thresholds, feed size,
   clock speed, operating costs. Nothing is hard-coded where it is used.
   Read through `sim.cfg` in the simulation and `manager.config()` in the
   UI. `Manager::configure` replaces it wholesale.
2. **`casino/clock.rs`** — simulated time. `Instance::next_at` and
   `Instance::opened` are now `Duration`s since the doors opened, not
   `Instant`s, which is what makes 0.25x possible: a slow clock genuinely
   means fewer rounds rather than the same rounds drawn more often. Speed is
   permille (`1_000` = 1x) so no float touches anything. `Every` is the
   periodic deadline Phase 7's expenses will hang off.
   *Gotcha:* `Clock::started_at` and `set_speed` take the wall `Instant` as
   an argument rather than sampling it. That is deliberate — a clock that
   reads the machine's time on its own account cannot be tested.
3. **`casino/event.rs`** — the bus. Publishers push, readers pull under the
   lock they already take for a snapshot; there is no callback registry, so
   no reader's code can ever run on the simulation thread. `Weight` is what
   makes "don't display every low-level event" enforceable: `Routine`
   traffic is *counted but never kept*, so a busy floor cannot evict the
   readable feed with its own noise.
4. **`casino/sim.rs`** — `Sim<'a>`, the bundle of floor-owned services a
   table borrows for one call (`rng`, `bank`, `feed`, `cfg`, `now`,
   `next_patron`). Every later phase adds a field here instead of changing
   twenty signatures.

Consequences worth knowing before touching this code:

- `Instance::new` / `play_round` / `seat_one` / `turn_over_seats` all take
  `&mut Sim` now. The instance tests use a `Bench` helper that stands up the
  same five services.
- `Patron` has an `id: u64`, minted by the *floor* (`Sim::patron_id`), not
  by a table. `Patron::leaving` returns `Option<Departure>` rather than
  `bool`, because the feed and the statistics both want the cause.
- `instance::MIN_BET` is gone; it is `cfg.min_bet`. Bets are clamped to
  `cfg.bet_ceiling(cfg.tier_of(p.staked))`, so the tier ladder already binds
  even before Phase 5 gives it a screen.
- The floor screen has an `ON THE FLOOR` strip fed from
  `FloorView::feed`, and shows casino time alongside wall time whenever the
  speed is not 1x.

### The persistent roster (session 7, Phases 2-4)

The single largest structural change since the casino itself: **people moved
off the tables and onto the floor.**

- `Instance::patrons` is now `Vec<u64>` — seat ids, not people. A table
  holds seats; the people live in `casino/roster.rs`, owned by `Floor`.
- `Patron` gained an `Archetype` (Conservative, Gambler, Strategist,
  Chaser, Whale, Beginner, Lucky), a `Presence` (`Away { back_at }` /
  `Looking` / `Seated { table }`) and a `Lifetime` record that accumulates
  across visits. `begin_visit` / `end_visit` are the boundary: the per-visit
  figures reset, the lifetime ones never do.
- `Instance::seat_one` and `turn_over_seats` are **gone**. Seating is
  `Floor::lifecycle(now)`, run once per tick before the rounds: people whose
  time away is up walk back in, people done at a table get up and cash out,
  and empty seats are offered to whoever is in the building. A patron picks
  the table that best suits them via `Patron::taste(variants, pace_ms)`.
- The population is **bounded** by `cfg.roster_size` (120). Past the cap a
  seat is filled by somebody coming back, not by a stranger being invented —
  which is what Rule 4 is actually asking for, and also what stops an
  all-night run leaking a person per seat.

Things that will bite if you forget them:

- **An archetype never touches an outcome.** `Lucky` is a *staking*
  behaviour (a wider jitter) and a table preference, nothing else. There is
  a test — `a_lucky_player_is_a_behaviour_and_never_a_thumb_on_the_scale` —
  whose whole job is to fail if anyone changes that.
- `play_round` destructures `Sim` (`let Sim { rng, bank, feed, cfg, roster,
  now } = sim;`) so a patron can be borrowed out of the roster while the rng
  and bank are in use. A seat whose id is not in the roster is *skipped*,
  never treated as a reason to stop the table.
- The conservation invariant now sums the tray plus `roster.chips_in_play()`
  rather than one table's stacks. Departed patrons hold no chips — they hand
  them back in `end_visit`.
- `Manager::open` no longer seats anybody, so the cage does not move until
  the next lifecycle pass. A test that asserts otherwise is asserting the
  old architecture.
- `TableView` and `FloorView` now carry a `Config` clone, so the drawing
  code names a tier through `cfg.tier_name(...)` instead of writing any
  threshold down. `FloorView` also carries `crowd`, `known` and `by_tier`.

### The formal books and the house's costs (session 7, Phases 6-7)

`Bank` grew a **transaction model** and an **expense side**, without being
rewritten: everything that was there still means what it meant.

- `Movement` names every way money or chips can move (`BuyIn`, `CashOut`,
  `Wager`, `Payout`, `Expense`) and each is *accumulated*, never logged — a
  busy floor makes millions an hour. `Movement::in_dollars()` is how the two
  currencies stay apart.
- `Bank::settle` now takes `(table, staked, returned)` rather than a net
  delta, which is what gives the books a **handle** (turnover) to measure
  against. `ggr()` is the gaming win in chips, `hold()` is it as a share of
  the handle in hundredths of a percent, `ngr()` is the bottom line **in
  cash**: `cage_profit() - spent()`.
- **Why NGR is cash and not chips:** chips only ever leave the building
  through the cage, so cage flow already *is* the gaming win converted at
  the house's own spread. Adding a chip figure to it would count the same
  money twice. If you ever feel the urge to write `ngr = ggr + cage`, this
  is the paragraph that says don't.
- `Expense` is `Overhead` (the building) and `Staffing` (per open table).
  `Bank::pay` takes it out of the **cage** and never out of the tray, and
  lets the cage go negative — a casino losing money should look like one.
- `Floor::pay_the_bills(now)` runs off `clock::Every`, in a `while` loop so
  a fast clock or a busy spell pays every period it owes rather than
  skipping them. `set_speed` calls `rent.resync` so winding the clock
  forward does not present a backlog of unpaid rent.
- New screen: **`m` from the floor** opens THE HOUSE BOOKS — handle, payouts,
  gross win, realised hold, the cage, costs by kind, the bottom line, and
  the movement ledger.

**A real bug the books surfaced.** Scaling a table's unit-stake payout to
the patron's actual stake used to truncate: `unit_returned * stake /
unit_staked`. That shaves a fraction off every *winning* settlement and none
off a losing one — a house edge nobody audited and nobody wrote down, worst
at the small stakes where a whole chip is a large share of the bet. It is
now `instance::scale_payout`, which rounds to nearest, with its own tests.
The realised floor-wide hold went from a nonsense figure to about 2%.

### Demand, occupancy and the velvet rope (session 7, Phases 8 and 5)

`casino/demand.rs` holds two separate things, and keeping them apart is the
point:

- **Appeal** — how much the room fancies a kind of game right now, permille
  against a neutral `1_000`. It drifts on its own clock as a random walk
  pulled back toward neutral, bounded by `cfg.appeal_floor/ceiling`, and it
  multiplies a person's own `taste` when they choose a table.
- **Utilization** — how full each kind's tables have actually been, sampled
  periodically and accumulated.

**Appeal decides where somebody sits and nothing else.** A hot table is a
busy table, not a generous one. There is a test in `demand.rs` whose only
job is to fail if anybody wires appeal into a payout.

**The thing that made occupancy real.** The first version measured 100%
forever, because the lifecycle refilled a seat the instant it emptied — the
supply of people was whatever the seats demanded, so utilization was a
tautology. Now people arrive at a *rate* (`cfg.arrivals_period` /
`arrivals_per_period`), and only somebody already `Looking` can be seated:
`Roster::admit` is the front door, `Roster::waiting` fills a seat and
**never invents anybody**. Anyone left standing about gives up and goes home
with probability `cfg.gives_up` per pass, so a room with too few tables does
not silently fill with people who never play. A real floor now reads ~95%,
and a floor with more seats than customers reads well under it — which is
what makes over-opening cost you staffing money for nothing.

A subtlety worth keeping: a table re-rolls how many seats it *wants* as
people come and go, so a sample can catch it seating more than it is asking
for. `record_sample` counts `wanted.max(seated)` as offered — otherwise a
busy floor reads as over 100% full, which is not a thing occupancy can be.

**Phase 5 finished.** `instance::Limit` is `House` or `High`. A high-limit
table wears a `★` in its name, imposes `cfg.vip_max_bet`, and only seats
people the house counts as VIPs. Two ceilings apply to every bet and **the
lower one wins** — the table's and the patron's own — so neither can smuggle
the other up. Who counts as a VIP is `cfg.vip_tier` (default `2`, "high
roller") rather than the top tier, because pinning it to the top would leave
the high-limit tables empty all night. `Manager::open_at(kind, n, limit)`,
and the open-a-table screen asks which.

### Watching the place (session 7, Phases 9-12)

Four screens, all of them read-only over things that already existed.

- **`e` — the feed.** The whole event stream as a screen, filtered to
  `Notable` and up, with `b` to narrow to `Major`. The filter is the point:
  a floor publishes millions of things an hour, and it is the *publisher's*
  weight that decides what can appear at all.
- **Notifications.** The floor screen keeps a feed cursor and announces
  anything new that clears `Weight::Major`. It starts from the current
  cursor, so walking in does not replay the night at you, and `feed_since`
  guarantees nothing is announced twice. The screen does not know what a big
  win is — the simulation set the weight, from `cfg.big_win` / `huge_win` /
  `jackpot_multiple`.
- **`v` — spectator mode.** Holds on a table for `cfg.spectate_for`, then
  moves to whatever is most worth watching. `h` holds, `n` steps, `f`
  resumes following. **It never pauses anything** — `z` still pauses a
  table, because that is the user asking by name.
- **`casino/interest.rs` — follow the action.** Scores a table on money
  through it, the biggest recent hit, the crowd, a VIP in a seat, and the
  house losing. Every weight is configuration (`cfg.interest_*`), and the
  score reports *why* it ranked a table where it did, so the spectator can
  say "the players are winning" instead of silently teleporting you.

The score reads only a table's bounded recent history, so it costs the same
whether the casino opened a minute ago or has run all night, and it is
computed once per snapshot under the lock rather than per frame. Scoring a
table cannot touch it: there is a test that runs the scorer fifty times over
and asserts the table is byte-for-byte where it was.

### The night in numbers (session 7, Phases 13-15)

`casino/analytics.rs`, and two screens: **`t`** for the night, **`c`** for
the customers.

The rule the whole file exists to enforce, stated as strongly as it can be:
**nothing is ever recomputed from history.** Every figure on every screen is
a sum of counters that were incremented once, when the thing they count
happened.

- **`Series`** — a bounded ring of 30-second buckets, 120 of them. A range
  query adds up the handful of buckets it covers, so "the last ten minutes"
  costs the same as "the last minute", both cost the same at 3am as at
  opening, and an all-night run stops growing after the first hour.
  `Span::AllNight` is answered from a single running total and costs
  nothing. A long stall resyncs rather than walking ten thousand empty
  buckets — the night's totals survive, only the shape of the recent past is
  lost, which is the honest outcome.
- **`Games`** — Phase 14's "common interface", and deliberately *not* a
  trait: every game is measured by the same six numbers, keyed by the same
  string the books and the stats file already use. A new table gets
  analytics for free and has nowhere to hide a special case.
- **Leaderboards** — three lists that are rarely the same people: biggest
  spenders (lifetime turnover), up on the house (lifetime net), and
  regulars (visits). The third could not have existed at all before patrons
  became persistent, which is why it is worth its own column.

Rounds are folded in at the point they are played, from the `RoundLog` the
table already produced; arrivals and departures are counted in the lifecycle
pass. `Series::advance` runs once per tick, before anything else.

### Weather and tournaments (session 7, Phases 16-17)

**`casino/happening.rs`** — things that happen to the room: a rush on the
doors, a quiet spell, word getting round about one game, somebody worth
looking at walking in, being short-staffed, a comp night. Shown at the top
of the floor screen (`tonight: …`) and announced on the feed.

Every one of them moves **a rate or a cost, and nothing else**. There is no
variant that can make a table pay better, and the test
`no_happening_can_ever_touch_a_payout` is written so that adding one breaks
the build. Effects multiply rather than override, so a rush during a comp
night is genuinely busier and a lull cancels a rush out instead of one of
them silently winning.

**`casino/tournament.rs`** — a fixed field, everybody level, playing down to
one winner. Press **`r`**. The bracket is by *survival* rather than pairings:
every survivor plays the same hands at the same ante each round and the
short stacks go out, so the field halves (128 → 64 → 32 …) to a final table
and then to a winner. Hands are resolved by the same `Kind::resolve` a cash
table uses — a short stack does not get luckier because it would be a better
story.

Money: entries come out of patrons' own chips, the house takes
`cfg.tourney_rake` **once, at the door**, and the pool is paid back out in
full — the last place paid absorbs the rounding, so nothing is lost to
integer division. Tournament chips are a separate scoring currency that
never touches the tray. A tournament that does not fill is called off and
every chip goes back, rake included.

Two ordering facts that are load-bearing, both found by a failing test:

- **`run_tournaments` runs before `seat_the_room`.** Registration and
  seating draw from the same pool of people standing about, and whichever
  runs first takes the lot. With seating first, a tournament could never
  field anybody at all. That is why the lifecycle's seating pass is now its
  own method.
- **A finished tournament's retention window is measured from when it
  *finished*, not when it started.** Measured from the start, one that took
  an hour to play down was forgotten the instant somebody won it — exactly
  the moment anybody would want to look at it.

Entering a tournament counts as starting a visit, so somebody who has just
walked in buys chips at the cage first, exactly as they would sitting down.
Entrants are `Presence::InTournament`, which is why the seating loop leaves
them alone.

---

## Delivering changes

The cloud workspace this is developed in is a *mirror*, not the repo. Two
rules, both learned the hard way:

1. **A change that spans modules must ship every file it touched.** The
   session-6 casino failed to build on the real machine because the
   `casino/` files shipped but the seventeen game files whose `#[cfg(test)]`
   gates had been lifted did not. A green build in the mirror proves
   nothing about the repo unless the same set of files is in both.
2. **Diff against the real repo before shipping, not after.** Stage the
   files with `device_stage_files` and `cmp` them. That is also how the
   user's own edit to `economy.rs` was caught before it got overwritten —
   they had raised the starting balances (`START_CHIPS` 10000,
   `START_DOLLARS` 9000) and a blind commit would have reverted it.

A from-scratch build in a pristine copy (`cp -r src Cargo.toml` somewhere
new, then `cargo build --release`) is the check that actually matches what
`cargo install` does on the user's machine.

## Verifying changes

`cargo test` covers the maths, and `cargo test --release audit --
--nocapture` prints the whole book of returns — the fastest way to see that
a paytable edit did what you meant. It cannot cover the thing this app mostly
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
4. **Big Six returns 75–89%**, well below the 93–99% the rest of the floor
   pays. That is authentic — a real money wheel is the meanest thing on any
   casino floor — but it is inconsistent with this building. Worth deciding
   deliberately rather than leaving as an accident.
5. **Video poker and blackjack are audited under sub-optimal play** (demo
   holds, and hit-below-17 with no doubling). Their bands are floors, not
   the tables' headline returns. Real basic strategy would raise both.

---

## Next steps

**The roadmap comes first.** `ROADMAP.md` carries the ordered plan and the
progress table; the next step there is the persistent NPC roster (Phases
2–4), which means moving patron ownership off `Instance` and onto `Floor`
so a patron can exist while not seated. The items below are the older
backlog and sit behind it.


Roughly in the order I would take them.

### 1. Persist the running floor

The casino's *balances* survive a restart (`casino.money` / `casino.chips`
in the save file) but its *tables* do not — reopening starts a fresh floor.
Serialising the instance roster would make the casino genuinely persistent
between sessions, which is the one part of the brief's "living environment"
that is currently only true within a run.

### 2. Close the four idle gaps

Pig and Tournament are cheap — `pig::simulate` already runs a headless
match, so a bot-vs-bot display loop is mostly rendering. Yahtzee needs an
AI that fills a scorecard, which is more interesting work. The Dice Lab
could idle by cycling distribution plots, which would actually look good.

### 3. Decide what Big Six should pay

The auditor makes it plain that Big Six is the outlier on the floor at
75–89%. Authentic, but out of keeping. Either soften the paytable to sit
with everything else, or leave it and say so on the table's own screen so a
player is choosing it knowingly rather than being caught out.

Pig and Tournament are cheap — `pig::simulate` already runs a headless
match, so a bot-vs-bot display loop is mostly rendering. Yahtzee needs an
AI that fills a scorecard, which is more interesting work. The Dice Lab
could idle by cycling distribution plots, which would actually look good.

### 4. Save-file versioning

Add `save.version` now, while the layout is fresh, plus a migration hook.
Cheap today, and the alternative is silently corrupting people's stats the
first time a key gets renamed.

### 5. Split `main.rs`

609 lines, and the four room menus are the bulk of it. They belong in
something like `ui/dashboard.rs`, leaving `main.rs` as wiring. Worth doing
before a fifth room appears.

### 6. Product depth, if you want the game to grow rather than the codebase

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

### 7. Smaller polish

- Widen Plinko's slot cells (gap 2).
- Make the floor cycler's 24-second slot configurable in Options.
- Handle a terminal resize mid-animation — currently the next frame simply
  re-fits, which mostly works but can tear one frame.
