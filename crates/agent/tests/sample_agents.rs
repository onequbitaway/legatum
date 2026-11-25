//! The sample agents play whole turns without a refused order or a protocol error. The Python
//! agent needs `python3` (a setup requirement in CONTRIBUTING.md).
#![cfg(unix)]

use legatum_agent::process::AgentCommand;
use legatum_agent::protocol::Limits;
use legatum_agent::runner::{MatchConfig, MatchReport, run_match};
use legatum_agent::setup::MatchSetup;
use legatum_core::PlayerId;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn rust_agent() -> AgentCommand {
    AgentCommand::from_words(env!("CARGO_BIN_EXE_legatum-sample-agent")).unwrap()
}

fn python_agent() -> AgentCommand {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/agents/random_agent.py");
    AgentCommand {
        program: "python3".into(),
        args: vec![script.to_string_lossy().into_owned()],
        dir: None,
    }
}

fn play(agents: [AgentCommand; 2], turns: u32) -> MatchReport {
    let config = MatchConfig {
        setup: MatchSetup {
            seed: 13,
            ..MatchSetup::default()
        }
        .to_game_setup()
        .unwrap(),
        agents: BTreeMap::from([
            (PlayerId(0), agents[0].clone()),
            (PlayerId(1), agents[1].clone()),
        ]),
        limits: Limits {
            hello_ms: 10_000,
            turn_ms: 10_000,
        },
        max_turns: turns,
    };
    run_match(config, |_| {}).unwrap().0
}

#[test]
fn the_rust_and_python_sample_agents_play_each_other() {
    let report = play([rust_agent(), python_agent()], 25);
    let names: Vec<_> = report
        .seats
        .iter()
        .map(|s| s.agent.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(names, ["Sample agent", "Random (Python)"]);
    for seat in &report.seats {
        let agent = seat.agent.as_ref().unwrap();
        assert_eq!(
            (agent.answered, agent.timeouts, agent.errors),
            (25, 0, 0),
            "{agent:?}"
        );
    }
    let rust = report.seats[0].agent.as_ref().unwrap();
    assert_eq!(rust.rejected, 0, "{:?}", rust.stderr);
}
