//! Carattere degli NPC: 2–3 tratti di personalità (burbero, allegro,
//! pettegolo…) che danno il tono alle conversazioni ([`crate::dialogue`]).
//!
//! Drawn at birth and never changed, partly correlated with [`Traits`]
//! (bold people are more often cheerful, grumpy or talkative; cautious ones
//! shy; honest ones kind; the less scrupulous gossip and complain more).
//! Children inherit some of their parents' tags.

use std::fmt;

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::ids::NpcId;
use crate::npc::{Sex, Traits};

/// One personality tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Temper {
    /// Grumpy: curt answers, quarrels more easily.
    Burbero,
    /// Cheerful: friendly tone, exclamations.
    Allegro,
    /// Gossip: loves talking about others.
    Pettegolo,
    /// Shy: short answers, sometimes just "…".
    Timido,
    /// Curious: asks questions, likes news.
    Curioso,
    /// Complainer: rations, cold, work, everything.
    Lamentoso,
    /// Kind: soothes quarrels, comforts.
    Gentile,
    /// Talkative: longer conversations.
    Chiacchierone,
}

impl Temper {
    pub const COUNT: usize = 8;
    pub const ALL: [Temper; Self::COUNT] = [
        Temper::Burbero,
        Temper::Allegro,
        Temper::Pettegolo,
        Temper::Timido,
        Temper::Curioso,
        Temper::Lamentoso,
        Temper::Gentile,
        Temper::Chiacchierone,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Lowercase Italian adjective, gendered: "burbera", "allegro"...
    pub fn name(self, sex: Sex) -> &'static str {
        match self {
            Temper::Burbero => sex.pick("burbera", "burbero"),
            Temper::Allegro => sex.pick("allegra", "allegro"),
            Temper::Pettegolo => sex.pick("pettegola", "pettegolo"),
            Temper::Timido => sex.pick("timida", "timido"),
            Temper::Curioso => sex.pick("curiosa", "curioso"),
            Temper::Lamentoso => sex.pick("lamentosa", "lamentoso"),
            Temper::Gentile => "gentile",
            Temper::Chiacchierone => sex.pick("chiacchierona", "chiacchierone"),
        }
    }

    /// Tags that contradict this one (never together in one person).
    fn clashes_with(self, other: Temper) -> bool {
        use Temper::*;
        matches!(
            (self.min(other), self.max(other)),
            (Burbero, Allegro)
                | (Burbero, Gentile)
                | (Allegro, Lamentoso)
                | (Timido, Chiacchierone)
                | (Allegro, Timido)
        )
    }

    /// How likely a person with `traits` is to have this tag (relative weight).
    fn weight(self, t: Traits) -> f32 {
        match self {
            Temper::Burbero => 0.3 + 0.8 * t.boldness,
            Temper::Allegro => 0.6 + 0.6 * t.boldness,
            Temper::Pettegolo => 0.4 + 1.0 * (1.0 - t.honesty),
            Temper::Timido => 0.2 + 1.4 * (1.0 - t.boldness),
            Temper::Curioso => 0.7 + 0.4 * t.boldness,
            Temper::Lamentoso => 0.4 + 0.6 * (1.0 - t.honesty),
            Temper::Gentile => 0.3 + 1.2 * t.honesty,
            Temper::Chiacchierone => 0.4 + 0.8 * t.boldness,
        }
    }
}

/// A set of [`Temper`] tags (2–3 per NPC), as a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Personality(u16);

impl Personality {
    /// Most tags a person has.
    pub const MAX_TAGS: usize = 3;

    pub fn has(self, tag: Temper) -> bool {
        self.0 & (1 << tag.index()) != 0
    }

