//! Agents against each other online, with no browser: `legatum-host` and two agent bridges.
#![cfg(unix)]

use legatum_agent::bridge::{BridgeConfig, run_bridge};
use legatum_agent::process::AgentCommand;
use legatum_agent::protocol::{EndReason, Limits};
use legatum_agent::setup::MatchSetup;
use legatum_host::{ServeOptions, serve};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

/// Answers hello with its name and each turn with no orders. A `sleep` before each answer
/// makes it slow.
fn sh_agent(name: &str, sleep: &str) -> AgentCommand {
    let script = format!(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"hello"'*) echo '{{"type":"hello","name":"{name}"}}' ;;
    *'"type":"turn"'*)
      sleep {sleep}
      t=$(printf '%s' "$line" | sed 's/^{{"type":"turn","turn":\([0-9]*\).*/\1/')
      echo "{{\"type\":\"orders\",\"turn\":$t,\"orders\":[]}}" ;;
    *'"type":"end"'*) exit 0 ;;
  esac
done
"#
    );
    AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script],
        dir: None,
    }
}

fn play(
    agents: [AgentCommand; 2],
    turn_seconds: u32,
    turn_limit: u32,
) -> Vec<legatum_agent::bridge::BridgeReport> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let mut setup = MatchSetup {
        seed: 17,
        players: 3,
        turn_limit: Some(turn_limit),
        ..MatchSetup::default()
    }
    .to_game_setup()
    .unwrap();
    for p in setup.players.iter_mut().take(2) {
        p.is_human = true;
    }
    let options = ServeOptions {
        setup,
        turn_seconds: Some(turn_seconds),
        save_dir: None,
        exit_when_finished: true,
        stop: Arc::new(AtomicBool::new(false)),
    };
    let (sender, credentials) = mpsc::channel();
    let host =
        thread::spawn(move || serve(listener, options, |c| sender.send(c.to_vec()).unwrap()));
    let credentials = credentials.recv_timeout(Duration::from_secs(10)).unwrap();
    let bridges: Vec<_> = agents
        .into_iter()
        .zip(credentials)
        .map(|(agent, credential)| {
            let url = url.clone();
            thread::spawn(move || {
                run_bridge(BridgeConfig {
                    url,
                    seat: credential.seat,
                    token: credential.token,
                    agent,
                    limits: Limits {
                        hello_ms: 10_000,
                        turn_ms: 10_000,
                    },
                })
            })
        })
        .collect();
    let reports = bridges
        .into_iter()
        .map(|b| b.join().unwrap().unwrap())
        .collect();
    assert!(host.join().unwrap().unwrap().finished);
    reports
}

#[test]
fn two_agents_play_a_whole_game_online() {
    let reports = play([sh_agent("north", "0"), sh_agent("south", "0")], 30, 6);
    for (report, name) in reports.iter().zip(["north", "south"]) {
        assert_eq!(report.name, name);
        assert_eq!((report.timeouts, report.errors, report.missed), (0, 0, 0));
        assert_eq!(report.turns, 6);
        let result = report.result.as_ref().unwrap();
        assert_eq!(result.turn, 6);
        assert_eq!(result.reason, EndReason::TurnLimit);
        assert_eq!(result.vp.len(), 3);
    }
}

#[test]
fn a_slow_agent_misses_its_turns_but_the_clock_keeps_the_game_going() {
    // With a 2 s clock the slow agent gets 1.6 s per turn and needs 3 s.
    let reports = play([sh_agent("quick", "0"), sh_agent("slow", "3")], 2, 4);
    assert_eq!(reports[0].timeouts, 0);
    assert!(reports[1].timeouts > 0, "{:?}", reports[1]);
    assert!(reports.iter().all(|r| r.result.is_some()));
}
