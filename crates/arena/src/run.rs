//! Plays the arena's fixtures and keeps each result as one line of `results.jsonl`. A run that
//! stops can start again: fixtures with a result are not played again.

use crate::config::{ArenaConfig, Play};
use crate::ratings::{Placement, Ratings};
use crate::schedule::{Fixture, fixtures};
use legatum_agent::runner::{MatchConfig, MatchReport, run_match};
use legatum_agent::setup::MatchSetup;
use legatum_core::PlayerId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const RESULTS: &str = "results.jsonl";
pub const GAMES: &str = "games";

/// Stop a match that the game's own limit has not ended after this many turns.
const MAX_TURNS: u32 = 600;

/// One played fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchRecord {
    pub fixture: Fixture,
    /// The save of the last turn, relative to the arena folder.
    pub save: String,
    pub report: MatchReport,
}

impl MatchRecord {
    /// Each seat's place: the winning team first, then by VP. Equal VP with no winner between
    /// them is a shared place.
    pub fn placements(&self) -> Vec<Placement> {
        let result = &self.report.result;
        let key = |seat: usize| {
            let player = &self.report.seats[seat];
            let won = result.winner == Some(player.team);
            let vp = result
                .vp
                .iter()
                .find(|(p, _)| *p == player.player)
                .map_or(0, |(_, vp)| *vp);
            (won, vp)
        };
        (0..self.fixture.seats.len())
            .map(|seat| Placement {
                entrant: self.fixture.seats[seat].clone(),
                place: 1
                    + (0..self.fixture.seats.len())
                        .filter(|&other| key(other) > key(seat))
                        .count() as u32,
            })
            .collect()
    }
}

