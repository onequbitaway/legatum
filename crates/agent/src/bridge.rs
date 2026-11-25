//! The agent bridge: plays one seat of an online game (the browser host, or `legatum-host`) with
//! an agent process. To the host the bridge is a normal player client (`plans/lan-protocol.md`);
//! to the agent it is the game, speaking the line protocol (`protocol`).
//!
//! Each planning turn the bridge asks the host for the seat's order menu, sends the agent the
//! view, the menu, the events of the last turn and the orders the host refused, applies the
//! Ready helpers (`assist`) to the answer, sends the orders, drops any the host refuses, and
//! presses Ready. A turn that the host resolves first (a turn clock) is skipped: an answer for an
//! old turn is never sent.

use crate::assist::ready_orders;
use crate::process::{AgentCommand, AgentError, AgentProcess};
use crate::protocol::{
    EndReason, GameInfo, Limits, MatchResult, PROTOCOL_VERSION, PlayerInfo, Seat, SeatKind, ToAgent,
};
use legatum_core::{Order, OrderError, OrderMenu, PlayerView};
use serde_json::{Value, json};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// The host closes a guest that sends nothing for 120 s; clients ping every 20 s.
const PING_EVERY: Duration = Duration::from_secs(20);
/// How long the socket thread waits for a frame before it writes what is queued.
const POLL: Duration = Duration::from_millis(20);
/// Time the bridge waits for the host's answer to one of its requests.
const HOST_WAIT: Duration = Duration::from_secs(30);
/// Part of a turn clock the agent may use; the rest covers the bridge's own round trips.
const CLOCK_SHARE_PERCENT: u64 = 80;

pub struct BridgeConfig {
    /// `ws://host:port/ws`.
    pub url: String,
    pub seat: usize,
    pub token: String,
    pub agent: AgentCommand,
    /// Time for the agent's answers. Without a turn clock the agent gets `turn_ms`; with one it
    /// gets the smaller of `turn_ms` and 80% of the clock.
    pub limits: Limits,
}

/// What a bridge did, for its caller's log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BridgeReport {
    pub name: String,
    pub turns: u32,
    pub timeouts: u32,
    pub errors: u32,
    pub rejected: u32,
    /// Turns the host resolved before the agent's orders were in.
    pub missed: u32,
    /// None: the connection ended before the game did.
    pub result: Option<MatchResult>,
}

/// The socket, owned by one thread that reads frames, writes queued messages and pings.
struct Link {
    incoming: Receiver<Result<Value, String>>,
    outgoing: Sender<Value>,
}

impl Link {
    fn open(url: &str) -> Result<Self, String> {
        let (socket, _) =
            tungstenite::connect(url).map_err(|e| format!("cannot connect to {url}: {e}"))?;
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream
                .set_read_timeout(Some(POLL))
                .map_err(|e| format!("cannot poll the socket: {e}"))?;
        }
        let (to_bridge, incoming) = mpsc::channel();
        let (outgoing, from_bridge) = mpsc::channel();
        thread::spawn(move || socket_loop(socket, &to_bridge, &from_bridge));
        Ok(Self { incoming, outgoing })
    }

    fn send(&self, message: Value) -> Result<(), String> {
        self.outgoing
            .send(message)
            .map_err(|_| "the connection closed".to_owned())
    }

    /// The next host message, waiting until `deadline`.
    fn next(&self, deadline: Instant) -> Result<Option<Value>, String> {
        match self
            .incoming
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(why)) => Err(why),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("the connection closed".into()),
        }
    }
}

fn socket_loop(
    mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
    incoming: &Sender<Result<Value, String>>,
    outgoing: &Receiver<Value>,
) {
    let mut last_sent = Instant::now();
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let parsed = serde_json::from_str(&text).map_err(|e| format!("bad frame: {e}"));
                if incoming.send(parsed).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) => {
                let _ = incoming.send(Err("the host closed the connection".into()));
                return;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => {
                let _ = incoming.send(Err(format!("connection lost: {e}")));
                return;
            }
        }
        loop {
            match outgoing.try_recv() {
                Ok(message) => {
                    if socket.send(Message::text(message.to_string())).is_err() {
                        return;
                    }
                    last_sent = Instant::now();
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }
        if last_sent.elapsed() >= PING_EVERY {
            if socket
                .send(Message::text(json!({"type": "ping"}).to_string()))
                .is_err()
            {
                return;
            }
            last_sent = Instant::now();
        }
    }
}

/// The bridge's view of the online game.
struct Game {
    link: Link,
    game_id: String,
    view: Option<PlayerView>,
    /// The events of the last `turn` report, for the next `turn` message to the agent.
    events: Vec<Value>,
    finished: Option<Value>,
}

