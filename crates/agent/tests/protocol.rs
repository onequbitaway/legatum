use legatum_agent::protocol::{
    EndReason, FromAgent, GameInfo, Limits, MAX_AGENT_LINE, MAX_ORDERS, MatchResult, PlayerInfo,
    Seat, SeatKind, ToAgent, decode_from_agent, encode,
};
use legatum_core::{
    Faction, GameSetup, Hex, Order, PlayerId, TeamId, UnitId, new_game, order_menu, player_view,
};

fn hello() -> ToAgent {
    ToAgent::Hello {
        protocol: 1,
        rules_version: legatum_core::RULES_VERSION,
        seat: Seat {
            player: PlayerId(1),
            team: TeamId(1),
            faction: Faction::Sparta,
        },
        game: GameInfo {
            width: 40,
            height: 26,
            turn_limit: Some(100),
            players: vec![PlayerInfo {
                player: PlayerId(0),
                team: TeamId(0),
                faction: Faction::Athens,
                kind: SeatKind::Ai,
            }],
        },
        limits: Limits::default(),
    }
}

#[test]
fn every_game_message_survives_a_line_round_trip() {
    let state = new_game(&GameSetup::all_ai(3));
    let turn = ToAgent::Turn {
        turn: state.turn,
        view: Box::new(player_view(&state, PlayerId(0))),
        menu: order_menu(&state, PlayerId(0)),
        events: vec![serde_json::json!({"type": "TurnStarted"})],
        rejected: Vec::new(),
    };
    let end = ToAgent::End {
        result: MatchResult {
            turn: 40,
            winner: Some(TeamId(2)),
            vp: vec![(PlayerId(0), 3), (PlayerId(1), 9)],
            reason: EndReason::Victory,
        },
    };
    for message in [hello(), turn, end] {
        let line = encode(&message);
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<ToAgent>(&line).unwrap(), message);
    }
}

#[test]
fn agent_answers_have_a_stable_wire_shape() {
    assert_eq!(
        decode_from_agent(r#"{"type":"hello","name":"Greedy","version":"0.3"}"#).unwrap(),
        FromAgent::Hello {
            name: "Greedy".into(),
            version: Some("0.3".into()),
        }
    );
    let orders = decode_from_agent(
        r#"{"type":"orders","turn":4,"orders":[{"type":"MoveUnit","unit":7,"to":{"q":3,"r":2}}]}"#,
    )
    .unwrap();
    assert_eq!(
        orders,
        FromAgent::Orders {
            turn: 4,
            orders: vec![Order::MoveUnit {
                unit: UnitId(7),
                to: Hex::new(3, 2),
            }],
        }
    );
    assert_eq!(
        encode(&orders),
        r#"{"type":"orders","turn":4,"orders":[{"type":"MoveUnit","unit":7,"to":{"q":3,"r":2}}]}"#
    );
}

#[test]
fn a_trailing_newline_is_accepted() {
    assert!(decode_from_agent("{\"type\":\"orders\",\"turn\":1,\"orders\":[]}\r\n").is_ok());
}

#[test]
fn bad_answers_are_refused_with_a_reason() {
    for line in [
        "",
        "not json",
        r#"{"type":"orders","orders":[]}"#,
        r#"{"type":"resign"}"#,
        r#"{"type":"orders","turn":1,"orders":[{"type":"Teleport"}]}"#,
    ] {
        let error = decode_from_agent(line).unwrap_err();
        assert!(error.starts_with("bad message"), "{line}: {error}");
    }
}

#[test]
fn oversized_answers_are_refused() {
    let long = format!(
        "{{\"type\":\"hello\",\"name\":\"{}\"}}",
        "x".repeat(MAX_AGENT_LINE)
    );
    assert!(decode_from_agent(&long).unwrap_err().contains("bytes"));
    let skip = r#"{"type":"SkipUnit","unit":1}"#;
    let many = format!(
        "{{\"type\":\"orders\",\"turn\":1,\"orders\":[{}]}}",
        vec![skip; MAX_ORDERS + 1].join(",")
    );
    assert!(decode_from_agent(&many).unwrap_err().contains("at most"));
}
