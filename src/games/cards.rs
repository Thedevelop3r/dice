//! The shared card engine: one 52-card deck, one shoe, and one set of
//! poker hand rankings for every card game on the floor.
//!
//! Blackjack used to carry its own private `Card`/shoe pair; with six card
//! tables in the building that would have meant six copies of the same
//! shuffle. Everything card-shaped now goes through here, so a fix to the
//! deal or the rankings lands in every game at once.
//!
//! Ranks are `1..=13` with 1 meaning Ace, which is how a deck is naturally
//! indexed. Poker comparisons want an ace *high*, so `poker_rank` maps that
//! to `2..=14` and the two wheel straights (A-2-3-4-5 and A-2-3) are picked
//! out explicitly where they matter.
//!
//! Like `ui::theme`, this module deliberately offers a little more than
//! any single table uses today — the whole deck, both hand rankings, a
//! raw shuffle — so a new card game is a new file and nothing else.
#![allow(dead_code)]

use crate::rng::Rng;

pub const SUITS: [char; 4] = ['♠', '♥', '♦', '♣'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// 1..=13, where 1 is an Ace and 11/12/13 are J/Q/K.
    pub rank: u8,
    /// 0=♠ 1=♥ 2=♦ 3=♣.
    pub suit: u8,
}

impl Card {
    pub fn new(rank: u8, suit: u8) -> Card {
        Card { rank, suit }
    }

    /// The corner index the renderer prints, e.g. `A♠` or `10♦`.
    pub fn label(&self) -> String {
        let r = match self.rank {
            1 => "A".to_string(),
            11 => "J".to_string(),
            12 => "Q".to_string(),
            13 => "K".to_string(),
            n => n.to_string(),
        };
        format!("{r}{}", SUITS[(self.suit % 4) as usize])
    }

    pub fn is_red(&self) -> bool {
        matches!(self.suit, 1 | 2)
    }

    /// Blackjack's count: an ace is 11 here and drops to 1 in `hand_total`
    /// only if the hand would otherwise bust.
    pub fn blackjack_value(&self) -> u32 {
        match self.rank {
            1 => 11,
            n if n >= 10 => 10,
            n => n as u32,
        }
    }

    /// Ace-high ordering for poker: 2..=14.
    pub fn poker_rank(&self) -> u8 {
        if self.rank == 1 {
            14
        } else {
            self.rank
        }
    }
}

/// A shuffled shoe of one or more decks that reshuffles itself the moment
/// it runs dry, so no table can ever deal off an empty pile.
#[derive(Debug, Clone)]
pub struct Shoe {
    cards: Vec<Card>,
    decks: usize,
}

impl Shoe {
    pub fn new(rng: &mut Rng, decks: usize) -> Shoe {
        let decks = decks.max(1);
        let mut shoe = Shoe { cards: Vec::new(), decks };
        shoe.refill(rng);
        shoe
    }

    fn refill(&mut self, rng: &mut Rng) {
        self.cards.clear();
        self.cards.reserve(52 * self.decks);
        for _ in 0..self.decks {
            for suit in 0..4u8 {
                for rank in 1..=13u8 {
                    self.cards.push(Card { rank, suit });
                }
            }
        }
        shuffle(&mut self.cards, rng);
    }

    pub fn draw(&mut self, rng: &mut Rng) -> Card {
        if self.cards.is_empty() {
            self.refill(rng);
        }
        self.cards.pop().expect("a shoe always refills before it is drawn from")
    }

    pub fn deal(&mut self, rng: &mut Rng, n: usize) -> Vec<Card> {
        (0..n).map(|_| self.draw(rng)).collect()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

/// A single shuffled 52-card deck, for games that want to see the whole
/// pack rather than draw from a running shoe.
pub fn deck(rng: &mut Rng) -> Vec<Card> {
    let mut cards = Vec::with_capacity(52);
    for suit in 0..4u8 {
        for rank in 1..=13u8 {
            cards.push(Card { rank, suit });
        }
    }
    shuffle(&mut cards, rng);
    cards
}

/// Fisher-Yates, the same shuffle the tournament bracket and Ultra's seat
/// order use.
pub fn shuffle(cards: &mut [Card], rng: &mut Rng) {
    for i in (1..cards.len()).rev() {
        let j = rng.below(i + 1);
        cards.swap(i, j);
    }
}

/// Cards in the `(label, is_red)` form `ui::card_art` lays out.
pub fn labels(cards: &[Card]) -> Vec<(String, bool)> {
    cards.iter().map(|c| (c.label(), c.is_red())).collect()
}

/// Standard five-card poker categories, weakest to strongest, so the
/// derived `Ord` compares hands the right way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandRank {
    HighCard,
    Pair,
    TwoPair,
    Trips,
    Straight,
    Flush,
    FullHouse,
    Quads,
    StraightFlush,
}

impl HandRank {
    pub fn name(self) -> &'static str {
        match self {
            HandRank::HighCard => "high card",
            HandRank::Pair => "pair",
            HandRank::TwoPair => "two pair",
            HandRank::Trips => "three of a kind",
            HandRank::Straight => "straight",
            HandRank::Flush => "flush",
            HandRank::FullHouse => "full house",
            HandRank::Quads => "four of a kind",
            HandRank::StraightFlush => "straight flush",
        }
    }
}

