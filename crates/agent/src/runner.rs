//! An offline match: one engine session, the built-in AI for some seats and agent processes for
//! the others. The runner treats an agent seat as a human seat: it sends the seat's view and
//! order menu, takes the answer, drops refused orders and resolves the turn with everyone else's.

use crate::process::{AgentCommand, AgentError, AgentProcess};
use crate::protocol::{
    EndReason, GameInfo, Limits, MatchResult, PROTOCOL_VERSION, PlayerInfo, Seat, SeatKind, ToAgent,
};
use legatum_core::event_visibility::visible_events;
use legatum_core::{
    AiDifficulty, Faction, GameSetup, GameState, Order, OrderError, PlayerId, PlayerView, TeamId,
    order_menu, player_view, scoring, validate_orders,
};
use legatum_wasm::session::{BrainKind, Session};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

/// Time an agent gets to exit after `end` before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

pub struct MatchConfig {
    /// Every seat starts as an AI; the seats in `agents` become agent seats.
    pub setup: GameSetup,
    pub agents: BTreeMap<PlayerId, AgentCommand>,
    pub limits: Limits,
    /// Stop the match after this many turns even if the game goes on (reason `stopped`).
    pub max_turns: u32,
}

/// Everything a match produced, as JSON for reports and the arena.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchReport {
    pub seed: u64,
    pub rules_version: u32,
    pub protocol: u32,
    pub result: MatchResult,
    pub seats: Vec<SeatReport>,
    /// Victory points after each resolved turn, one entry per seat in seat order.
    pub vp_by_turn: Vec<Vec<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatReport {
    pub player: PlayerId,
    pub team: TeamId,
    pub faction: Faction,
    pub kind: SeatKind,
    /// The level of a built-in AI seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<AiDifficulty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSeatReport>,
}

/// How an agent seat behaved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSeatReport {
    pub command: String,
    /// The name from its `hello`, cleaned; empty if it never said hello.
    pub name: String,
    /// Turns it answered in time.
    pub answered: u32,
    pub timeouts: u32,
    /// Answers that broke the protocol.
    pub errors: u32,
    /// Orders the game refused.
    pub rejected: u32,
    /// The planning turn at which it stopped answering for good, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone_turn: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The last lines it wrote to standard error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stderr: Vec<String>,
}

/// One agent seat while the match runs.
struct Agent {
    player: PlayerId,
    process: Option<AgentProcess>,
    report: AgentSeatReport,
    events: Vec<serde_json::Value>,
    rejected: Vec<OrderError>,
}

impl Agent {
    /// Counts a missing answer. An agent that is gone for good is dropped: its seat gives no
    /// more orders, and it is not asked again.
    fn fail(&mut self, turn: u32, error: &AgentError) {
        match error {
            AgentError::Timeout => self.report.timeouts += 1,
            _ => self.report.errors += 1,
        }
        self.report.last_error = Some(error.to_string());
        if let Some(process) = self.process.take_if(|p| p.is_gone()) {
            self.report.stderr = process.stderr_tail();
            self.report.gone_turn = Some(turn);
        }
    }

    /// Ends the agent's process, keeping its standard error for the report.
    fn finish(&mut self, result: &MatchResult) {
        if let Some(process) = self.process.take() {
            self.report.stderr = process.stderr_tail();
            process.finish(
                &ToAgent::End {
                    result: result.clone(),
                },
                EXIT_GRACE,
            );
        }
    }
}

/// Plays one match to its end, or to `max_turns`. `on_turn` sees the state after each turn.
/// Returns the report and the session, whose save and tape the caller may keep.
pub fn run_match(
    config: MatchConfig,
    mut on_turn: impl FnMut(&GameState),
) -> Result<(MatchReport, Session), String> {
    let mut setup = config.setup;
    let seats = setup.players.len();
    for player in config.agents.keys() {
        setup
            .players
            .get_mut(player.0 as usize)
            .ok_or_else(|| format!("no seat {} in a {seats}-player game", player.0))?
            .is_human = true;
    }
    let mut session = Session::try_new(&setup, |_| BrainKind::Scripted)?;
    let info = game_info(&setup, &config.agents);
    let mut agents: Vec<Agent> = config
        .agents
        .iter()
        .map(|(&player, command)| start(player, command, &setup, &info, config.limits))
        .collect();
    let mut views: BTreeMap<PlayerId, PlayerView> = agents
        .iter()
        .map(|a| (a.player, player_view(&session.state, a.player)))
        .collect();
    let mut vp_by_turn = Vec::new();
    let turn_limit = Duration::from_millis(config.limits.turn_ms);
    while !session.state.finished && vp_by_turn.len() < config.max_turns as usize {
        let state = &session.state;
        let turn = state.turn;
        let messages: Vec<Option<ToAgent>> = agents
            .iter_mut()
            .map(|a| {
                (state.player(a.player).alive && a.process.is_some()).then(|| ToAgent::Turn {
                    turn,
                    view: Box::new(views[&a.player].clone()),
                    menu: order_menu(state, a.player),
                    events: std::mem::take(&mut a.events),
                    rejected: std::mem::take(&mut a.rejected),
                })
            })
            .collect();
        let answers = ask_all(&mut agents, &messages, turn, turn_limit);
        let mut orders = session.ai_orders();
        for (agent, answer) in agents.iter_mut().zip(answers) {
            match answer {
                Some(Ok(given)) => {
                    agent.report.answered += 1;
                    let (kept, refused) = keep_valid(&session.state, agent.player, given);
                    agent.report.rejected += refused.len() as u32;
                    agent.rejected = refused;
                    orders.insert(agent.player, kept);
                }
                Some(Err(error)) => agent.fail(turn, &error),
                None => {}
            }
        }
        let events = session.step(&orders);
        for agent in &mut agents {
            let after = player_view(&session.state, agent.player);
            agent.events = visible_events(&events, &views[&agent.player], &after)
                .iter()
                .map(|e| serde_json::to_value(e).expect("events serialize"))
                .collect();
            views.insert(agent.player, after);
        }
        vp_by_turn.push(vp_of(&session.state));
        on_turn(&session.state);
    }
    let result = result_of(&session.state, vp_by_turn.len() as u32);
    for agent in &mut agents {
        agent.finish(&result);
    }
    let mut agents: BTreeMap<PlayerId, AgentSeatReport> =
        agents.into_iter().map(|a| (a.player, a.report)).collect();
    let seats = setup
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let player = PlayerId(i as u8);
            let agent = agents.remove(&player);
            SeatReport {
                player,
                team: p.team,
                faction: p.faction,
                kind: if agent.is_some() {
                    SeatKind::Agent
                } else {
                    SeatKind::Ai
                },
                difficulty: agent.is_none().then_some(p.difficulty),
                agent,
            }
        })
        .collect();
    let report = MatchReport {
        seed: setup.seed,
        rules_version: legatum_core::RULES_VERSION,
        protocol: PROTOCOL_VERSION,
        result,
        seats,
        vp_by_turn,
    };
    Ok((report, session))
}

