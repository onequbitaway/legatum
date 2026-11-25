use super::{OrderMenu, order_menu};
use crate::orders::Order;
use crate::state::{GameSetup, GameState};
use crate::types::{Faction, PlayerId};
use crate::{new_game, player_view, resolve_turn, validate_orders};
use std::collections::BTreeMap;

fn standard_game() -> GameState {
    new_game(&GameSetup::standard(
        7,
        [Faction::Switzerland, Faction::Mongols],
    ))
}

/// The game after `turns` turns in which nobody gives an order.
fn played(turns: u32) -> GameState {
    let mut state = standard_game();
    for _ in 0..turns {
        state = resolve_turn(&state, &BTreeMap::new()).0;
    }
    state
}

fn assert_each_order_is_valid(state: &GameState, menu: &OrderMenu) {
    for order in menu.orders() {
        let errors = validate_orders(state, menu.player, std::slice::from_ref(order));
        assert!(
            errors.is_empty(),
            "{order:?} is listed but refused: {errors:?}"
        );
    }
}

#[test]
fn every_listed_order_is_valid_alone() {
    for state in [standard_game(), played(12)] {
        for player in state.players.iter().map(|p| p.id) {
            assert_each_order_is_valid(&state, &order_menu(&state, player));
        }
    }
}

#[test]
fn a_new_game_lists_moves_production_and_research() {
    let state = standard_game();
    let menu = order_menu(&state, PlayerId(0));
    assert_eq!(menu.turn, state.turn);
    assert!(!menu.units.is_empty(), "every empire starts with units");
    assert!(
        menu.units
            .iter()
            .any(|u| u.orders.iter().any(|o| matches!(o, Order::MoveUnit { .. })))
    );
    let capital = menu
        .cities
        .first()
        .expect("every empire starts with a capital");
    assert!(
        capital
            .orders
            .iter()
            .any(|o| matches!(o, Order::SetProduction { .. }))
    );
    assert!(
        menu.empire
            .iter()
            .any(|o| matches!(o, Order::SetResearch { .. }))
    );
}

#[test]
fn units_and_cities_are_the_seats_own() {
    let state = played(5);
    let player = PlayerId(2);
    let menu = order_menu(&state, player);
    for u in &menu.units {
        assert_eq!(state.units[&u.unit].owner, player);
    }
    for c in &menu.cities {
        assert_eq!(state.cities[&c.city].owner, player);
    }
}

#[test]
fn moves_and_shots_target_only_what_the_seat_knows() {
    let state = played(12);
    for player in state.players.iter().map(|p| p.id) {
        let view = player_view(&state, player);
        for order in order_menu(&state, player).orders() {
            match order {
                Order::MoveUnit { to, .. } => {
                    assert!(
                        view.tile(*to).is_some_and(|t| t.explored),
                        "{to:?} unexplored"
                    );
                }
                Order::RangedAttack { target, .. } => assert!(
                    view.units.iter().any(|u| u.pos == *target)
                        || view.cities.iter().any(|c| c.pos == *target),
                    "{target:?} is not in the view"
                ),
                _ => {}
            }
        }
    }
}

#[test]
fn an_eliminated_seat_or_a_finished_game_has_an_empty_menu() {
    let mut state = standard_game();
    state.players[3].alive = false;
    assert!(order_menu(&state, PlayerId(3)).is_empty());
    assert!(order_menu(&state, PlayerId(200)).is_empty());
    state.finished = true;
    assert!(order_menu(&state, PlayerId(0)).is_empty());
}

#[test]
fn the_menu_is_the_same_for_the_same_state() {
    let state = played(3);
    assert_eq!(
        order_menu(&state, PlayerId(1)),
        order_menu(&state, PlayerId(1))
    );
}

#[test]
fn the_menu_survives_a_json_round_trip() {
    let menu = order_menu(&standard_game(), PlayerId(0));
    let json = serde_json::to_string(&menu).unwrap();
    let back: OrderMenu = serde_json::from_str(&json).unwrap();
    assert_eq!(back, menu);
    assert_eq!(back.len(), menu.len());
}