/// A scored hand: its category plus the tiebreakers that separate two
/// hands of the same category, most significant first. Two `Score`s
/// compare exactly as the hands they came from do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Score {
    pub rank: HandRank,
    pub kickers: Vec<u8>,
}

/// Groups a hand's ranks into `(count, rank)` pairs sorted the way poker
/// tiebreaks read: biggest group first, then highest rank.
fn rank_groups(cards: &[Card]) -> Vec<(usize, u8)> {
    let mut counts: [usize; 15] = [0; 15];
    for c in cards {
        counts[c.poker_rank() as usize] += 1;
    }
    let mut groups: Vec<(usize, u8)> = (2..=14u8).filter(|r| counts[*r as usize] > 0).map(|r| (counts[r as usize], r)).collect();
    groups.sort_by(|a, b| b.cmp(a));
    groups
}

fn is_flush(cards: &[Card]) -> bool {
    !cards.is_empty() && cards.iter().all(|c| c.suit == cards[0].suit)
}

/// The high card of the straight these ranks form, if they form one.
/// Handles the wheel, where the ace plays low and the straight is counted
/// by its top card (5 for A-2-3-4-5, 3 for A-2-3).
fn straight_high(cards: &[Card]) -> Option<u8> {
    let mut ranks: Vec<u8> = cards.iter().map(|c| c.poker_rank()).collect();
    ranks.sort_unstable();
    ranks.dedup();
    if ranks.len() != cards.len() {
        return None;
    }
    let n = ranks.len();
    if ranks[n - 1] - ranks[0] == (n - 1) as u8 {
        return Some(ranks[n - 1]);
    }
    // The wheel: an ace sitting on top of a run that starts at 2.
    if ranks[n - 1] == 14 && ranks[0] == 2 && ranks[n - 2] - ranks[0] == (n - 2) as u8 {
        return Some(ranks[n - 2]);
    }
    None
}

/// Scores a five-card hand.
pub fn evaluate5(cards: &[Card]) -> Score {
    debug_assert_eq!(cards.len(), 5, "evaluate5 wants exactly five cards");
    let groups = rank_groups(cards);
    let flush = is_flush(cards);
    let straight = straight_high(cards);

    if let (true, Some(high)) = (flush, straight) {
        return Score { rank: HandRank::StraightFlush, kickers: vec![high] };
    }
    let shape: Vec<usize> = groups.iter().map(|(n, _)| *n).collect();
    let by_rank: Vec<u8> = groups.iter().map(|(_, r)| *r).collect();
    let (rank, kickers) = match shape.as_slice() {
        [4, 1] => (HandRank::Quads, by_rank),
        [3, 2] => (HandRank::FullHouse, by_rank),
        _ if flush => (HandRank::Flush, by_rank),
        _ if straight.is_some() => (HandRank::Straight, vec![straight.unwrap()]),
        [3, 1, 1] => (HandRank::Trips, by_rank),
        [2, 2, 1] => (HandRank::TwoPair, by_rank),
        [2, 1, 1, 1] => (HandRank::Pair, by_rank),
        _ => (HandRank::HighCard, by_rank),
    };
    Score { rank, kickers }
}

/// True when a hand is the very top straight flush — ten through ace.
pub fn is_royal(cards: &[Card]) -> bool {
    let s = evaluate5(cards);
    s.rank == HandRank::StraightFlush && s.kickers.first() == Some(&14)
}

/// Three-card poker categories. The order is genuinely different from the
/// five-card game: with only three cards a straight is *harder* to make
/// than a flush, so it pays more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreeRank {
    HighCard,
    Pair,
    Flush,
    Straight,
    Trips,
    StraightFlush,
}

