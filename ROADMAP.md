# Autonomous Casino Simulation — Roadmap

Companion to `CONTEXT.md`. `CONTEXT.md` describes what the program **is**;
this file describes what the 24-phase autonomous-simulation roadmap is
adding to it, in what order, and why.

The governing constraint, restated so it is never lost between sessions:

> Build **on top of** the existing architecture. Do not replace or rewrite
> working systems. Do not introduce a second system for something that
> already exists. No casino expansion of any kind (Rule 8).

---

## Phase 0 — Audit of the existing architecture

Answers to the eight questions the roadmap asks before any code is written.
All line references are to the tree as of this audit.

### 1. Where the central simulation manager lives

`src/casino/manager.rs`. `Manager` is a handle holding
`Arc<Mutex<Floor>>` + `Arc<AtomicBool>` + a `JoinHandle`. A real OS thread
ticks every `TICK = 40ms`, takes the lock, calls `Floor::tick(now)`, and
drops it. `MAX_CATCH_UP = 4` bounds resync after a stall. `Drop` stops the
flag and joins.

`Floor` (private) owns everything: `bank`, `instances: Vec<Instance>`,
`next_id`, `opened`, `rng`, `speed: u32`, `started: Instant`.

**Verdict:** the manager is real, threaded, and already the sole owner of
simulation state. It stays. Everything the roadmap adds hangs off `Floor`.

**Gaps:** `speed` is an integer round-multiplier applied inside the catch-up
loop, not a clock (Phase 20 needs `0.25x`, which an integer cannot express).
There is no scheduler for anything that is not a table round — operating
costs (Phase 7), demand re-evaluation (Phase 8), tournaments (Phase 17) all
need a periodic hook that does not yet exist.

### 2. Where individual game instances are represented

`src/casino/instance.rs`. `Kind` is a 17-variant enum with
`ALL`, `label`, `seat_word`, `key`, `seats`, `pace`, `variants`,
`variant_name`, and `resolve(rng, variant) -> (staked, returned)`.
`Instance { id, kind, name, patrons, round, paused, next_at, history,
staked, returned, opened, seen }` with `HISTORY = 12` and `MIN_BET = 5`.

`resolve` calls straight into the shipped `games::*::simulate` /
`simulate_at` functions, so a background table runs the *same audited
settlement code* a human plays against. That is the single most valuable
property in the codebase and must survive every phase (Rule 5).

**Gaps:** `Instance` owns its patrons by value (`Vec<Patron>`), so a patron
cannot move between tables and cannot exist while not seated — which is
exactly what Phases 2–4 require. `MIN_BET` is a single global constant, not
a per-table limit (Phase 5). There is no per-kind analytics accumulation
beyond `staked`/`returned` (Phase 14).

### 3. Where player state is stored

`src/casino/patron.rs`. `Patron` has four traits (`nerve`, `appetite`,
`discipline`, `read`, each `0..=100`, rolled as the mean of two draws so the
population clusters), plus `chips`, `bought`, `rounds`, `staked`,
`returned`, `biggest_win`. Behaviour: `stake`, `pick_bet`, `settle`,
`net`, `luck` (**measured, never rolled**), `leaving`, `style`.

**Gaps — this is the biggest one.** Patrons are created inside
`Instance::seat_one` and destroyed inside `turn_over_seats`; they have no
ID, no identity across tables, and no life beyond one sitting. Rule 4 and
Phases 2–4 require the opposite. `style()` is a derived label, not the
archetype Phase 3 asks for.

### 4. Where casino money and chips are stored

`src/casino/bank.rs`. `Bank` holds `money`, `chips`, `collected`, `paid`,
`bought_in`, `cashed_out`, `rounds`, `bets`, and `per_table`. It exposes
`buy_in`, `cash_out`, `settle(table, house_delta)`, `count_round`,
`table_profit`, `cage_profit`, `totals`, `by_table`. It lives behind the
floor lock and nothing outside it keeps a copy. The invariant test
`chips_are_only_ever_moved_never_created` pins conservation across 400
rounds.

Separately, `src/economy.rs` holds `House` — the *human player's* books for
the 23 playable tables. The two ledgers are deliberately distinct and share
only the chip rates (`CHIPS_PER_DOLLAR`, `CHIPS_PER_DOLLAR_SELL`) and the
table keys.

**Verdict:** Rule 3 is already satisfied structurally — `Bank` is private
to the floor and the UI has no mutable path to it.

**Gaps:** no transaction model (Phase 6 wants a typed record per movement,
GGR/NGR); no expense side at all (Phase 7).

### 5. How the UI subscribes to state changes

It does not subscribe — it **polls**. `casino::ui::floor` and
`casino::ui::watch` loop on `poll(REFRESH = 250ms, keys)`, call
`manager.snapshot()` or `manager.table(id)`, get a deep-copied
`FloorView` / `TableView`, and draw it. The one push channel is
`ui::Badge`, three atomics written by the simulation thread and stamped into
the corner by `Screen::present`.

**Verdict:** Rule 1 and Rule 2 already hold. The UI never advances anything,
and closing a watch screen touches no instance. Verified live last session:
hopped away from Slots #1 at round 14 and back at round 17 with patrons and
history intact.