impl Game {
    /// Handles one host message. An `error` is returned as `Err`, except that the bridge's own
    /// stale frames (an answer for a turn the clock already ended) are only logged.
    fn handle(&mut self, message: Value) -> Result<(), String> {
        match message["type"].as_str().unwrap_or_default() {
            "welcome" => {
                self.game_id = message["game_id"].as_str().unwrap_or_default().to_owned();
            }
            "view" => {
                let view = serde_json::from_value(message["view"].clone())
                    .map_err(|e| format!("bad view: {e}"))?;
                self.view = Some(view);
            }
            "turn" => {
                self.events = message["events"].as_array().cloned().unwrap_or_default();
                if message["finished"] == true {
                    self.finished = Some(message);
                }
            }
            "error" => {
                let text = message["message"].as_str().unwrap_or_default();
                if text.contains("different game or planning turn") {
                    eprintln!("bridge: a late frame was refused: {text}");
                } else {
                    return Err(format!("host error: {text}"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn turn(&self) -> Option<u32> {
        self.view.as_ref().map(|v| v.turn)
    }

    /// Waits for a message of `kind` that `accept` takes, handling everything else, until the
    /// planning turn moves on (None) or the host is silent for too long (Err).
    fn wait_for(
        &mut self,
        kind: &str,
        turn: u32,
        accept: impl Fn(&Value) -> bool,
    ) -> Result<Option<Value>, String> {
        let deadline = Instant::now() + HOST_WAIT;
        loop {
            let Some(message) = self.link.next(deadline)? else {
                return Err(format!("no {kind} from the host in time"));
            };
            if message["type"] == kind && accept(&message) {
                return Ok(Some(message));
            }
            self.handle(message)?;
            if self.turn() != Some(turn) || self.finished.is_some() {
                return Ok(None);
            }
        }
    }

    /// Waits for the view of a new planning turn, or the end of the game.
    fn wait_for_new_turn(&mut self, turn: u32) -> Result<(), String> {
        loop {
            if self.turn() != Some(turn) || self.finished.is_some() {
                return Ok(());
            }
            let message = self
                .link
                .next(Instant::now() + Duration::from_secs(3600))?
                .ok_or("the host was silent for an hour")?;
            self.handle(message)?;
        }
    }

    fn frame(&self, kind: &str, turn: u32, extra: Value) -> Value {
        let mut frame = json!({"type": kind, "game_id": self.game_id, "turn": turn});
        if let (Some(frame), Some(extra)) = (frame.as_object_mut(), extra.as_object()) {
            frame.extend(extra.clone());
        }
        frame
    }
}

/// Plays the seat until the game ends or the connection closes.
pub fn run_bridge(config: BridgeConfig) -> Result<BridgeReport, String> {
    let mut report = BridgeReport::default();
    let mut game = Game {
        link: Link::open(&config.url)?,
        game_id: String::new(),
        view: None,
        events: Vec::new(),
        finished: None,
    };
    let join = |name: &str| json!({"type": "join", "seat": config.seat, "name": name, "token": config.token});
    game.link.send(join("Agent"))?;
    let welcome = loop {
        let message = game
            .link
            .next(Instant::now() + HOST_WAIT)?
            .ok_or("no welcome from the host in time")?;
        let welcome = (message["type"] == "welcome").then(|| message.clone());
        game.handle(message)?;
        if let Some(welcome) = welcome {
            break welcome;
        }
    };
    while game.view.is_none() {
        let message = game
            .link
            .next(Instant::now() + HOST_WAIT)?
            .ok_or("no view in time")?;
        game.handle(message)?;
    }
    let limits = agent_limits(config.limits, welcome["turn_seconds"].as_u64());
    let mut agent = AgentProcess::spawn(&config.agent).map_err(|e| e.to_string())?;
    let view = game.view.as_ref().expect("a view arrived");
    let name = agent
        .hello(&hello(view, limits), Duration::from_millis(limits.hello_ms))
        .map_err(|e| format!("the agent did not say hello: {e}"))?;
    report.name = legatum_wasm::host::clean_name(&name);
    if !report.name.is_empty() {
        game.link.send(join(&report.name))?;
    }
    let mut rejected: Vec<OrderError> = Vec::new();
    while game.finished.is_none() {
        let turn = game.turn().expect("a view arrived");
        if !game.view.as_ref().is_some_and(alive) {
            game.wait_for_new_turn(turn)?;
            continue;
        }
        game.link.send(json!({"type": "menu", "req_id": turn}))?;
        let Some(reply) = game.wait_for("menu", turn, |m| m["req_id"] == turn)? else {
            report.missed += 1;
            continue;
        };
        let menu: OrderMenu =
            serde_json::from_value(reply["menu"].clone()).map_err(|e| format!("bad menu: {e}"))?;
        let view = game.view.clone().expect("a view arrived");
        let message = ToAgent::Turn {
            turn,
            view: Box::new(view.clone()),
            menu,
            events: std::mem::take(&mut game.events),
            rejected: std::mem::take(&mut rejected),
        };
        let orders = match agent.turn(&message, turn, Duration::from_millis(limits.turn_ms)) {
            Ok(orders) => {
                report.turns += 1;
                ready_orders(&view, orders)
            }
            Err(error) => {
                match error {
                    AgentError::Timeout => report.timeouts += 1,
                    _ => report.errors += 1,
                }
                eprintln!("bridge: turn {turn}: {error}");
                if agent.is_gone() {
                    return Err(format!("the agent stopped: {error}"));
                }
                Vec::new()
            }
        };
        match send_orders(&mut game, turn, orders)? {
            Some(refused) => {
                report.rejected += refused.len() as u32;
                rejected = refused;
                game.link
                    .send(game.frame("ready", turn, json!({"ready": true})))?;
            }
            None => report.missed += 1,
        }
        game.wait_for_new_turn(turn)?;
    }
    let result = final_result(&mut game);
    agent.finish(
        &ToAgent::End {
            result: result.clone(),
        },
        Duration::from_secs(2),
    );
    report.result = Some(result);
    Ok(report)
}

/// Sends the orders and, if the host refuses some, sends the rest again, so the valid ones
/// count. Returns the refused orders, by index into `orders`, or None if the turn moved on.
fn send_orders(
    game: &mut Game,
    turn: u32,
    orders: Vec<Order>,
) -> Result<Option<Vec<OrderError>>, String> {
    game.link
        .send(game.frame("orders", turn, json!({"orders": orders})))?;
    let Some(reply) = game.wait_for("validation", turn, |_| true)? else {
        return Ok(None);
    };
    let refused: Vec<OrderError> = serde_json::from_value(reply["errors"].clone())
        .map_err(|e| format!("bad validation: {e}"))?;
    if refused.is_empty() {
        return Ok(Some(refused));
    }
    let kept: Vec<Order> = orders
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !refused.iter().any(|e| e.index == *i))
        .map(|(_, o)| o)
        .collect();
    game.link
        .send(game.frame("orders", turn, json!({"orders": kept})))?;
    Ok(game
        .wait_for("validation", turn, |_| true)?
        .map(|_| refused))
}

fn alive(view: &PlayerView) -> bool {
    view.players.iter().any(|p| p.id == view.me && p.alive)
}

/// The agent's limits: `turn_ms` capped at a share of the host's turn clock, if it has one.
fn agent_limits(limits: Limits, turn_seconds: Option<u64>) -> Limits {
    Limits {
        turn_ms: turn_seconds.map_or(limits.turn_ms, |s| {
            limits.turn_ms.min(s * 1000 * CLOCK_SHARE_PERCENT / 100)
        }),
        ..limits
    }
}

fn hello(view: &PlayerView, limits: Limits) -> ToAgent {
    let me = view
        .players
        .iter()
        .find(|p| p.id == view.me)
        .expect("a view lists its own player");
    ToAgent::Hello {
        protocol: PROTOCOL_VERSION,
        rules_version: legatum_core::RULES_VERSION,
        seat: Seat {
            player: view.me,
            team: me.team,
            faction: me.faction,
        },
        game: GameInfo {
            width: view.width,
            height: view.height,
            turn_limit: view.turn_limit,
            players: view
                .players
                .iter()
                .map(|p| PlayerInfo {
                    player: p.id,
                    team: p.team,
                    faction: p.faction,
                    kind: if p.is_human {
                        SeatKind::Remote
                    } else {
                        SeatKind::Ai
                    },
                })
                .collect(),
        },
        limits,
    }
}

/// The result from the final `turn` report and the view that follows it.
fn final_result(game: &mut Game) -> MatchResult {
    let report = game.finished.clone();
    let played = report
        .as_ref()
        .and_then(|t| t["turn"].as_u64())
        .map_or(0, |t| t.saturating_sub(1) as u32);
    // The host sends the final view right after the final report.
    let deadline = Instant::now() + Duration::from_secs(5);
    while game.turn().is_some_and(|t| t <= played) {
        match game.link.next(deadline) {
            Ok(Some(message)) => {
                if game.handle(message).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
    let view = game.view.as_ref();
    let limit = view
        .and_then(|v| v.turn_limit)
        .unwrap_or(legatum_core::data::DEFAULT_TURN_LIMIT);
    MatchResult {
        turn: played,
        winner: report
            .as_ref()
            .and_then(|t| serde_json::from_value(t["winner"].clone()).ok())
            .flatten(),
        vp: view
            .map(|v| v.players.iter().map(|p| (p.id, p.vp)).collect())
            .unwrap_or_default(),
        reason: if report.is_none() {
            EndReason::Stopped
        } else if played >= limit {
            EndReason::TurnLimit
        } else {
            EndReason::Victory
        },
    }
}
