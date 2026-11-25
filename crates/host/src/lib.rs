//! `legatum-host`: the online host with no browser. The authoritative game is the same engine
//! host the browser tab runs (`legatum_wasm::host::net::NetHost`); this crate only adds a
//! WebSocket transport, a clock and files.
//!
//! One thread owns the `NetHost` and does every game step. Each connection has its own thread
//! that reads frames and writes what the host thread queues for it. No game rule is here.

use legatum_core::GameSetup;
use legatum_wasm::host::SeatCredential;
use legatum_wasm::host::net::{MAX_GUEST_MESSAGE, NetHost};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// The path players connect to, as on the relay (`/rooms/<id>/ws`) and the old LAN server.
pub const WS_PATH: &str = "/ws";
/// How often the host thread ticks the engine (join deadlines, silence, the turn clock).
const TICK: Duration = Duration::from_millis(200);
/// How long a connection thread waits for a frame before it writes what is queued.
const POLL: Duration = Duration::from_millis(20);
/// Time to deliver the last messages after the game ends, before the host stops.
const FINISH_GRACE: Duration = Duration::from_secs(2);
/// Messages queued for one connection's thread. The engine already bounds each connection's
/// queue (16 deliveries, 8 MiB); a connection whose thread falls this far behind is closed as
/// a slow reader, as the engine closes one.
const WRITER_QUEUE: usize = 64;
/// A write that takes longer than this ends the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ServeOptions {
    /// The game. Its human seats are the seats players connect to; the rest are the built-in AI.
    pub setup: GameSetup,
    /// Resolve each planning turn this many seconds after it began, Ready or not.
    pub turn_seconds: Option<u32>,
    /// Write the autosave (`game.legatum`) and the game history (`history.json`) here.
    pub save_dir: Option<PathBuf>,
    /// Stop once the game is finished and the last messages are sent.
    pub exit_when_finished: bool,
    /// Set it to stop the host from another thread.
    pub stop: Arc<AtomicBool>,
}

/// How the host stopped.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub finished: bool,
    pub turn: u32,
}

/// What a connection thread tells the host thread.
enum Event {
    Open {
        outgoing: SyncSender<Outgoing>,
        reply: Sender<Result<u64, String>>,
    },
    Text(u64, String),
    Closed(u64),
}

/// What the host thread tells a connection thread.
enum Outgoing {
    Text(String),
    Close,
}

/// Serves one game on `listener` until it finishes (with `exit_when_finished`) or `stop` is set.
/// `on_ready` gets every human seat's credential once the game exists, before any player can
/// connect: hand each one to whoever plays that seat.
pub fn serve(
    listener: TcpListener,
    options: ServeOptions,
    on_ready: impl FnOnce(&[SeatCredential]),
) -> Result<Outcome, String> {
    let mut host = NetHost::new(None)?;
    host.new_game(&legatum_wasm::json::setup_to_value(&options.setup).to_string())?;
    host.set_turn_clock(options.turn_seconds)?;
    let credentials = host.issue_seat_credentials()?;
    if let Some(dir) = &options.save_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    on_ready(&credentials);

    let (events, inbox) = mpsc::channel();
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot poll the listener: {e}"))?;
    let accepting = Arc::clone(&options.stop);
    let accept_events = events.clone();
    let acceptor = thread::spawn(move || accept_loop(listener, accept_events, accepting));
    drop(events);
    let outcome = host_loop(&mut host, &inbox, &options);
    options.stop.store(true, Ordering::SeqCst);
    let _ = acceptor.join();
    Ok(outcome)
}

