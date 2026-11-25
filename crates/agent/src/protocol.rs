//! The agent line protocol, version [`PROTOCOL_VERSION`]. The guide is `docs/agents/protocol.md`.
//!
//! Each message is one line of compact JSON with a `type` field. The game sends [`ToAgent`]
//! messages; the agent answers with [`FromAgent`] messages:
//!
//! 1. `hello` (game) → `hello` (agent): the seat and the match, then the agent's name.
//! 2. `turn` (game) → `orders` (agent): once per planning turn, until the game ends.
//! 3. `end` (game): the result. The agent then exits; it sends nothing.

use legatum_core::{Faction, Order, OrderError, OrderMenu, PlayerId, PlayerView, TeamId};
use serde::{Deserialize, Serialize};

/// Bumped on any change an agent built for the previous version could misread.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest line the game reads from an agent. A full turn of orders is far smaller.
pub const MAX_AGENT_LINE: usize = 1024 * 1024;

/// Most orders one agent message may hold, as the online host allows per seat and turn.
pub const MAX_ORDERS: usize = 2000;

/// A message from the game to the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToAgent {
    /// The first message. Answer with [`FromAgent::Hello`].
    Hello {
        protocol: u32,
        rules_version: u32,
        seat: Seat,
        game: GameInfo,
        limits: Limits,
    },
    /// A planning turn. Answer with [`FromAgent::Orders`] for the same `turn`.
    Turn {
        turn: u32,
        view: Box<PlayerView>,
        menu: OrderMenu,
        /// What the seat's team saw in the turn just resolved (empty on the first turn). Each
        /// event is an object with a `type` field, as in the online host's `turn` message.
        events: Vec<serde_json::Value>,
        /// The orders of the previous answer that were refused, by index into that answer.
        /// Refused orders are dropped; the others are played.
        rejected: Vec<OrderError>,
    },
    /// The match is over. Exit after reading it. An eliminated seat gets no more turns, and gets
    /// this message when the match ends.
    End { result: MatchResult },
}

/// A message from the agent to the game.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromAgent {
    /// The answer to [`ToAgent::Hello`].
    Hello {
        /// A short display name for reports. The game removes control characters and keeps at
        /// most 80 characters.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// The answer to [`ToAgent::Turn`]. Send an empty list to give no new orders.
    Orders { turn: u32, orders: Vec<Order> },
}

/// The seat the agent plays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    pub player: PlayerId,
    pub team: TeamId,
    pub faction: Faction,
}

/// Public facts about the match, the same for every seat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInfo {
    pub width: i32,
    pub height: i32,
    /// None: the game runs until a team wins.
    pub turn_limit: Option<u32>,
    pub players: Vec<PlayerInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub player: PlayerId,
    pub team: TeamId,
    pub faction: Faction,
    /// `agent` for a seat played by a program, `ai` for the built-in AI, `remote` for an online
    /// seat that a person or a program plays (the host cannot tell which).
    pub kind: SeatKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Agent,
    Ai,
    Remote,
}

/// Time the game waits for each answer. A late or bad answer counts as no orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub hello_ms: u64,
    pub turn_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            hello_ms: 10_000,
            turn_ms: 30_000,
        }
    }
}

/// How the match ended for everyone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResult {
    /// The last turn played.
    pub turn: u32,
    /// None: no team won (a turn limit with a tie, or the game was stopped).
    pub winner: Option<TeamId>,
    /// Every seat's victory points at the end, in seat order.
    pub vp: Vec<(PlayerId, u32)>,
    /// Why the match ended: `victory`, `turn_limit` or `stopped`.
    pub reason: EndReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Victory,
    TurnLimit,
    Stopped,
}

/// One message as one line, without the newline. Compact JSON never holds a raw newline.
pub fn encode<T: Serialize>(message: &T) -> String {
    serde_json::to_string(message).expect("protocol messages serialize")
}

/// Reads one agent line. Refuses lines over [`MAX_AGENT_LINE`] and answers over [`MAX_ORDERS`].
pub fn decode_from_agent(line: &str) -> Result<FromAgent, String> {
    if line.len() > MAX_AGENT_LINE {
        return Err(format!("line over {MAX_AGENT_LINE} bytes"));
    }
    let message: FromAgent =
        serde_json::from_str(line.trim_end()).map_err(|e| format!("bad message: {e}"))?;
    if let FromAgent::Orders { orders, .. } = &message
        && orders.len() > MAX_ORDERS
    {
        return Err(format!("{} orders (at most {MAX_ORDERS})", orders.len()));
    }
    Ok(message)
}
