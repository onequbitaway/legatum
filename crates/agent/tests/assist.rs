use legatum_agent::assist::ready_orders;
use legatum_core::rules::units::order_unit;
use legatum_core::{
    Automation, CityBuildFocus, GameSetup, Order, PlayerId, UnitKind, new_game, player_view,
    resolve_turn,
};
use std::collections::BTreeMap;

fn first_turn() -> legatum_core::GameState {
    new_game(&GameSetup::all_ai(8))
}

#[test]
fn orders_without_automation_pass_unchanged() {
    let state = first_turn();
    let view = player_view(&state, PlayerId(0));
    let unit = view.units.iter().find(|u| u.owner == view.me).unwrap().id;
    let orders = vec![Order::SkipUnit { unit }];
    assert_eq!(ready_orders(&view, orders.clone()), orders);
}

#[test]
fn an_exploring_scout_gets_this_turns_step() {
    let state = first_turn();
    let view = player_view(&state, PlayerId(0));
    let scout = view
        .units
        .iter()
        .find(|u| u.owner == view.me && u.kind == UnitKind::Scout)
        .unwrap()
        .id;
    let orders = ready_orders(
        &view,
        vec![Order::SetAutomation {
            unit: scout,
            automation: Some(Automation::Explore),
            path: Vec::new(),
            stop_reason: None,
        }],
    );
    let mine: Vec<_> = orders
        .iter()
        .filter(|o| order_unit(o) == Some(scout))
        .collect();
    assert_eq!(mine.len(), 1, "one order per unit: {orders:?}");
    assert!(
        matches!(mine[0], Order::SetAutomation { automation: Some(Automation::Explore), path, .. } if !path.is_empty()),
        "{mine:?}"
    );
    assert!(legatum_core::validate_orders(&state, PlayerId(0), &orders).is_empty());
}

#[test]
fn a_scout_keeps_exploring_on_later_turns_without_new_orders() {
    let state = first_turn();
    let view = player_view(&state, PlayerId(0));
    let scout = view
        .units
        .iter()
        .find(|u| u.owner == view.me && u.kind == UnitKind::Scout)
        .unwrap();
    let start = scout.pos;
    let mut state = state.clone();
    let mut orders = vec![Order::SetAutomation {
        unit: scout.id,
        automation: Some(Automation::Explore),
        path: Vec::new(),
        stop_reason: None,
    }];
    for _ in 0..3 {
        let view = player_view(&state, PlayerId(0));
        let ready = ready_orders(&view, std::mem::take(&mut orders));
        state = resolve_turn(&state, &BTreeMap::from([(PlayerId(0), ready)])).0;
    }
    assert_ne!(state.units[&scout.id].pos, start);
}

#[test]
fn an_automated_city_gets_its_next_build() {
    let state = first_turn();
    let view = player_view(&state, PlayerId(0));
    let city = view.cities.iter().find(|c| c.owner == view.me).unwrap().id;
    let orders = ready_orders(
        &view,
        vec![Order::SetCityAutomation {
            city,
            focus: Some(CityBuildFocus::Economy),
            picked: None,
        }],
    );
    assert!(
        orders.iter().any(|o| matches!(o,
            Order::SetProduction { city: c, .. } | Order::SetProductionQueue { city: c, .. } if *c == city)),
        "{orders:?}"
    );
    assert!(orders.iter().any(|o| matches!(o,
        Order::SetCityAutomation { city: c, focus: Some(CityBuildFocus::Economy), picked: Some(_) } if *c == city)));
    assert!(legatum_core::validate_orders(&state, PlayerId(0), &orders).is_empty());
}