fn host_loop(host: &mut NetHost, inbox: &Receiver<Event>, options: &ServeOptions) -> Outcome {
    let start = Instant::now();
    let now = || start.elapsed().as_secs_f64() * 1000.0;
    let mut writers: BTreeMap<u64, SyncSender<Outgoing>> = BTreeMap::new();
    let mut last_tick = Instant::now();
    let mut finished_at: Option<Instant> = None;
    loop {
        if options.stop.load(Ordering::SeqCst)
            || finished_at.is_some_and(|t| t.elapsed() >= FINISH_GRACE)
        {
            break;
        }
        match inbox.recv_timeout(TICK) {
            Ok(Event::Open { outgoing, reply }) => match host.connect(false, now()) {
                Ok(id) => {
                    writers.insert(id, outgoing);
                    let _ = reply.send(Ok(id));
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            },
            Ok(Event::Text(id, text)) => host.receive(id, &text, now()),
            Ok(Event::Closed(id)) => {
                writers.remove(&id);
                host.disconnect(id);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_tick.elapsed() >= TICK {
            host.tick(now());
            last_tick = Instant::now();
        }
        for out in host.drain(now()) {
            let Some(writer) = writers.get(&out.conn) else {
                continue;
            };
            let messages = out.texts.into_iter().map(Outgoing::Text);
            let sent = messages
                .chain(out.close.then_some(Outgoing::Close))
                .try_for_each(|m| writer.try_send(m));
            if out.close || sent.is_err() {
                writers.remove(&out.conn);
            }
            if let Err(TrySendError::Full(_)) = sent {
                eprintln!(
                    "legatum-host: connection {} reads too slowly; closing it",
                    out.conn
                );
                host.disconnect(out.conn);
            }
        }
        keep_files(host, options);
        if options.exit_when_finished && host.finished() && finished_at.is_none() {
            finished_at = Some(Instant::now());
        }
    }
    for writer in writers.values() {
        let _ = writer.try_send(Outgoing::Close);
    }
    Outcome {
        finished: host.finished(),
        turn: host.status(true).turn,
    }
}

/// Writes the autosave and the newest game history, when the host has new ones.
fn keep_files(host: &mut NetHost, options: &ServeOptions) {
    let save = host.take_save();
    let history = host.take_recordings().pop();
    let Some(dir) = &options.save_dir else {
        return;
    };
    for (name, text) in [("game.legatum", save), ("history.json", history)] {
        if let Some(text) = text
            && let Err(e) = write_atomically(&dir.join(name), &text)
        {
            eprintln!("legatum-host: cannot write {name}: {e}");
        }
    }
}

/// Writes through a temporary file, so a reader never sees half a save.
fn write_atomically(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}

fn accept_loop(listener: TcpListener, events: Sender<Event>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let events = events.clone();
                let stop = Arc::clone(&stop);
                thread::spawn(move || connection(stream, events, stop));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                eprintln!("legatum-host: accept failed: {e}");
                thread::sleep(POLL);
            }
        }
    }
}

/// Only `GET /ws` becomes a WebSocket; anything else gets 404.
#[allow(clippy::result_large_err)] // tungstenite's handshake callback fixes this signature
fn only_ws(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.uri().path() == WS_PATH {
        return Ok(response);
    }
    let mut refused = ErrorResponse::new(Some("not found".into()));
    *refused.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
    Err(refused)
}

fn connection(stream: TcpStream, events: Sender<Event>, stop: Arc<AtomicBool>) {
    let _ = stream.set_nonblocking(false);
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_GUEST_MESSAGE))
        .max_frame_size(Some(MAX_GUEST_MESSAGE));
    let Ok(mut socket) = tungstenite::accept_hdr_with_config(stream, only_ws, Some(config)) else {
        return;
    };
    let stream = socket.get_ref();
    if stream.set_read_timeout(Some(POLL)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let (outgoing, queued) = mpsc::sync_channel(WRITER_QUEUE);
    let (reply, id) = mpsc::channel();
    if events.send(Event::Open { outgoing, reply }).is_err() {
        return;
    }
    let id = match id.recv() {
        Ok(Ok(id)) => id,
        Ok(Err(message)) => {
            let _ = socket.send(Message::text(
                serde_json::json!({"type": "error", "message": message}).to_string(),
            ));
            let _ = socket.close(None);
            return;
        }
        Err(_) => return,
    };
    pump(&mut socket, id, &events, &queued, &stop);
    let _ = events.send(Event::Closed(id));
}

/// Reads frames into events and writes queued messages until either side closes.
fn pump(
    socket: &mut WebSocket<TcpStream>,
    id: u64,
    events: &Sender<Event>,
    queued: &Receiver<Outgoing>,
    stop: &AtomicBool,
) {
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = socket.close(None);
            let _ = socket.flush();
            return;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                if events.send(Event::Text(id, text.to_string())).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        loop {
            match queued.try_recv() {
                Ok(Outgoing::Text(text)) => {
                    if socket.send(Message::text(text)).is_err() {
                        return;
                    }
                }
                Ok(Outgoing::Close) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }
}
