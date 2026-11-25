//! The arena file: who plays, and how many matches of what kind.

use legatum_agent::process::AgentCommand;
use legatum_agent::protocol::Limits;
use legatum_agent::setup::MapSize;
use legatum_core::AiDifficulty;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArenaConfig {
    pub entrants: Vec<Entrant>,
    #[serde(default)]
    pub schedule: Schedule,
    /// Time each agent gets for each answer.
    #[serde(default)]
    pub limits: Limits,
}

/// One player in the arena: an agent program, or the built-in AI at a level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrant {
    /// Unique, at most 80 characters. Results are kept by name.
    pub name: String,
    /// The program and its arguments, run with no shell. Exactly one of `command` and `builtin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// The agent's working directory, relative to the arena file's folder (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// The built-in AI at this level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<AiDifficulty>,
}

/// How a seat of a match is played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Play {
    Agent(AgentCommand),
    Builtin(AiDifficulty),
}

impl Entrant {
    /// How this entrant plays. The agent runs in `dir` relative to `base` (the arena file's
    /// folder), or in `base` itself. A program path with a `/` is taken relative to that folder
    /// too; a bare name such as `python3` is looked up on the PATH.
    pub fn play(&self, base: &Path) -> Play {
        match (&self.command, self.builtin) {
            (Some(words), _) => {
                let dir = self
                    .dir
                    .as_ref()
                    .map_or_else(|| base.to_owned(), |d| base.join(d));
                let program = if words[0].contains('/') && Path::new(&words[0]).is_relative() {
                    dir.join(&words[0]).to_string_lossy().into_owned()
                } else {
                    words[0].clone()
                };
                Play::Agent(AgentCommand {
                    program,
                    args: words[1..].to_vec(),
                    dir: Some(dir),
                })
            }
            (None, Some(level)) => Play::Builtin(level),
            (None, None) => unreachable!("checked by ArenaConfig::check"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// Seats in each match, each on its own team.
    pub players: usize,
    pub map: MapSize,
    /// Each pairing (or group) is played once per seed, in every seat order.
    pub seeds: u64,
    pub first_seed: u64,
    /// The game's own turn limit, where the team with the most VP wins.
    pub turn_limit: Option<u32>,
    pub barbarians: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            players: 2,
            map: MapSize::Small,
            seeds: 2,
            first_seed: 1,
            turn_limit: Some(150),
            barbarians: true,
        }
    }
}

impl ArenaConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let config: Self =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        config.check()?;
        Ok(config)
    }

    /// Names are unique and clean, each entrant plays one way, and there are enough entrants.
    pub fn check(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        for entrant in &self.entrants {
            let clean = legatum_wasm::host::clean_name(&entrant.name);
            if clean.is_empty() || clean != entrant.name {
                return Err(format!(
                    "entrant name {:?}: use 1 to 80 printable characters with single spaces",
                    entrant.name
                ));
            }
            if !names.insert(&entrant.name) {
                return Err(format!("two entrants are named {:?}", entrant.name));
            }
            match (&entrant.command, entrant.builtin) {
                (Some(words), None) if !words.is_empty() && !words[0].is_empty() => {}
                (Some(_), None) => return Err(format!("{}: the command is empty", entrant.name)),
                (None, Some(_)) => {}
                _ => {
                    return Err(format!(
                        "{}: give exactly one of command and builtin",
                        entrant.name
                    ));
                }
            }
        }
        let players = self.schedule.players;
        if !(2..=8).contains(&players) {
            return Err(format!("a match has 2 to 8 players, not {players}"));
        }
        if self.entrants.len() < players {
            return Err(format!(
                "{} entrants cannot fill {players}-player matches",
                self.entrants.len()
            ));
        }
        if self.schedule.seeds == 0 {
            return Err("the schedule needs at least one seed".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<ArenaConfig, String> {
        let config: ArenaConfig = serde_json::from_str(json).map_err(|e| e.to_string())?;
        config.check().map(|()| config)
    }

    #[test]
    fn a_small_arena_file_parses_with_defaults() {
        let config = parse(
            r#"{"entrants": [
                {"name": "Mine", "command": ["python3", "agent.py"], "dir": "agents"},
                {"name": "Hard AI", "builtin": "Hard"},
                {"name": "Built", "command": ["./bin/agent", "--fast"]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(config.schedule, Schedule::default());
        assert_eq!(config.limits, Limits::default());
        let Play::Agent(command) = config.entrants[0].play(Path::new("/arena")) else {
            panic!("an agent");
        };
        assert_eq!(command.program, "python3");
        assert_eq!(command.dir, Some(PathBuf::from("/arena/agents")));
        assert_eq!(
            config.entrants[1].play(Path::new("/arena")),
            Play::Builtin(AiDifficulty::Hard)
        );
        let Play::Agent(built) = config.entrants[2].play(Path::new("/arena")) else {
            panic!("an agent");
        };
        assert_eq!(built.program, "/arena/./bin/agent");
        assert_eq!(built.dir, Some(PathBuf::from("/arena")));
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        let cases = [
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}, {"name": "A", "builtin": "Easy"}]}"#,
                "two entrants",
            ),
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}, {"name": "B"}]}"#,
                "exactly one",
            ),
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}, {"name": "B", "command": []}]}"#,
                "empty",
            ),
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}, {"name": " B ", "builtin": "Easy"}]}"#,
                "name",
            ),
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}]}"#,
                "cannot fill",
            ),
            (
                r#"{"entrants": [{"name": "A", "builtin": "Hard"}, {"name": "B", "builtin": "Easy"}], "schedule": {"players": 2, "map": "small", "seeds": 0, "first_seed": 1, "turn_limit": null, "barbarians": true}}"#,
                "seed",
            ),
            (r#"{"entrants": [], "extra": 1}"#, "unknown field"),
        ];
        for (json, why) in cases {
            let error = parse(json).unwrap_err();
            assert!(error.contains(why), "{json}: {error}");
        }
    }
}
