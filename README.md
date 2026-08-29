# Dice Arena

A console dice suite in Rust — **zero dependencies**, including its own PRNG.

```
cargo run --release
```

## Games

| | |
|---|---|
| **Pig** | Press-your-luck race to a target score. 2–6 players, any mix of humans and bots, classic (1d6) or two-dice variant (snake eyes wipes your score, doubles pay double). |
| **Yahtzee** | Full 13-category scorecard, three rolls a turn with dice holding, upper-section 63/35 bonus, 100-point bonus Yahtzees. 1–4 players, hot-seat or vs. AI. |
| **Chuck-a-Luck** | Three-dice wagering with a bankroll that persists between sessions. Bet a face (1:1/2:1/3:1), HIGH/LOW at evens, or any triple at 30:1. |
| **Luck Bet** | Eight dice named **A–H**. Up to three players (you + 2 CPUs) each back a letter and the face they think it lands on — 6 by default. A hit pays 5:1 (6:1 with a VIP pass); a miss loses the stake and rakes a fifth of it into a carried-over jackpot. Hit your number while three or more dice show it and you sweep the jackpot too. |
| **Luck Bet: Turbo** | The identical game, hands-free. Press Enter and the table spins in place at 20 frames a second for three seconds before it settles; bets repeat automatically. |
| **Tournament** | Chip buy-in, single-elimination Pig bracket (4 or 8 entrants, three tiers). Your matches are played out, the rest of the bracket is simulated. Champion takes 70% of the pool, runner-up 30%. |
| **Store** | Exchange in-game dollars for chips (10 per $1) and back (12 chips per $1), and buy reroll tokens, insurance chits, lucky charms, a VIP pass or a gold dice skin. |
| **Dice Lab** | Roll any `NdM+K` (`3d6`, `d20`, `4d10+2`), plot a distribution with `sim 2d6 50000`, and pin the RNG with `seed 42`. |

## Economy

Chips are the table currency, dollars are the in-game cash — **all of it fictional**,
with no real money involved anywhere. One shared wallet spans Luck Bet,
Chuck-a-Luck and the tournaments, and it persists between sessions. Go broke and
the house stakes you again, so the game is never a dead end.

## Features

- ASCII pip faces with held-dice highlighting, colored output (toggleable)
- Three AI difficulties — Easy is erratic, Normal holds at 20, Hard uses the
  near-optimal "hold at 25 minus banked" rule and pushes when it's behind
- Persistent stats, high scores and settings in `$XDG_DATA_HOME/dice_arena/save.conf`
- Reproducible sessions: `DICE_SEED=42 cargo run`
- Persistent inventory: consumables (reroll, insurance, charm) and permanent
  upgrades (VIP payouts, gold dice)
- Handles EOF cleanly, so the whole app is scriptable from a pipe

## Tests

```
cargo test
```

Covers dice-notation parsing, roll uniformity, seeded reproducibility, every
Yahtzee scoring category, and Luck Bet hit/sweep resolution.
