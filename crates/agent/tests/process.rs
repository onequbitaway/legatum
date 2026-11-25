//! Agents here are small `sh` scripts, so the tests need a Unix shell.
#![cfg(unix)]

use legatum_agent::process::{AgentCommand, AgentError, AgentProcess};
use legatum_agent::protocol::{EndReason, GameInfo, Limits, MatchResult, Seat, ToAgent, encode};
use legatum_core::{Faction, GameSetup, PlayerId, TeamId, new_game, order_menu, player_view};
use std::time::{Duration, Instant};

/// Answers hello with its name and each turn with no orders, for the turn it was sent.
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

fn sh(script: &str) -> AgentProcess {
    AgentProcess::spawn(&AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        dir: None,
    })
    .expect("sh starts")
}

fn hello() -> ToAgent {
    ToAgent::Hello {
        protocol: 1,
        rules_version: legatum_core::RULES_VERSION,
        seat: Seat {
            player: PlayerId(0),
            team: TeamId(0),
            faction: Faction::Celts,
        },
        game: GameInfo {
            width: 40,
            height: 26,
            turn_limit: None,
            players: Vec::new(),
        },
        limits: Limits::default(),
    }
}

fn turn(turn: u32) -> ToAgent {
    let state = new_game(&GameSetup::all_ai(5));
    ToAgent::Turn {
        turn,
        view: Box::new(player_view(&state, PlayerId(0))),
        menu: order_menu(&state, PlayerId(0)),
        events: Vec::new(),
        rejected: Vec::new(),
    }
}

fn end() -> ToAgent {
    ToAgent::End {
        result: MatchResult {
            turn: 3,
            winner: None,
            vp: Vec::new(),
            reason: EndReason::Stopped,
        },
    }
}

const SHORT: Duration = Duration::from_millis(300);
const LONG: Duration = Duration::from_secs(10);

#[test]
fn an_agent_says_hello_answers_turns_and_exits_on_end() {
    let mut agent = sh(ECHO);
    assert_eq!(agent.hello(&hello(), LONG).unwrap(), "echo");
    for t in 1..=3 {
        assert_eq!(agent.turn(&turn(t), t, LONG).unwrap(), Vec::new());
    }
    assert!(agent.finish(&end(), LONG).is_some_and(|s| s.success()));
}

#[test]
fn a_silent_agent_times_out_without_holding_the_game() {
    let mut agent = sh("sleep 30");
    let start = Instant::now();
    assert_eq!(agent.hello(&hello(), SHORT), Err(AgentError::Timeout));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_late_answer_is_dropped_and_never_counts_for_the_next_turn() {
    // Turn 1 is answered after the deadline, then turn 2 at once.
    let mut agent = sh(r#"
read -r hello; echo '{"type":"hello","name":"slow"}'
read -r t1; sleep 1; echo '{"type":"orders","turn":1,"orders":[{"type":"SkipUnit","unit":1}]}'
read -r t2; echo '{"type":"orders","turn":2,"orders":[]}'
sleep 5
"#);
    agent.hello(&hello(), LONG).unwrap();
    assert_eq!(agent.turn(&turn(1), 1, SHORT), Err(AgentError::Timeout));
    assert_eq!(agent.turn(&turn(2), 2, LONG).unwrap(), Vec::new());
}

#[test]
fn an_agent_that_exits_is_gone() {
    let mut agent = sh("exit 3");
    assert!(matches!(
        agent.hello(&hello(), LONG),
        Err(AgentError::Gone | AgentError::Io(_))
    ));
    assert!(matches!(
        agent.turn(&turn(1), 1, SHORT),
        Err(AgentError::Gone)
    ));
}

#[test]
fn bad_answers_are_protocol_errors() {
    let mut agent = sh(
        r#"read -r h; echo 'hello there'; read -r t; echo '{"type":"orders","turn":9,"orders":[]}'; sleep 5"#,
    );
    assert!(matches!(
        agent.hello(&hello(), LONG),
        Err(AgentError::Protocol(_))
    ));
    let error = agent.turn(&turn(1), 1, LONG).unwrap_err();
    assert_eq!(
        error,
        AgentError::Protocol("orders for turn 9 during turn 1".into())
    );
}

#[test]
fn an_oversized_line_ends_the_agent_without_reading_it_all() {
    let mut agent = sh(r#"read -r h; head -c 3000000 /dev/zero | tr '\0' 'x'; echo; sleep 5"#);
    assert!(
        matches!(agent.hello(&hello(), LONG), Err(AgentError::Protocol(why)) if why.contains("bytes"))
    );
    assert!(agent.is_gone());
}

#[test]
fn standard_error_is_kept_for_the_log() {
    let mut agent = sh(
        r#"read -r h; echo 'thinking hard' >&2; echo '{"type":"hello","name":"loud"}'; sleep 5"#,
    );
    agent.hello(&hello(), LONG).unwrap();
    let deadline = Instant::now() + LONG;
    while agent.stderr_tail().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(agent.stderr_tail(), ["thinking hard"]);
}

#[test]
fn a_program_that_does_not_exist_fails_to_start() {
    let error =
        AgentProcess::spawn(&AgentCommand::from_words("no-such-agent-program --x").unwrap())
            .err()
            .unwrap();
    assert!(matches!(error, AgentError::Io(why) if why.contains("no-such-agent-program")));
}

#[test]
fn commands_split_on_whitespace() {
    let command = AgentCommand::from_words("  python3  agent.py --level 2 ").unwrap();
    assert_eq!(command.program, "python3");
    assert_eq!(command.args, ["agent.py", "--level", "2"]);
    assert_eq!(command.to_string(), "python3 agent.py --level 2");
    assert!(AgentCommand::from_words("   ").is_err());
}

#[test]
fn the_turn_message_starts_with_its_type_and_turn() {
    // The echo script above reads the turn number from this prefix.
    assert!(encode(&turn(12)).starts_with(r#"{"type":"turn","turn":12,"#));
}
