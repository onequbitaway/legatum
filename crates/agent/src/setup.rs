//! A match's `GameSetup` from a few choices, for the command line and the arena.

use legatum_core::rng::{Rng, purpose};
use legatum_core::{AiDifficulty, Faction, GameSetup, PlayerId};

/// The map presets of the simulation runner (`legatum-sim --map-size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl MapSize {
    pub fn dimensions(self) -> (i32, i32) {
        match self {
            Self::Small => (40, 26),
            Self::Medium => (48, 32),
            Self::Large => (60, 40),
        }
    }
}

impl std::str::FromStr for MapSize {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            _ => Err(format!(
                "unknown map size {value}; expected small, medium or large"
            )),
        }
    }
}

/// What a match needs to decide before the first turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchSetup {
    pub seed: u64,
    /// 2 to 8 empires, each on its own team.
    pub players: usize,
    pub map: MapSize,
    /// Factions in seat order; a seeded draw when None.
    pub factions: Option<Vec<Faction>>,
    /// The level of the built-in AI seats.
    pub difficulty: AiDifficulty,
    /// The game's own turn limit (the team with the most VP wins there). None: the default limit.
    pub turn_limit: Option<u32>,
    pub barbarians: bool,
}

impl Default for MatchSetup {
    fn default() -> Self {
        Self {
            seed: 1,
            players: 2,
            map: MapSize::Small,
            factions: None,
            difficulty: AiDifficulty::Normal,
            turn_limit: None,
            barbarians: true,
        }
    }
}

impl MatchSetup {
    /// The game setup with every seat an AI; the runner turns agent seats into human seats.
    pub fn to_game_setup(&self) -> Result<GameSetup, String> {
        if !(2..=8).contains(&self.players) {
            return Err(format!("a match has 2 to 8 players, not {}", self.players));
        }
        let factions = match &self.factions {
            Some(f) if f.len() != self.players => {
                return Err(format!("{} factions for {} players", f.len(), self.players));
            }
            Some(f) => f.clone(),
            None => seeded_factions(self.seed, self.players),
        };
        let mut setup = GameSetup::all_ai(self.seed);
        setup.players.truncate(self.players);
        for (player, faction) in setup.players.iter_mut().zip(factions) {
            player.faction = faction;
            player.difficulty = self.difficulty;
        }
        (setup.width, setup.height) = self.map.dimensions();
        setup.turn_limit = self.turn_limit;
        setup.barbarians = self.barbarians;
        legatum_core::validation::check_setup(&setup)?;
        Ok(setup)
    }
}

/// Distinct factions for `players` seats, drawn from every faction by the seed alone.
pub fn seeded_factions(seed: u64, players: usize) -> Vec<Faction> {
    let mut pool = Faction::ALL.to_vec();
    let mut rng = Rng::derive(seed, &[purpose::SETUP, 0xA6E7]);
    for i in (1..pool.len()).rev() {
        let j = rng.range(0, i as i64) as usize;
        pool.swap(i, j);
    }
    pool.truncate(players);
    pool
}

/// A faction by its name in saves (`Sparta`, `Celts`, ...), in any case.
pub fn parse_faction(value: &str) -> Result<Faction, String> {
    Faction::ALL
        .into_iter()
        .find(|f| format!("{f:?}").eq_ignore_ascii_case(value))
        .ok_or_else(|| {
            format!(
                "unknown faction {value}; expected one of {:?}",
                Faction::ALL
            )
        })
}

/// `easy`, `normal` or `hard`, in any case.
pub fn parse_difficulty(value: &str) -> Result<AiDifficulty, String> {
    match value.to_ascii_lowercase().as_str() {
        "easy" => Ok(AiDifficulty::Easy),
        "normal" => Ok(AiDifficulty::Normal),
        "hard" => Ok(AiDifficulty::Hard),
        _ => Err(format!(
            "unknown level {value}; expected easy, normal or hard"
        )),
    }
}

/// Parses `SEAT=VALUE`, for options such as `--agent 1="python3 agent.py"`.
pub fn parse_seat_value(text: &str) -> Result<(PlayerId, String), String> {
    let (seat, value) = text
        .split_once('=')
        .ok_or_else(|| format!("expected SEAT=VALUE, got {text}"))?;
    let seat: u8 = seat
        .trim()
        .parse()
        .map_err(|_| format!("bad seat number in {text}"))?;
    Ok((PlayerId(seat), value.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_match_is_a_small_two_player_game() {
        let setup = MatchSetup::default().to_game_setup().unwrap();
        assert_eq!(setup.players.len(), 2);
        assert_eq!((setup.width, setup.height), (40, 26));
        assert!(setup.players.iter().all(|p| !p.is_human));
        assert_ne!(setup.players[0].team, setup.players[1].team);
    }

    #[test]
    fn the_faction_draw_depends_on_the_seed_only_and_never_repeats() {
        assert_eq!(seeded_factions(9, 8), seeded_factions(9, 8));
        let mut drawn = seeded_factions(9, 8);
        drawn.sort();
        drawn.dedup();
        assert_eq!(drawn.len(), 8);
        assert!((0..20).any(|seed| seeded_factions(seed, 2) != seeded_factions(9, 2)));
    }

    #[test]
    fn explicit_factions_and_levels_are_kept() {
        let setup = MatchSetup {
            players: 3,
            factions: Some(vec![Faction::Rome, Faction::Egypt, Faction::Celts]),
            difficulty: AiDifficulty::Hard,
            turn_limit: Some(80),
            ..MatchSetup::default()
        }
        .to_game_setup()
        .unwrap();
        let factions: Vec<_> = setup.players.iter().map(|p| p.faction).collect();
        assert_eq!(factions, [Faction::Rome, Faction::Egypt, Faction::Celts]);
        assert!(
            setup
                .players
                .iter()
                .all(|p| p.difficulty == AiDifficulty::Hard)
        );
        assert_eq!(setup.turn_limit, Some(80));
    }

    #[test]
    fn bad_choices_are_refused() {
        let one = MatchSetup {
            players: 1,
            ..MatchSetup::default()
        };
        assert!(one.to_game_setup().is_err());
        let mismatched = MatchSetup {
            factions: Some(vec![Faction::Rome]),
            ..MatchSetup::default()
        };
        assert!(mismatched.to_game_setup().unwrap_err().contains("factions"));
    }

    #[test]
    fn seat_values_parse() {
        assert_eq!(
            parse_seat_value(" 3 = python3 a.py").unwrap(),
            (PlayerId(3), "python3 a.py".into())
        );
        assert!(parse_seat_value("x=y").is_err());
        assert!(parse_seat_value("3").is_err());
        assert_eq!("LARGE".parse::<MapSize>().unwrap(), MapSize::Large);
        assert_eq!(parse_faction("sparta").unwrap(), Faction::Sparta);
        assert!(parse_faction("Atlantis").is_err());
        assert_eq!(parse_difficulty("Hard").unwrap(), AiDifficulty::Hard);
        assert!(parse_difficulty("brutal").is_err());
    }
}
