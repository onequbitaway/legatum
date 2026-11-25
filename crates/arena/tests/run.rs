//! A whole small arena with `sh` agents and the built-in AI. Unix only.
#![cfg(unix)]

use legatum_agent::protocol::Limits;
use legatum_agent::setup::MapSize;
use legatum_arena::config::{ArenaConfig, Entrant, Schedule};
use legatum_arena::leaderboard::leaderboard;
use legatum_arena::run::{RESULTS, load_records, run};
use legatum_core::AiDifficulty;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const ECHO: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"hello"'*) echo '{"type":"hello","name":"echo"}' ;;
    *'"type":"turn"'*)
      t=$(printf '%s' "$line" | sed 's/^{"type":"turn","turn":\([0-9]*\).*/\1/')
      echo "{\"type\":\"orders\",\"turn\":$t,\"orders\":[]}" ;;
    *'"type":"end"'*) exit 0 ;;
  esac
done
"#;

fn agent(name: &str) -> Entrant {
    Entrant {
        name: name.into(),
        command: Some(vec!["sh".into(), "-c".into(), ECHO.into()]),
        dir: None,
        builtin: None,
    }
}

fn config() -> ArenaConfig {
    ArenaConfig {
        entrants: vec![
            agent("Echo A"),
            agent("Echo B"),
            Entrant {
                name: "Easy AI".into(),
                command: None,
                dir: None,
                builtin: Some(AiDifficulty::Easy),
            },
        ],
        schedule: Schedule {
            players: 2,
            map: MapSize::Small,
            seeds: 1,
            first_seed: 5,
            turn_limit: Some(4),
            barbarians: false,
        },
        limits: Limits {
            hello_ms: 5_000,
            turn_ms: 5_000,
        },
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("legatum-arena-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn an_arena_plays_every_fixture_once_and_resumes_without_replaying() {
    let dir = scratch("resume");
    let config = config();
    let seen = AtomicUsize::new(0);
    let records = run(&config, Path::new("."), &dir, 3, |_, _, total| {
        assert_eq!(total, 6);
        seen.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    assert_eq!(records.len(), 6);
    assert_eq!(seen.load(Ordering::SeqCst), 6);
    for record in &records {
        let save = std::fs::read_to_string(dir.join(&record.save)).unwrap();
        assert!(
            save.contains(r#""format":"legatum-save""#),
            "an app save file"
        );
        assert_eq!(record.report.result.turn, 4);
    }
    let again = run(&config, Path::new("."), &dir, 3, |_, _, _| {
        panic!("nothing to play")
    })
    .unwrap();
    assert!(again.is_empty());
    let board = leaderboard(&config, &load_records(&dir).unwrap());
    assert_eq!((board.played, board.scheduled), (6, 6));
    assert_eq!(board.standings.len(), 3);
    assert!(board.standings.iter().all(|s| s.matches == 4));
    let easy = board
        .standings
        .iter()
        .find(|s| s.name == "Easy AI")
        .unwrap();
    assert_eq!(easy.kind, "built-in Easy");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_torn_last_line_is_skipped_and_its_fixture_is_played_again() {
    let dir = scratch("torn");
    let config = config();
    run(&config, Path::new("."), &dir, 2, |_, _, _| {}).unwrap();
    let path = dir.join(RESULTS);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    let last = lines.pop().unwrap();
    let torn = format!("{}\n{}", lines.join("\n"), &last[..last.len() / 2]);
    std::fs::write(&path, torn).unwrap();
    assert_eq!(load_records(&dir).unwrap().len(), 5);
    let replayed = run(&config, Path::new("."), &dir, 2, |_, _, _| {}).unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(
        load_records(&dir).unwrap().len(),
        6,
        "the new record is whole"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_report_has_an_index_and_one_page_per_match() {
    let dir = scratch("report");
    let config = config();
    let records = run(&config, Path::new("."), &dir, 3, |_, _, _| {}).unwrap();
    let index = legatum_arena::report::write_report(&config, &records, &dir).unwrap();
    let html = std::fs::read_to_string(&index).unwrap();
    assert!(html.contains("6 of 6 matches played"));
    for name in ["Echo A", "Echo B", "Easy AI"] {
        assert!(html.contains(name), "{name}");
    }
    for i in 0..6 {
        let page = dir.join("report").join(format!("match-{i:04}.html"));
        let page = std::fs::read_to_string(page).unwrap();
        assert!(page.contains("Victory points by turn"));
        assert!(page.contains("../games/"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
