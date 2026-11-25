//! The arena's standings: ratings with each entrant's record and behaviour.

use crate::config::ArenaConfig;
use crate::run::{MatchRecord, ratings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub rank: u32,
    pub name: String,
    /// `agent` or the built-in AI's level.
    pub kind: String,
    pub elo: f64,
    pub matches: u32,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    /// Mean victory points at the end of its matches, in tenths.
    pub mean_vp_tenths: u32,
    /// Agent behaviour over all its matches.
    pub timeouts: u32,
    pub errors: u32,
    pub rejected: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leaderboard {
    pub played: usize,
    pub scheduled: usize,
    pub standings: Vec<Standing>,
}

pub fn leaderboard(config: &ArenaConfig, records: &[MatchRecord]) -> Leaderboard {
    let ratings = ratings(config, records);
    let mut behaviour: BTreeMap<&str, (u32, u32, u32, u32, u32)> = BTreeMap::new();
    for record in records {
        for (seat, name) in record.fixture.seats.iter().enumerate() {
            let report = &record.report;
            let entry = behaviour.entry(name.as_str()).or_default();
            let player = report.seats[seat].player;
            entry.0 += report
                .result
                .vp
                .iter()
                .find(|(p, _)| *p == player)
                .map_or(0, |(_, vp)| *vp);
            entry.1 += 1;
            if let Some(agent) = &report.seats[seat].agent {
                entry.2 += agent.timeouts;
                entry.3 += agent.errors;
                entry.4 += agent.rejected;
            }
        }
    }
    let kinds: BTreeMap<&str, String> = config
        .entrants
        .iter()
        .map(|e| {
            let kind = e
                .builtin
                .map_or_else(|| "agent".to_owned(), |level| format!("built-in {level:?}"));
            (e.name.as_str(), kind)
        })
        .collect();
    let standings = ratings
        .table()
        .into_iter()
        .enumerate()
        .map(|(i, (name, rating))| {
            let (vp, seats, timeouts, errors, rejected) =
                behaviour.get(name).copied().unwrap_or_default();
            Standing {
                rank: i as u32 + 1,
                name: name.to_owned(),
                kind: kinds.get(name).cloned().unwrap_or_default(),
                elo: (rating.elo * 10.0).round() / 10.0,
                matches: rating.matches,
                wins: rating.wins,
                draws: rating.draws,
                losses: rating.losses,
                mean_vp_tenths: (vp * 10).checked_div(seats).unwrap_or(0),
                timeouts,
                errors,
                rejected,
            }
        })
        .collect();
    Leaderboard {
        played: records.len(),
        scheduled: crate::schedule::fixtures(&crate::run::names(config), &config.schedule).len(),
        standings,
    }
}