impl ThreeRank {
    pub fn name(self) -> &'static str {
        match self {
            ThreeRank::HighCard => "high card",
            ThreeRank::Pair => "pair",
            ThreeRank::Flush => "flush",
            ThreeRank::Straight => "straight",
            ThreeRank::Trips => "three of a kind",
            ThreeRank::StraightFlush => "straight flush",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ThreeScore {
    pub rank: ThreeRank,
    pub kickers: Vec<u8>,
}

/// Scores a three-card hand under Three Card Poker's ranking.
pub fn evaluate3(cards: &[Card]) -> ThreeScore {
    debug_assert_eq!(cards.len(), 3, "evaluate3 wants exactly three cards");
    let groups = rank_groups(cards);
    let flush = is_flush(cards);
    let straight = straight_high(cards);
    let by_rank: Vec<u8> = groups.iter().map(|(_, r)| *r).collect();

    if let (true, Some(high)) = (flush, straight) {
        return ThreeScore { rank: ThreeRank::StraightFlush, kickers: vec![high] };
    }
    if groups[0].0 == 3 {
        return ThreeScore { rank: ThreeRank::Trips, kickers: by_rank };
    }
    if let Some(high) = straight {
        return ThreeScore { rank: ThreeRank::Straight, kickers: vec![high] };
    }
    if flush {
        return ThreeScore { rank: ThreeRank::Flush, kickers: by_rank };
    }
    if groups[0].0 == 2 {
        return ThreeScore { rank: ThreeRank::Pair, kickers: by_rank };
    }
    ThreeScore { rank: ThreeRank::HighCard, kickers: by_rank }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(spec: &[(u8, u8)]) -> Vec<Card> {
        spec.iter().map(|(r, s)| Card::new(*r, *s)).collect()
    }

    #[test]
    fn labels_read_the_way_a_card_is_named() {
        assert_eq!(Card::new(1, 0).label(), "A♠");
        assert_eq!(Card::new(10, 2).label(), "10♦");
        assert_eq!(Card::new(13, 1).label(), "K♥");
        assert_eq!(Card::new(7, 3).label(), "7♣");
    }

    #[test]
    fn only_hearts_and_diamonds_are_red() {
        assert!(!Card::new(1, 0).is_red());
        assert!(Card::new(1, 1).is_red());
        assert!(Card::new(1, 2).is_red());
        assert!(!Card::new(1, 3).is_red());
    }

    #[test]
    fn a_shoe_deals_every_card_before_it_reshuffles() {
        let mut rng = Rng::from_seed(4);
        let mut shoe = Shoe::new(&mut rng, 1);
        assert_eq!(shoe.len(), 52);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..52 {
            let c = shoe.draw(&mut rng);
            assert!(seen.insert((c.rank, c.suit)), "dealt {c:?} twice from one deck");
        }
        assert!(shoe.is_empty());
        // The 53rd draw refills rather than panicking.
        shoe.draw(&mut rng);
        assert_eq!(shoe.len(), 51);
    }

    #[test]
    fn a_multi_deck_shoe_holds_every_deck() {
        let mut rng = Rng::from_seed(9);
        assert_eq!(Shoe::new(&mut rng, 6).len(), 312);
    }

    #[test]
    fn five_card_categories_are_identified() {
        assert_eq!(evaluate5(&hand(&[(10, 0), (11, 0), (12, 0), (13, 0), (1, 0)])).rank, HandRank::StraightFlush);
        assert_eq!(evaluate5(&hand(&[(9, 0), (10, 0), (11, 0), (12, 0), (13, 0)])).rank, HandRank::StraightFlush);
        assert_eq!(evaluate5(&hand(&[(7, 0), (7, 1), (7, 2), (7, 3), (2, 0)])).rank, HandRank::Quads);
        assert_eq!(evaluate5(&hand(&[(7, 0), (7, 1), (7, 2), (2, 3), (2, 0)])).rank, HandRank::FullHouse);
        assert_eq!(evaluate5(&hand(&[(2, 0), (5, 0), (9, 0), (11, 0), (13, 0)])).rank, HandRank::Flush);
        assert_eq!(evaluate5(&hand(&[(5, 0), (6, 1), (7, 2), (8, 3), (9, 0)])).rank, HandRank::Straight);
        assert_eq!(evaluate5(&hand(&[(7, 0), (7, 1), (7, 2), (4, 3), (2, 0)])).rank, HandRank::Trips);
        assert_eq!(evaluate5(&hand(&[(7, 0), (7, 1), (4, 2), (4, 3), (2, 0)])).rank, HandRank::TwoPair);
        assert_eq!(evaluate5(&hand(&[(7, 0), (7, 1), (9, 2), (4, 3), (2, 0)])).rank, HandRank::Pair);
        assert_eq!(evaluate5(&hand(&[(3, 0), (7, 1), (9, 2), (11, 3), (2, 0)])).rank, HandRank::HighCard);
    }

    #[test]
    fn the_wheel_is_a_straight_counted_by_its_five() {
        let s = evaluate5(&hand(&[(1, 0), (2, 1), (3, 2), (4, 3), (5, 0)]));
        assert_eq!(s.rank, HandRank::Straight);
        assert_eq!(s.kickers, vec![5]);
        // And a wheel in one suit is the lowest straight flush, not a royal.
        let s = evaluate5(&hand(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]));
        assert_eq!(s.rank, HandRank::StraightFlush);
        assert_eq!(s.kickers, vec![5]);
        assert!(!is_royal(&hand(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)])));
    }

    #[test]
    fn only_ten_through_ace_is_royal() {
        assert!(is_royal(&hand(&[(10, 1), (11, 1), (12, 1), (13, 1), (1, 1)])));
        assert!(!is_royal(&hand(&[(9, 1), (10, 1), (11, 1), (12, 1), (13, 1)])));
    }

    #[test]
    fn stronger_hands_compare_greater() {
        let quads = evaluate5(&hand(&[(7, 0), (7, 1), (7, 2), (7, 3), (2, 0)]));
        let boat = evaluate5(&hand(&[(7, 0), (7, 1), (7, 2), (2, 3), (2, 0)]));
        let flush = evaluate5(&hand(&[(2, 0), (5, 0), (9, 0), (11, 0), (13, 0)]));
        assert!(quads > boat);
        assert!(boat > flush);
    }

    #[test]
    fn same_category_breaks_on_the_higher_kicker() {
        let aces = evaluate5(&hand(&[(1, 0), (1, 1), (9, 2), (4, 3), (2, 0)]));
        let kings = evaluate5(&hand(&[(13, 0), (13, 1), (9, 2), (4, 3), (2, 0)]));
        assert!(aces > kings);
        // Same pair, better side card.
        let high_kicker = evaluate5(&hand(&[(7, 0), (7, 1), (13, 2), (4, 3), (2, 0)]));
        let low_kicker = evaluate5(&hand(&[(7, 0), (7, 1), (9, 2), (4, 3), (2, 0)]));
        assert!(high_kicker > low_kicker);
    }

    #[test]
    fn three_card_poker_ranks_a_straight_above_a_flush() {
        let straight = evaluate3(&hand(&[(5, 0), (6, 1), (7, 2)]));
        let flush = evaluate3(&hand(&[(2, 0), (9, 0), (13, 0)]));
        assert_eq!(straight.rank, ThreeRank::Straight);
        assert_eq!(flush.rank, ThreeRank::Flush);
        assert!(straight > flush, "in the three-card game a straight is the harder hand");
    }

    #[test]
    fn three_card_categories_are_identified() {
        assert_eq!(evaluate3(&hand(&[(11, 0), (12, 0), (13, 0)])).rank, ThreeRank::StraightFlush);
        assert_eq!(evaluate3(&hand(&[(7, 0), (7, 1), (7, 2)])).rank, ThreeRank::Trips);
        assert_eq!(evaluate3(&hand(&[(7, 0), (7, 1), (9, 2)])).rank, ThreeRank::Pair);
        assert_eq!(evaluate3(&hand(&[(3, 0), (7, 1), (11, 2)])).rank, ThreeRank::HighCard);
        // A-2-3 is the lowest three-card straight.
        let wheel = evaluate3(&hand(&[(1, 0), (2, 1), (3, 2)]));
        assert_eq!(wheel.rank, ThreeRank::Straight);
        assert_eq!(wheel.kickers, vec![3]);
    }

    #[test]
    fn a_shuffled_deck_still_holds_exactly_one_of_each_card() {
        let mut rng = Rng::from_seed(11);
        let d = deck(&mut rng);
        assert_eq!(d.len(), 52);
        let mut seen = std::collections::HashSet::new();
        for c in &d {
            assert!(seen.insert((c.rank, c.suit)));
        }
    }
}