/// Every record in `dir`, in the order they were written. A torn last line (a run killed
/// while writing) is skipped.
pub fn load_records(dir: &Path) -> Result<Vec<MatchRecord>, String> {
    let path = dir.join(RESULTS);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if let Ok(record) = serde_json::from_str(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Ratings from the records of the current fixtures, applied in fixture order, so they do not
/// depend on which match finished first.
pub fn ratings(config: &ArenaConfig, records: &[MatchRecord]) -> Ratings {
    let by_id: BTreeMap<&str, &MatchRecord> =
        records.iter().map(|r| (r.fixture.id.as_str(), r)).collect();
    let mut ratings = Ratings::default();
    for entrant in &config.entrants {
        ratings.entrants.entry(entrant.name.clone()).or_default();
    }
    for fixture in fixtures(&names(config), &config.schedule) {
        if let Some(record) = by_id.get(fixture.id.as_str()) {
            ratings.apply(&record.placements());
        }
    }
    ratings
}

pub fn names(config: &ArenaConfig) -> Vec<String> {
    config.entrants.iter().map(|e| e.name.clone()).collect()
}

/// Plays every fixture without a record in `dir`, `jobs` at a time. `base` is the folder that
/// agent directories are relative to. `on_record` sees each new record as it is kept.
pub fn run(
    config: &ArenaConfig,
    base: &Path,
    dir: &Path,
    jobs: usize,
    on_record: impl Fn(&MatchRecord, usize, usize) + Sync,
) -> Result<Vec<MatchRecord>, String> {
    std::fs::create_dir_all(dir.join(GAMES)).map_err(|e| format!("{}: {e}", dir.display()))?;
    let done: BTreeSet<String> = load_records(dir)?
        .into_iter()
        .map(|r| r.fixture.id)
        .collect();
    let all = fixtures(&names(config), &config.schedule);
    let todo: Vec<(usize, Fixture)> = all
        .into_iter()
        .enumerate()
        .filter(|(_, f)| !done.contains(&f.id))
        .collect();
    let path = dir.join(RESULTS);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // A run killed while writing leaves a torn last line; end it so the next record starts
    // on a line of its own.
    let torn = std::fs::read(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .last()
        .is_some_and(|b| *b != b'\n');
    if torn {
        writeln!(file).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let results = Mutex::new(file);
    let next = AtomicUsize::new(0);
    let played = Mutex::new(Vec::new());
    let failures = Mutex::new(Vec::new());
    let plays: BTreeMap<&str, Play> = config
        .entrants
        .iter()
        .map(|e| (e.name.as_str(), e.play(base)))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                while let Some((index, fixture)) = todo.get(next.fetch_add(1, Ordering::SeqCst)) {
                    match play(config, &plays, dir, *index, fixture) {
                        Ok(record) => {
                            let line = serde_json::to_string(&record).expect("records serialize");
                            let mut file = results.lock().expect("results lock");
                            if let Err(e) = writeln!(file, "{line}").and_then(|()| file.flush()) {
                                failures
                                    .lock()
                                    .expect("lock")
                                    .push(format!("{}: {e}", path.display()));
                                return;
                            }
                            drop(file);
                            let mut played = played.lock().expect("lock");
                            on_record(&record, played.len() + 1, todo.len());
                            played.push(record);
                        }
                        Err(e) => failures
                            .lock()
                            .expect("lock")
                            .push(format!("{}: {e}", fixture.id)),
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().expect("lock");
    if !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    Ok(played.into_inner().expect("lock"))
}

fn play(
    config: &ArenaConfig,
    plays: &BTreeMap<&str, Play>,
    dir: &Path,
    index: usize,
    fixture: &Fixture,
) -> Result<MatchRecord, String> {
    let schedule = &config.schedule;
    let mut setup = MatchSetup {
        seed: fixture.seed,
        players: fixture.seats.len(),
        map: schedule.map,
        factions: None,
        turn_limit: schedule.turn_limit,
        barbarians: schedule.barbarians,
        ..MatchSetup::default()
    }
    .to_game_setup()?;
    let mut agents = BTreeMap::new();
    for (seat, name) in fixture.seats.iter().enumerate() {
        match &plays[name.as_str()] {
            Play::Agent(command) => {
                agents.insert(PlayerId(seat as u8), command.clone());
            }
            Play::Builtin(level) => setup.players[seat].difficulty = *level,
        }
    }
    let (report, session) = run_match(
        MatchConfig {
            setup,
            agents,
            limits: config.limits,
            max_turns: MAX_TURNS,
        },
        |_| {},
    )?;
    let save = PathBuf::from(GAMES).join(format!("{index:04}.legatum"));
    std::fs::write(dir.join(&save), session.save_json())
        .map_err(|e| format!("{}: {e}", save.display()))?;
    Ok(MatchRecord {
        fixture: fixture.clone(),
        save: save.to_string_lossy().into_owned(),
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use legatum_agent::protocol::{EndReason, MatchResult, SeatKind};
    use legatum_agent::runner::SeatReport;
    use legatum_core::{Faction, TeamId};

    fn record(winner: Option<u8>, vp: &[u32]) -> MatchRecord {
        let seats: Vec<String> = (0..vp.len()).map(|i| format!("E{i}")).collect();
        MatchRecord {
            fixture: Fixture {
                id: "1: test".into(),
                seed: 1,
                seats,
            },
            save: String::new(),
            report: MatchReport {
                seed: 1,
                rules_version: 0,
                protocol: 1,
                result: MatchResult {
                    turn: 10,
                    winner: winner.map(TeamId),
                    vp: vp
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (PlayerId(i as u8), *v))
                        .collect(),
                    reason: EndReason::Victory,
                },
                seats: (0..vp.len())
                    .map(|i| SeatReport {
                        player: PlayerId(i as u8),
                        team: TeamId(i as u8),
                        faction: Faction::Celts,
                        kind: SeatKind::Ai,
                        difficulty: None,
                        agent: None,
                    })
                    .collect(),
                vp_by_turn: Vec::new(),
            },
        }
    }

    fn places(record: &MatchRecord) -> Vec<u32> {
        record.placements().iter().map(|p| p.place).collect()
    }

    #[test]
    fn the_winner_is_first_even_with_fewer_vp_and_the_rest_go_by_vp() {
        assert_eq!(places(&record(Some(2), &[5, 7, 4, 1])), [3, 2, 1, 4]);
    }

    #[test]
    fn equal_vp_without_a_winner_share_a_place() {
        assert_eq!(places(&record(None, &[3, 3, 1])), [1, 1, 3]);
    }
}
