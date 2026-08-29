//! Dice rolling primitives and a tiny `NdM+K` notation parser.

use crate::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notation {
    pub count: u32,
    pub sides: u32,
    pub modifier: i64,
}

impl Notation {
    /// Parses forms like `3d6`, `d20`, `2d10+5`, `4d6 - 1`.
    pub fn parse(input: &str) -> Result<Notation, String> {
        let s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        let s = s.to_lowercase();
        if s.is_empty() {
            return Err("empty expression".into());
        }
        let (dice_part, modifier) = match s.find(['+', '-']) {
            Some(i) if i > 0 => {
                let (a, b) = s.split_at(i);
                let m: i64 = b.parse().map_err(|_| format!("bad modifier '{b}'"))?;
                (a.to_string(), m)
            }
            _ => (s.clone(), 0),
        };
        let mut parts = dice_part.split('d');
        let head = parts.next().unwrap_or("");
        let tail = parts.next().ok_or("expected 'd', e.g. 3d6")?;
        if parts.next().is_some() {
            return Err("only one 'd' allowed".into());
        }
        let count: u32 = if head.is_empty() { 1 } else { head.parse().map_err(|_| format!("bad dice count '{head}'"))? };
        let sides: u32 = tail.parse().map_err(|_| format!("bad die size '{tail}'"))?;
        if count == 0 || count > 100 {
            return Err("dice count must be 1..=100".into());
        }
        if !(2..=1000).contains(&sides) {
            return Err("die must have 2..=1000 sides".into());
        }
        Ok(Notation { count, sides, modifier })
    }
}

pub fn roll_n(rng: &mut Rng, count: u32, sides: u32) -> Vec<u32> {
    (0..count).map(|_| rng.roll(sides)).collect()
}

pub fn total(values: &[u32], modifier: i64) -> i64 {
    values.iter().map(|v| *v as i64).sum::<i64>() + modifier
}

/// Counts of each face value, indexed 1..=sides (index 0 unused).
pub fn tally(values: &[u32], sides: u32) -> Vec<u32> {
    let mut t = vec![0u32; sides as usize + 1];
    for v in values {
        if let Some(slot) = t.get_mut(*v as usize) {
            *slot += 1;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn parses_notation_forms() {
        assert_eq!(Notation::parse("3d6").unwrap(), Notation { count: 3, sides: 6, modifier: 0 });
        assert_eq!(Notation::parse("d20").unwrap(), Notation { count: 1, sides: 20, modifier: 0 });
        assert_eq!(Notation::parse("2d10+5").unwrap(), Notation { count: 2, sides: 10, modifier: 5 });
        assert_eq!(Notation::parse(" 4D6 - 1 ").unwrap(), Notation { count: 4, sides: 6, modifier: -1 });
    }

    #[test]
    fn rejects_nonsense() {
        for bad in ["", "d", "0d6", "3d1", "3x6", "3d6d6", "101d6", "3d6+x"] {
            assert!(Notation::parse(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn rolls_stay_in_range_and_cover_every_face() {
        let mut rng = Rng::from_seed(7);
        let values = roll_n(&mut rng, 2000, 6);
        assert!(values.iter().all(|v| (1..=6).contains(v)));
        let t = tally(&values, 6);
        assert!(t[1..].iter().all(|c| *c > 200), "faces look skewed: {t:?}");
    }

    #[test]
    fn seeded_rng_is_reproducible() {
        let a = roll_n(&mut Rng::from_seed(42), 50, 20);
        let b = roll_n(&mut Rng::from_seed(42), 50, 20);
        assert_eq!(a, b);
        assert_ne!(a, roll_n(&mut Rng::from_seed(43), 50, 20));
    }
}