    pub fn with(self, tag: Temper) -> Personality {
        Personality(self.0 | (1 << tag.index()))
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Tags in [`Temper::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Temper> {
        Temper::ALL.into_iter().filter(move |&t| self.has(t))
    }

    /// Whether `tag` can be added (not there yet, no contradiction, room left).
    fn accepts(self, tag: Temper) -> bool {
        self.len() < Self::MAX_TAGS && !self.has(tag) && !self.iter().any(|t| t.clashes_with(tag))
    }

    /// Adds tags drawn by weight (see [`Temper::weight`]) until there are `n`.
    fn fill(mut self, n: usize, traits: Traits, rng: &mut impl rand::Rng) -> Personality {
        while self.len() < n.min(Self::MAX_TAGS) {
            let weights: Vec<f32> = Temper::ALL
                .iter()
                .map(|&t| {
                    if self.accepts(t) {
                        t.weight(traits)
                    } else {
                        0.0
                    }
                })
                .collect();
            let total: f32 = weights.iter().sum();
            if total <= 0.0 {
                break;
            }
            let mut r = rng.random::<f32>() * total;
            let mut pick = None;
            for (t, w) in Temper::ALL.iter().zip(&weights) {
                if *w > 0.0 {
                    pick = Some(*t);
                    if r < *w {
                        break;
                    }
                    r -= w;
                }
            }
            match pick {
                Some(t) => self = self.with(t),
                None => break,
            }
        }
        self
    }

    /// 2 or 3 random tags, correlated with `traits`.
    pub fn random(traits: Traits, rng: &mut impl rand::Rng) -> Personality {
        let n = if rng.random_bool(0.5) { 2 } else { 3 };
        Personality::default().fill(n, traits, rng)
    }

    /// A child's tags: each parent tag passes with some chance, the rest is
    /// drawn like [`Personality::random`].
    pub fn inherited(
        mother: Personality,
        father: Personality,
        traits: Traits,
        rng: &mut impl rand::Rng,
    ) -> Personality {
        let n = if rng.random_bool(0.5) { 2 } else { 3 };
        let mut p = Personality::default();
        for tag in mother.iter().chain(father.iter()) {
            if rng.random_bool(0.3) && p.len() + 1 < n && p.accepts(tag) {
                p = p.with(tag);
            }
        }
        p.fill(n, traits, rng)
    }

    /// Deterministic tags for NPCs saved before personalities existed.
    pub fn from_id(id: NpcId, traits: Traits) -> Personality {
        let mut rng = ChaCha8Rng::seed_from_u64(0x7E4F_E400 ^ u64::from(id.0));
        Personality::random(traits, &mut rng)
    }

    /// Italian description, e.g. "burbera, curiosa e gentile".
    pub fn describe(self, sex: Sex) -> String {
        let names: Vec<&str> = self.iter().map(|t| t.name(sex)).collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => (*one).to_string(),
            [init @ .., last] => format!("{} e {last}", init.join(", ")),
        }
    }

    /// Affinity between two characters, in `-1..=1`: likes attract (two
    /// gossips, two cheerful people), some clash (grumpy and talkative).
    pub fn compatibility(self, other: Personality) -> f32 {
        use Temper::*;
        let pair =
            |x: Temper, y: Temper| (self.has(x) && other.has(y)) || (self.has(y) && other.has(x));
        let both = |t: Temper| self.has(t) && other.has(t);
        let mut c: f32 = 0.0;
        if both(Pettegolo) {
            c += 0.5;
        }
        if both(Allegro) {
            c += 0.3;
        }
        if both(Lamentoso) {
            c += 0.3;
        }
        if both(Curioso) {
            c += 0.2;
        }
        if both(Burbero) {
            c -= 0.4;
        }
        if pair(Curioso, Chiacchierone) {
            c += 0.3;
        }
        if pair(Timido, Gentile) {
            c += 0.3;
        }
        if pair(Burbero, Chiacchierone) {
            c -= 0.5;
        }
        if pair(Burbero, Allegro) {
            c -= 0.3;
        }
        if pair(Timido, Chiacchierone) {
            c -= 0.2;
        }
        if pair(Lamentoso, Allegro) {
            c -= 0.2;
        }
        if self.has(Gentile) || other.has(Gentile) {
            c += 0.1;
        }
        c.clamp(-1.0, 1.0)
    }
}

impl fmt::Display for Personality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe(Sex::Male))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_personalities_have_two_or_three_compatible_tags() {
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut seen = [0usize; Temper::COUNT];
        for _ in 0..500 {
            let traits = Traits::random(&mut rng);
            let p = Personality::random(traits, &mut rng);
            assert!((2..=3).contains(&p.len()), "{p:?}");
            for a in p.iter() {
                seen[a.index()] += 1;
                for b in p.iter() {
                    assert!(!a.clashes_with(b) || a == b);
                }
            }
        }
        assert!(seen.iter().all(|&n| n > 20), "{seen:?}");
    }

    #[test]
    fn traits_shape_personality() {
        let mut rng = ChaCha8Rng::seed_from_u64(2);
        let count = |traits: Traits, tag: Temper, rng: &mut ChaCha8Rng| {
            (0..400)
                .filter(|_| Personality::random(traits, rng).has(tag))
                .count()
        };
        let bold = Traits {
            honesty: 0.5,
            boldness: 0.95,
        };
        let shy = Traits {
            honesty: 0.5,
            boldness: 0.05,
        };
        assert!(count(shy, Temper::Timido, &mut rng) > 2 * count(bold, Temper::Timido, &mut rng));
    }

    #[test]
    fn describe_and_inherit() {
        let p = Personality::default()
            .with(Temper::Burbero)
            .with(Temper::Curioso)
            .with(Temper::Pettegolo);
        assert_eq!(p.describe(Sex::Female), "burbera, pettegola e curiosa");
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let child = Personality::inherited(p, p, Traits::default(), &mut rng);
        assert!((2..=3).contains(&child.len()));
        assert_eq!(
            Personality::from_id(NpcId(7), Traits::default()),
            Personality::from_id(NpcId(7), Traits::default())
        );
        let gossip = Personality::default().with(Temper::Pettegolo);
        let chatty = Personality::default().with(Temper::Chiacchierone);
        let grumpy = Personality::default().with(Temper::Burbero);
        assert!(gossip.compatibility(gossip) > 0.0);
        assert!(grumpy.compatibility(chatty) < 0.0);
        assert_eq!(grumpy.compatibility(chatty), chatty.compatibility(grumpy));
    }
}
