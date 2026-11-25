//! Which entrants meet in which seats: every group of `players` entrants, in every seat
//! rotation, once per seed. Two-player matches are therefore each pairing in both seat orders.

use crate::config::Schedule;
use serde::{Deserialize, Serialize};

/// One match to play.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    /// Unique in the arena and stable while names and seeds stay: results are kept by it.
    pub id: String,
    pub seed: u64,
    /// Entrant names in seat order.
    pub seats: Vec<String>,
}

/// Every fixture, in a fixed order: by seed, then group, then rotation.
pub fn fixtures(names: &[String], schedule: &Schedule) -> Vec<Fixture> {
    let groups = combinations(names.len(), schedule.players);
    let mut out = Vec::new();
    for seed in schedule.first_seed..schedule.first_seed + schedule.seeds {
        for group in &groups {
            for turn in 0..group.len() {
                let mut seats = group.clone();
                seats.rotate_left(turn);
                let seats: Vec<String> = seats.iter().map(|&i| names[i].clone()).collect();
                out.push(Fixture {
                    id: format!("{seed}: {}", seats.join(" | ")),
                    seed,
                    seats,
                });
            }
        }
    }
    out
}

/// Every set of `k` indices below `n`, each in increasing order, in lexicographic order.
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    fn extend(
        start: usize,
        n: usize,
        k: usize,
        current: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if current.len() == k {
            out.push(current.clone());
            return;
        }
        for i in start..n {
            current.push(i);
            extend(i + 1, n, k, current, out);
            current.pop();
        }
    }
    let mut out = Vec::new();
    extend(0, n, k, &mut Vec::new(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("E{i}")).collect()
    }

    #[test]
    fn two_player_fixtures_are_each_pairing_in_both_seat_orders_per_seed() {
        let schedule = Schedule {
            seeds: 2,
            first_seed: 10,
            ..Schedule::default()
        };
        let all = fixtures(&names(3), &schedule);
        assert_eq!(all.len(), 2 * 3 * 2);
        assert_eq!(all[0].seats, ["E0", "E1"]);
        assert_eq!(all[1].seats, ["E1", "E0"]);
        assert_eq!(all[0].seed, 10);
        assert_eq!(all.last().unwrap().seed, 11);
        for name in names(3) {
            let first = all.iter().filter(|f| f.seats[0] == name).count();
            assert_eq!(first, 4, "{name} sits first as often as the others");
        }
    }

    #[test]
    fn larger_matches_rotate_every_group() {
        let schedule = Schedule {
            players: 3,
            seeds: 1,
            ..Schedule::default()
        };
        let all = fixtures(&names(4), &schedule);
        assert_eq!(all.len(), 4 * 3);
        assert_eq!(all[1].seats, ["E1", "E2", "E0"]);
    }

    #[test]
    fn ids_are_unique_and_stable() {
        let schedule = Schedule::default();
        let all = fixtures(&names(4), &schedule);
        let mut ids: Vec<_> = all.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert_eq!(fixtures(&names(4), &schedule), all);
        assert_eq!(all[0].id, "1: E0 | E1");
    }
}