fn game_info(setup: &GameSetup, agents: &BTreeMap<PlayerId, AgentCommand>) -> GameInfo {
    GameInfo {
        width: setup.width,
        height: setup.height,
        turn_limit: setup.turn_limit,
        players: setup
            .players
            .iter()
            .enumerate()
            .map(|(i, p)| PlayerInfo {
                player: PlayerId(i as u8),
                team: p.team,
                faction: p.faction,
                kind: if agents.contains_key(&PlayerId(i as u8)) {
                    SeatKind::Agent
                } else {
                    SeatKind::Ai
                },
            })
            .collect(),
    }
}

/// Starts one agent and waits for its `hello`. An agent that fails to start or to say hello is
/// stopped, and its seat gives no orders all game.
fn start(
    player: PlayerId,
    command: &AgentCommand,
    setup: &GameSetup,
    info: &GameInfo,
    limits: Limits,
) -> Agent {
    let seat = &setup.players[player.0 as usize];
    let mut agent = Agent {
        player,
        process: None,
        report: AgentSeatReport {
            command: command.to_string(),
            ..AgentSeatReport::default()
        },
        events: Vec::new(),
        rejected: Vec::new(),
    };
    let hello = ToAgent::Hello {
        protocol: PROTOCOL_VERSION,
        rules_version: legatum_core::RULES_VERSION,
        seat: Seat {
            player,
            team: seat.team,
            faction: seat.faction,
        },
        game: info.clone(),
        limits,
    };
    let said = AgentProcess::spawn(command).and_then(|mut process| {
        let name = process.hello(&hello, Duration::from_millis(limits.hello_ms));
        match name {
            Ok(name) => {
                agent.process = Some(process);
                Ok(name)
            }
            Err(error) => {
                agent.report.stderr = process.stderr_tail();
                Err(error)
            }
        }
    });
    match said {
        Ok(name) => agent.report.name = legatum_wasm::host::clean_name(&name),
        Err(error) => {
            agent.report.errors += 1;
            agent.report.last_error = Some(format!("hello: {error}"));
            agent.report.gone_turn = Some(0);
        }
    }
    agent
}

/// Sends each agent its message and waits for all answers at once, so one slow agent costs the
/// others no time. `None` for an agent with no message.
fn ask_all(
    agents: &mut [Agent],
    messages: &[Option<ToAgent>],
    turn: u32,
    limit: Duration,
) -> Vec<Option<Result<Vec<Order>, AgentError>>> {
    thread::scope(|scope| {
        let asks: Vec<_> = agents
            .iter_mut()
            .zip(messages)
            .map(|(agent, message)| {
                let process = agent.process.as_mut()?;
                let message = message.as_ref()?;
                Some(scope.spawn(move || process.turn(message, turn, limit)))
            })
            .collect();
        asks.into_iter()
            .map(|ask| ask.map(|h| h.join().expect("agent threads do not panic")))
            .collect()
    })
}

/// The orders `validate_orders` accepts, and the refused ones by index into `orders`.
fn keep_valid(
    state: &GameState,
    player: PlayerId,
    orders: Vec<Order>,
) -> (Vec<Order>, Vec<OrderError>) {
    let refused = validate_orders(state, player, &orders);
    let kept = orders
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !refused.iter().any(|e| e.index == *i))
        .map(|(_, o)| o)
        .collect();
    (kept, refused)
}

fn vp_of(state: &GameState) -> Vec<u32> {
    state
        .players
        .iter()
        .map(|p| scoring::vp_total(state, p.id))
        .collect()
}

fn result_of(state: &GameState, turns: u32) -> MatchResult {
    let limit = state
        .setup
        .turn_limit
        .unwrap_or(legatum_core::data::DEFAULT_TURN_LIMIT);
    let reason = if !state.finished {
        EndReason::Stopped
    } else if state.turn >= limit {
        EndReason::TurnLimit
    } else {
        EndReason::Victory
    };
    MatchResult {
        turn: turns,
        winner: state.winner,
        vp: state
            .players
            .iter()
            .map(|p| (p.id, scoring::vp_total(state, p.id)))
            .collect(),
        reason,
    }
}
