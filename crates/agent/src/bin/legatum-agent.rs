//! `legatum-agent`: play agent seats from the command line. See `docs/agents/`.

use clap::{Parser, Subcommand};
use legatum_agent::bridge::{BridgeConfig, run_bridge};
use legatum_agent::process::AgentCommand;
use legatum_agent::protocol::{Limits, SeatKind};
use legatum_agent::runner::{MatchConfig, MatchReport, run_match};
use legatum_agent::save_file::app_save_file;
use legatum_agent::setup::{
    MapSize, MatchSetup, parse_difficulty, parse_faction, parse_seat_value,
};
use legatum_core::{AiDifficulty, Faction};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "Play Legatum seats with outside programs (agents)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Play one offline match. Seats without an agent are played by the built-in AI.
    Match(MatchArgs),
    /// Play one seat of an online game (legatum-host, or a browser host) with an agent.
    Connect(ConnectArgs),
}

#[derive(clap::Args)]
struct ConnectArgs {
    /// The agent: a program and its arguments, split on spaces with no quoting.
    #[arg(long)]
    agent: String,
    /// The seat to play.
    #[arg(long)]
    seat: usize,
    /// The credentials file that legatum-host wrote; the bridge takes the seat's URL and
    /// credential from it.
    #[arg(long, conflicts_with_all = ["url", "token"])]
    credentials: Option<PathBuf>,
    /// The host's WebSocket address, for example ws://127.0.0.1:8787/ws.
    #[arg(long, requires = "token")]
    url: Option<String>,
    /// The seat's credential.
    #[arg(long, requires = "url")]
    token: Option<String>,
    /// Milliseconds the agent has to answer hello.
    #[arg(long, default_value_t = Limits::default().hello_ms)]
    hello_ms: u64,
    /// Milliseconds the agent has to answer each turn (at most 80% of a host's turn clock).
    #[arg(long, default_value_t = Limits::default().turn_ms)]
    turn_ms: u64,
}

#[derive(clap::Args)]
struct MatchArgs {
    /// An agent seat: SEAT=COMMAND, for example 0="python3 agent.py". Repeat for more seats.
    /// The command is split on spaces, with no quoting.
    #[arg(long = "agent", value_name = "SEAT=COMMAND", required = true)]
    agents: Vec<String>,
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
    /// Stop the match after this many turns, with no winner.
    #[arg(long, default_value_t = 600)]
    max_turns: u32,
    #[arg(long)]
    no_barbarians: bool,
    /// Milliseconds an agent has to answer hello.
    #[arg(long, default_value_t = Limits::default().hello_ms)]
    hello_ms: u64,
    /// Milliseconds an agent has to answer each turn.
    #[arg(long, default_value_t = Limits::default().turn_ms)]
    turn_ms: u64,
    /// Write report.json, game.legatum (a save the app opens) and tape.json here.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Match(args) => play(args),
        Command::Connect(args) => connect(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("legatum-agent: {error}");
            ExitCode::FAILURE
        }
    }
}

fn play(args: MatchArgs) -> Result<(), String> {
    let mut agents = BTreeMap::new();
    for text in &args.agents {
        let (seat, words) = parse_seat_value(text)?;
        if agents
            .insert(seat, AgentCommand::from_words(&words)?)
            .is_some()
        {
            return Err(format!("seat {} has two agents", seat.0));
        }
    }
    let setup = MatchSetup {
        seed: args.seed,
        players: args.players,
        map: args.map_size,
        factions: args.factions,
        difficulty: args.difficulty,
        turn_limit: args.turn_limit,
        barbarians: !args.no_barbarians,
    }
    .to_game_setup()?;
    let config = MatchConfig {
        setup,
        agents,
        limits: Limits {
            hello_ms: args.hello_ms,
            turn_ms: args.turn_ms,
        },
        max_turns: args.max_turns,
    };
    let (report, session) = run_match(config, |state| {
        if state.turn % 10 == 0 {
            eprintln!("turn {}", state.turn);
        }
    })?;
    print_report(&report);
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let write = |name: &str, text: String| {
            let path = dir.join(name);
            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
        };
        write(
            "report.json",
            serde_json::to_string_pretty(&report).expect("reports serialize"),
        )?;
        write("game.legatum", app_save_file(&session.save_json())?)?;
        if let Some(history) = &session.history {
            write("tape.json", history.tape.to_json())?;
        }
        println!("Wrote {}", dir.display());
    }
    Ok(())
}

fn connect(args: ConnectArgs) -> Result<(), String> {
    let (url, token) = match (&args.credentials, args.url, args.token) {
        (Some(path), _, _) => seat_credential(path, args.seat)?,
        (None, Some(url), Some(token)) => (url, token),
        _ => return Err("give --credentials, or --url and --token".into()),
    };
    let report = run_bridge(BridgeConfig {
        url,
        seat: args.seat,
        token,
        agent: AgentCommand::from_words(&args.agent)?,
        limits: Limits {
            hello_ms: args.hello_ms,
            turn_ms: args.turn_ms,
        },
    })?;
    println!(
        "{}: {} turns answered, {} late, {} errors, {} refused orders, {} turns missed",
        report.name, report.turns, report.timeouts, report.errors, report.rejected, report.missed
    );
    match report.result {
        Some(result) => println!(
            "Game over at turn {}: {} ({:?}).",
            result.turn,
            result
                .winner
                .map_or_else(|| "no team won".to_owned(), |t| format!("team {} won", t.0)),
            result.reason
        ),
        None => println!("The connection ended before the game did."),
    }
    Ok(())
}

/// The URL and credential of `seat` in a legatum-host credentials file.
fn seat_credential(path: &std::path::Path, seat: usize) -> Result<(String, String), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let seats: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let entry = seats
        .iter()
        .find(|s| s["seat"] == seat)
        .ok_or_else(|| format!("seat {seat} is not in {}", path.display()))?;
    match (entry["url"].as_str(), entry["token"].as_str()) {
        (Some(url), Some(token)) => Ok((url.to_owned(), token.to_owned())),
        _ => Err(format!(
            "seat {seat} in {} has no url or token",
            path.display()
        )),
    }
}

fn print_report(report: &MatchReport) {
    let result = &report.result;
    let winner = result
        .winner
        .map_or_else(|| "no team".to_owned(), |t| format!("team {}", t.0));
    println!("Turn {}: {winner} won ({:?}).", result.turn, result.reason);
    println!(
        "{:<5} {:<12} {:<6} {:>3}  player",
        "seat", "faction", "kind", "vp"
    );
    for seat in &report.seats {
        let vp = result
            .vp
            .iter()
            .find(|(p, _)| *p == seat.player)
            .map_or(0, |(_, vp)| *vp);
        let who = match (&seat.kind, &seat.agent, seat.difficulty) {
            (SeatKind::Agent, Some(a), _) => format!(
                "{} ({} answered, {} late, {} errors, {} refused)",
                if a.name.is_empty() {
                    &a.command
                } else {
                    &a.name
                },
                a.answered,
                a.timeouts,
                a.errors,
                a.rejected
            ),
            (_, _, Some(level)) => format!("built-in AI, {level:?}"),
            _ => String::new(),
        };
        println!(
            "{:<5} {:<12} {:<6} {:>3}  {who}",
            seat.player.0,
            format!("{:?}", seat.faction),
            if seat.kind == SeatKind::Agent {
                "agent"
            } else {
                "ai"
            },
            vp
        );
    }
}
