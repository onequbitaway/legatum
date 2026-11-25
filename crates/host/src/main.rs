//! `legatum-host`: host one online game with no browser. Players and agents connect over
//! WebSocket to `ws://ADDRESS/ws` and speak the game protocol (`plans/lan-protocol.md`).

use clap::Parser;
use legatum_agent::setup::{MapSize, MatchSetup, parse_difficulty, parse_faction};
use legatum_core::{AiDifficulty, Faction};
use legatum_host::{ServeOptions, WS_PATH, serve};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Parser)]
#[command(version, about = "Host one online Legatum game with no browser")]
struct Cli {
    /// The address to listen on. Use 127.0.0.1 to accept only this machine.
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: String,
    /// Seats played over the network: seats 0 to N-1. The other seats are the built-in AI.
    #[arg(long, default_value_t = 2)]
    remote: usize,
    /// Empires in the game, each on its own team.
    #[arg(long, default_value_t = 2)]
    players: usize,
    #[arg(long, default_value = "small")]
    map_size: MapSize,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Factions in seat order, comma-separated (a seeded draw when omitted).
    #[arg(long, value_delimiter = ',', value_parser = parse_faction)]
    factions: Option<Vec<Faction>>,
    /// Level of the built-in AI seats: easy, normal or hard.
    #[arg(long, default_value = "normal", value_parser = parse_difficulty)]
    difficulty: AiDifficulty,
    /// The game's turn limit: the team with the most VP wins there.
    #[arg(long)]
    turn_limit: Option<u32>,
    #[arg(long)]
    no_barbarians: bool,
    /// Resolve each turn this many seconds after it began, Ready or not.
    #[arg(long)]
    turn_seconds: Option<u32>,
    /// Write each seat's credential here as JSON (readable only by you). Give each one to the
    /// player of that seat. Without it the credentials go to standard output.
    #[arg(long)]
    credentials: Option<PathBuf>,
    /// Keep the autosave (game.legatum) and the game history (history.json) here.
    #[arg(long)]
    save_dir: Option<PathBuf>,
    /// Stop when the game is finished.
    #[arg(long)]
    exit_when_finished: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("legatum-host: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    if cli.remote == 0 || cli.remote > cli.players {
        return Err(format!(
            "--remote must be 1 to {} (the number of players)",
            cli.players
        ));
    }
    let mut setup = MatchSetup {
        seed: cli.seed,
        players: cli.players,
        map: cli.map_size,
        factions: cli.factions,
        difficulty: cli.difficulty,
        turn_limit: cli.turn_limit,
        barbarians: !cli.no_barbarians,
    }
    .to_game_setup()?;
    for player in setup.players.iter_mut().take(cli.remote) {
        player.is_human = true;
    }
    let listener = TcpListener::bind(&cli.listen)
        .map_err(|e| format!("cannot listen on {}: {e}", cli.listen))?;
    let address = listener
        .local_addr()
        .map_err(|e| format!("cannot read the address: {e}"))?;
    let url = format!("ws://{address}{WS_PATH}");
    let options = ServeOptions {
        setup,
        turn_seconds: cli.turn_seconds,
        save_dir: cli.save_dir,
        exit_when_finished: cli.exit_when_finished,
        stop: Arc::new(AtomicBool::new(false)),
    };
    let credentials_file = cli.credentials;
    let mut handed_out = Ok(());
    let outcome = serve(listener, options, |credentials| {
        let seats: Vec<_> = credentials
            .iter()
            .map(|c| serde_json::json!({"url": url, "seat": c.seat, "player": c.player, "token": c.token}))
            .collect();
        let json = serde_json::to_string_pretty(&seats).expect("credentials serialize");
        handed_out = match &credentials_file {
            Some(path) => write_private(path, &json),
            None => {
                println!("{json}");
                Ok(())
            }
        };
        eprintln!("legatum-host: {} seats wait at {url}", credentials.len());
    })?;
    handed_out?;
    eprintln!(
        "legatum-host: stopped at turn {}{}",
        outcome.turn,
        if outcome.finished { ", game over" } else { "" }
    );
    Ok(())
}

/// Writes a file only its owner can read: it holds seat credentials.
fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path).and_then(|mut f| {
        #[cfg(unix)]
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        f.write_all(text.as_bytes())
    });
    file.map_err(|e| format!("{}: {e}", path.display()))
}