**Gaps:** polling a full deep copy is fine at 51 tables (0.2% CPU, 3.5 MB)
but is O(tables × patrons × history) per frame; Phases 9/13 add feeds and
analytics that must **not** be recomputed per frame (Phase 21).

### 6. How new game modes are registered

Three lists, all hand-maintained, in three files:

- `instance::Kind::ALL` — what the casino can open.
- `games::floor::ATTRACT` — what the idle cycler shows.
- `main.rs::TABLES` — what Stats and the Casino books enumerate.

**Gap:** adding a table means editing three places and a `match` arm in
`resolve`. Phase 14 wants a common analytics interface, which is a natural
moment to collapse these onto one registry — but only if it can be done
without touching the 23 game files.

### 7. Whether an event/message system exists

**No.** There is nothing. The only thing resembling a message is
`ui::input::Poll` (a keypress) and the `Badge` atomics. Phase 1 is
genuinely new construction, not a refactor.

### 8. Whether save/load exists

Barely. `src/stats.rs` `Store` is a flat `key=value` file at
`$XDG_DATA_HOME/dice_arena/save.conf`, written by a single non-atomic
`fs::write`. The casino persists exactly two keys — `casino.money` and
`casino.chips`, written in `main.rs` on exit. Everything else about a
running floor (which tables were open, who was at them, the books' detail)
is lost.

**Gaps:** no version field, no atomic replacement, no validation, no
migration — all four required by Phase 19.

### Summary of what already exists vs. what is new

| Roadmap need | Status |
| --- | --- |
| Simulation/UI separation (Rule 1, 2) | **Exists.** Preserve. |
| Single economy authority (Rule 3) | **Exists.** Extend, don't replace. |
| Real game mechanics decide outcomes (Rule 5) | **Exists.** Never compromise. |
| Threaded manager, snapshots, pause/close | **Exists.** Extend. |
| Event bus (Phase 1) | New. |
| Persistent identified NPCs (Phase 2–4, Rule 4) | New; requires moving patron ownership off `Instance`. |
| Archetypes (Phase 3) | New; `style()` becomes derived-from-archetype. |
| VIP tiers + table limits (Phase 5) | New. |
| Transaction model, GGR/NGR (Phase 6) | New layer over `Bank`. |
| Operating expenses (Phase 7) | New; needs a periodic scheduler. |
| Demand/utilization (Phase 8) | New. |
| Feed, notifications, spectator, follow-action (9–12) | New; all read-only over the bus. |
| Analytics, leaderboards (13–15) | New; incremental accumulation only. |
| Random events, tournaments (16–17) | New. |
| Persistent world + versioned atomic save (18–19) | New; `Store` gains a sibling, not a rewrite. |
| Simulation clock (20) | Replaces the integer `speed` field only. |
| Config (Rule 7) | New; absorbs scattered constants. |

---

## Build order

The roadmap's phase numbers are a wish list, not a dependency order. The
dependency order is:

1. **Config** (Rule 7 / Phase 7's tunables) — every later phase reads it.
2. **Clock** (Phase 20) — every paced system reads it, including the
   scheduler that expenses, demand and tournaments need.
3. **Event bus** (Phase 1) — the substrate for 9, 10, 11, 12, 16.
4. **Roster** (Phases 2–4) — patron ownership moves from `Instance` to
   `Floor`; instances hold `PatronId`s and borrow.
5. **Limits and tiers** (Phase 5) — needs the roster.
6. **Transactions, GGR/NGR, expenses** (Phases 6–7) — needs clock + config.
7. **Demand** (Phase 8) — needs roster + clock.
8. **Read-only UI layers** (Phases 9–15, 23) — need the bus and accumulators.
9. **Events and tournaments** (Phases 16–17) — need everything above.
10. **Persistence** (Phases 18–19) — last, because it must serialise the
    final shape of the world, not an intermediate one.
11. **Tests and end-to-end** (Phases 22, 24) — written alongside each phase,
    with the 24-step integration walk done at the end.

Phase 21 (performance) is not a step; it is a rule applied at every step:
nothing recomputed per frame that can be accumulated per event.

---

## Progress

| Step | Phase(s) | State |
| --- | --- | --- |
| Audit | 0 | **done** — this document |
| Config | Rule 7 | **done** — `casino/config.rs` |
| Clock | 20 | **done** — `casino/clock.rs` |
| Event bus | 1 | **done** — `casino/event.rs`, `casino/sim.rs` |
| Roster | 2, 3, 4 | **done** — `casino/roster.rs`, `Floor::lifecycle` |
| Limits and tiers | 5 | **done** — `instance::Limit`, `cfg.vip_tier`, high-limit tables |
| Transactions and expenses | 6, 7 | **done** — `Movement`/`Expense` in `bank.rs`, `Floor::pay_the_bills`, the books screen |
| Demand | 8 | **done** — `casino/demand.rs`, arrival rate, occupancy |
| Feed, notifications, spectator, follow | 9, 10, 11, 12 | **done** — feed screen, Major notifications, `spectate`, `casino/interest.rs` |
| Analytics, leaderboards | 13, 14, 15 | **done** — `casino/analytics.rs`, the night screen, the customers screen |
| Random events, tournaments | 16, 17 | **done** — `casino/happening.rs`, `casino/tournament.rs` |
| Persistence | 18, 19 | pending |
| Tests, integration | 22, 24 | pending |
| UI integration | 23 | pending |
