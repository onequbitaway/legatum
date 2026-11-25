//! The host over real sockets: players connect with WebSocket clients and play turns.

use legatum_agent::setup::MatchSetup;
use legatum_host::{Outcome, ServeOptions, serve};
use legatum_wasm::host::SeatCredential;
use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

struct Running {
    url: String,
    credentials: Vec<SeatCredential>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<Outcome, String>>>,
}

impl Running {
    fn start(remote: usize, turn_seconds: Option<u32>, turn_limit: Option<u32>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let mut setup = MatchSetup {
            seed: 3,
            players: 3,
            turn_limit,
            ..MatchSetup::default()
        }
        .to_game_setup()
        .unwrap();
        for p in setup.players.iter_mut().take(remote) {
            p.is_human = true;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let options = ServeOptions {
            setup,
            turn_seconds,
            save_dir: None,
            exit_when_finished: turn_limit.is_some(),
            stop: Arc::clone(&stop),
        };
        let (sender, credentials) = mpsc::channel();
        let thread =
            thread::spawn(move || serve(listener, options, |c| sender.send(c.to_vec()).unwrap()));
        Self {
            url,
            credentials: credentials.recv_timeout(Duration::from_secs(10)).unwrap(),
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Outcome {
        self.thread.take().unwrap().join().unwrap().unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Player {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    game_id: String,
    turn: u64,
}

impl Player {
    fn join(running: &Running, seat: usize) -> (Self, Value) {
        let (socket, _) = tungstenite::connect(&running.url).unwrap();
        let mut player = Self {
            socket,
            game_id: String::new(),
            turn: 0,
        };
        let token = &running.credentials[seat].token;
        player.send(
            json!({"type": "join", "seat": seat, "name": format!("Agent {seat}"), "token": token}),
        );
        let welcome = player.until("welcome");
        player.game_id = welcome["game_id"].as_str().unwrap().into();
        let view = player.until("view");
        player.turn = view["view"]["turn"].as_u64().unwrap();
        (player, welcome)
    }

    fn send(&mut self, value: Value) {
        self.socket.send(Message::text(value.to_string())).unwrap();
    }

    fn until(&mut self, kind: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "no {kind} from the host");
            let Message::Text(text) = self.socket.read().unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == kind {
                if kind == "view" {
                    self.turn = value["view"]["turn"].as_u64().unwrap();
                }
                return value;
            }
            assert_ne!(value["type"], "error", "{value}");
        }
    }

    fn ready(&mut self) {
        let message =
            json!({"type": "ready", "game_id": self.game_id, "turn": self.turn, "ready": true});
        self.send(message);
    }
}

#[test]
fn two_players_join_with_issued_credentials_and_play_turns() {
    let running = Running::start(2, None, None);
    assert_eq!(running.credentials.len(), 2);
    let (mut a, welcome) = Player::join(&running, 0);
    assert!(welcome.get("turn_seconds").is_none());
    let (mut b, _) = Player::join(&running, 1);
    for turn in 2..=4 {
        a.ready();
        b.ready();
        assert_eq!(a.until("turn")["turn"], turn);
        assert_eq!(b.until("turn")["turn"], turn);
        a.until("view");
        b.until("view");
    }
    a.send(json!({"type": "menu", "req_id": 7}));
    let menu = a.until("menu");
    assert_eq!(menu["req_id"], 7);
    assert_eq!(menu["menu"]["turn"], 4);
}

#[test]
fn the_turn_clock_moves_a_game_whose_players_never_press_ready() {
    let running = Running::start(1, Some(1), None);
    let (mut a, welcome) = Player::join(&running, 0);
    assert_eq!(welcome["turn_seconds"], 1);
    assert_eq!(a.until("turn")["turn"], 2);
    assert_eq!(a.until("turn")["turn"], 3);
}

#[test]
fn a_wrong_credential_is_refused() {
    let running = Running::start(1, None, None);
    let (mut socket, _) = tungstenite::connect(&running.url).unwrap();
    let join = json!({"type": "join", "seat": 0, "name": "Intruder", "token": "0".repeat(64)});
    socket.send(Message::text(join.to_string())).unwrap();
    let Message::Text(reply) = socket.read().unwrap() else {
        panic!("expected text");
    };
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["code"], "invalid_seat_credential");
}

#[test]
fn only_the_ws_path_is_served() {
    let running = Running::start(1, None, None);
    let wrong = running.url.replace("/ws", "/admin");
    assert!(tungstenite::connect(&wrong).is_err());
}

#[test]
fn the_host_stops_once_the_game_is_finished() {
    let running = Running::start(1, Some(1), Some(3));
    let (mut a, _) = Player::join(&running, 0);
    loop {
        let turn = a.until("turn");
        if turn["finished"] == true {
            break;
        }
    }
    let outcome = running.finish();
    assert!(outcome.finished);
}

#[test]
fn the_autosave_is_an_app_save_file() {
    let dir = std::env::temp_dir().join(format!("legatum-host-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut setup = MatchSetup::default().to_game_setup().unwrap();
    setup.players[0].is_human = true;
    setup.players[1].is_human = true;
    let stop = Arc::new(AtomicBool::new(false));
    let options = ServeOptions {
        setup,
        turn_seconds: None,
        save_dir: Some(dir.clone()),
        exit_when_finished: false,
        stop: Arc::clone(&stop),
    };
    let host = thread::spawn(move || serve(listener, options, |_| {}));
    let path = dir.join("game.legatum");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.is_file() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::SeqCst);
    host.join().unwrap().unwrap();
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["format"], "legatum-save");
    assert_eq!(file["online"], true);
    assert!(file["game"].as_str().unwrap().starts_with('{'));
    std::fs::remove_dir_all(&dir).unwrap();
}
