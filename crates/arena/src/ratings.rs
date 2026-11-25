//! Elo ratings for matches of two or more seats.
//!
//! A match with N seats counts as every pair of seats playing each other once: the better
//! placed seat of a pair wins it, equal places draw. Each pair moves the two ratings by
//! `K / (N - 1)`, so one match moves a rating by at most `K` whatever its size. The total of all
//! ratings never changes. Results are applied in the order given, so the same results in the
//! same order always give the same ratings.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The rating every entrant starts with.
pub const START: f64 = 1500.0;
/// The most one match can move a rating.
pub const K: f64 = 32.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    pub elo: f64,
    pub matches: u32,
    /// Matches in which the entrant had the best place alone, shared it, or had a worse one.
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
}

impl Default for Rating {
    fn default() -> Self {
        Self {
            elo: START,
            matches: 0,
            wins: 0,
            draws: 0,
            losses: 0,
        }
    }
}

/// One seat's place in a match: lower is better, equal places are a tie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub entrant: String,
    pub place: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ratings {
    pub entrants: BTreeMap<String, Rating>,
}

/// The score `a` expects against `b`: 0.5 when equal, near 1 when `a` is far stronger.
pub fn expected(a: f64, b: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((b - a) / 400.0))
}

impl Ratings {
    /// Applies one match. Seats of the same entrant do not rate each other. A match with fewer
    /// than two entrants changes nothing.
    pub fn apply(&mut self, placements: &[Placement]) {
        let seats = placements.len();
        if seats < 2 {
            return;
        }
        let before: Vec<f64> = placements
            .iter()
            .map(|p| self.entrants.get(&p.entrant).map_or(START, |r| r.elo))
            .collect();
        let step = K / (seats - 1) as f64;
        let mut change = vec![0.0; seats];
        for i in 0..seats {
            for j in (i + 1)..seats {
                if placements[i].entrant == placements[j].entrant {
                    continue;
                }
                let score = match placements[i].place.cmp(&placements[j].place) {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => 0.5,
                    std::cmp::Ordering::Greater => 0.0,
                };
                let delta = step * (score - expected(before[i], before[j]));
                change[i] += delta;
                change[j] -= delta;
            }
        }
        let best = placements.iter().map(|p| p.place).min().unwrap_or(0);
        let sharing = placements.iter().filter(|p| p.place == best).count();
        for (placement, change) in placements.iter().zip(change) {
            let rating = self.entrants.entry(placement.entrant.clone()).or_default();
            rating.elo += change;
            rating.matches += 1;
            match (placement.place == best, sharing) {
                (true, 1) => rating.wins += 1,
                (true, _) => rating.draws += 1,
                (false, _) => rating.losses += 1,
            }
        }
    }

    /// Entrants by rating, best first; equal ratings by name.
    pub fn table(&self) -> Vec<(&str, &Rating)> {
        let mut rows: Vec<_> = self
            .entrants
            .iter()
            .map(|(name, rating)| (name.as_str(), rating))
            .collect();
        rows.sort_by(|a, b| b.1.elo.total_cmp(&a.1.elo).then_with(|| a.0.cmp(b.0)));
        rows
    }
}
