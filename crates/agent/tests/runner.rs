//! Whole matches with `sh` agents. Unix only.
#![cfg(unix)]

mod common;

use common::{BAD_ORDER, ECHO, explore_once, sh};
use legatum_agent::process::AgentCommand;
use legatum_agent::protocol::{EndReason, Limits, SeatKind};
use legatum_agent::runner::{MatchConfig, run_match};
use legatum_agent::setup::MatchSetup;
use legatum_core::PlayerId;
use std::collections::BTreeMap;

fn config(agents: &[(u8, AgentCommand)], max_turns: u32) -> MatchConfig {
    MatchConfig {
        setup: MatchSetup {
            seed: 21,
            ..MatchSetup::default()
        }
        .to_game_setup()
        .unwrap(),
        agents: agents
            .iter()
            .map(|(seat, command)| (PlayerId(*seat), command.clone()))
            .collect::<BTreeMap<_, _>>(),
        limits: Limits {
            hello_ms: 5_000,
            turn_ms: 5_000,
        },
        max_turns,
    }
}

#[test]
fn two_agents_play_each_other_turn_by_turn() {
    let mut seen = Vec::new();
    let (report, session) = run_match(config(&[(0, sh(ECHO)), (1, sh(ECHO))], 4), |state| {
        seen.push(state.turn);
    })
    .unwrap();
    assert_eq!(seen.len(), 4);
    assert_eq!(report.vp_by_turn.len(), 4);
    assert_eq!(report.result.reason, EndReason::Stopped);
    assert_eq!(report.result.turn, 4);
    for seat in &report.seats {
        assert_eq!(seat.kind, SeatKind::Agent);
        let agent = seat.agent.as_ref().unwrap();
        assert_eq!(agent.name, "echo");
        assert_eq!((agent.answered, agent.timeouts, agent.errors), (4, 0, 0));
    }
    assert!(session.state.players.iter().all(|p| p.is_human));
    assert_eq!(session.history.as_ref().unwrap().tape.turns.len(), 4);
}

#[test]
fn an_agent_against_the_built_in_ai() {
    let (report, session) = run_match(config(&[(1, sh(ECHO))], 3), |_| {}).unwrap();
    assert_eq!(report.seats[0].kind, SeatKind::Ai);
    assert!(report.seats[0].difficulty.is_some());
    assert_eq!(report.seats[1].kind, SeatKind::Agent);
    assert!(!session.state.players[0].is_human);
    assert!(session.state.players[1].is_human);
}

#[test]
fn refused_orders_are_dropped_counted_and_the_turn_still_resolves() {
    let (report, _) = run_match(config(&[(0, sh(BAD_ORDER))], 3), |_| {}).unwrap();
    let agent = report.seats[0].agent.as_ref().unwrap();
    assert_eq!(agent.answered, 3);
    assert_eq!(agent.rejected, 3);
}

#[test]
fn an_agent_that_never_says_hello_leaves_its_seat_without_orders() {
    let (report, _) = run_match(config(&[(0, sh("exit 0")), (1, sh(ECHO))], 2), |_| {}).unwrap();
    let broken = report.seats[0].agent.as_ref().unwrap();
    assert_eq!(broken.gone_turn, Some(0));
    assert_eq!(broken.answered, 0);
    assert!(broken.last_error.as_ref().unwrap().starts_with("hello"));
    assert_eq!(report.seats[1].agent.as_ref().unwrap().answered, 2);
}

#[test]
fn a_match_ends_at_the_game_turn_limit() {
    let mut config = config(&[(0, sh(ECHO))], 50);
    config.setup.turn_limit = Some(3);
    let (report, session) = run_match(config, |_| {}).unwrap();
    assert!(session.state.finished);
    assert_eq!(report.result.reason, EndReason::TurnLimit);
    assert_eq!(report.result.vp.len(), 2);
}

#[test]
fn a_seat_outside_the_game_is_refused() {
    let error = run_match(config(&[(5, sh(ECHO))], 1), |_| {})
        .err()
        .unwrap();
    assert!(error.contains("no seat 5"));
}

#[test]
fn the_same_agents_and_seed_give_the_same_game() {
    let play = || {
        run_match(config(&[(0, sh(ECHO))], 5), |_| {})
            .unwrap()
            .1
            .state_hash()
    };
    assert_eq!(play(), play());
}

#[test]
fn an_agent_can_leave_a_scout_exploring() {
    let config = config(&[(0, sh(ECHO))], 1);
    let state = legatum_core::new_game(&config.setup);
    let scout = state
        .units
        .values()
        .find(|u| u.owner == PlayerId(0) && u.kind == legatum_core::UnitKind::Scout)
        .unwrap();
    let (start, id) = (scout.pos, scout.id);
    let mut config = config;
    config.agents.insert(PlayerId(0), sh(&explore_once(id.0)));
    config.max_turns = 4;
    let (report, session) = run_match(config, |_| {}).unwrap();
    assert_eq!(report.seats[0].agent.as_ref().unwrap().rejected, 0);
    assert_ne!(session.state.units[&id].pos, start);
}
