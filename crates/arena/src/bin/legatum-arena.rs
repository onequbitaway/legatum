//! `legatum-arena`: play an arena of agents and the built-in AI, and rate them.

use clap::{Parser, Subcommand};
use legatum_arena::config::ArenaConfig;
use legatum_arena::leaderboard::{Leaderboard, leaderboard};
use legatum_arena::run::{load_records, run};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "Play and rate an arena of Legatum agents")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Play every fixture without a result yet, then print the standings.
    Run {
        /// The arena file (JSON).
        #[arg(long)]
        config: PathBuf,
        /// The arena folder: results.jsonl, games/ and leaderboard.json.
        #[arg(long)]
        out: PathBuf,
        /// Matches at once.
        #[arg(long, default_value_t = 2)]
        jobs: usize,
    },
    /// Print the standings from the results so far.
    Table {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run { config, out, jobs } => play(&config, &out, jobs),
        Command::Table { config, out } => table(&config, &out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("legatum-arena: {error}");
            ExitCode::FAILURE
        }
    }
}

fn play(config_path: &Path, out: &Path, jobs: usize) -> Result<(), String> {
    let config = ArenaConfig::load(config_path)?;
    let base = config_path.parent().unwrap_or(Path::new("."));
    run(&config, base, out, jobs, |record, done, total| {
        let result = &record.report.result;
        eprintln!(
            "[{done}/{total}] {} -> turn {}, {:?}",
            record.fixture.id, result.turn, result.reason
        );
    })?;
    table(config_path, out)
}

fn table(config_path: &Path, out: &Path) -> Result<(), String> {
    let config = ArenaConfig::load(config_path)?;
    let board = leaderboard(&config, &load_records(out)?);
    let path = out.join("leaderboard.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&board).expect("serializes"),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    print_board(&board);
    Ok(())
}

fn print_board(board: &Leaderboard) {
    println!("{} of {} matches played", board.played, board.scheduled);
    println!(
        "{:>4}  {:<24} {:<16} {:>7} {:>5} {:>9} {:>6} {:>6}",
        "rank", "entrant", "kind", "elo", "games", "W-D-L", "VP", "late"
    );
    for s in &board.standings {
        println!(
            "{:>4}  {:<24} {:<16} {:>7.1} {:>5} {:>9} {:>6.1} {:>6}",
            s.rank,
            s.name,
            s.kind,
            s.elo,
            s.matches,
            format!("{}-{}-{}", s.wins, s.draws, s.losses),
            f64::from(s.mean_vp_tenths) / 10.0,
            s.timeouts
        );
    }
}
