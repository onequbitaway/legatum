//! The helpers the app runs for a human seat when it presses Ready (`app/src/automation.ts`),
//! run here for agent seats, so an agent can delegate the same work a human can:
//!
//! - **Explore.** A unit whose order is `SetAutomation { automation: Some(Explore) }`, or that
//!   still explores from an earlier turn and has no new order, gets this turn's step from
//!   `legatum_ai::explore_step`, or stops exploring when nothing is left.
//! - **City automation.** A city with an automation focus (`SetCityAutomation`) gets its next
//!   build from the same chooser the app uses (`legatum_wasm::city_automation_orders_json`).
//!
//! Both plan only from the seat's own view.

use legatum_core::rules::units::order_unit;
use legatum_core::{Automation, ExploreStopReason, Order, PlayerView, UnitId};
use std::collections::BTreeSet;

/// `orders` with the Ready helpers applied.
pub fn ready_orders(view: &PlayerView, orders: Vec<Order>) -> Vec<Order> {
    with_city_automation(view, with_exploration(view, orders))
}

/// The seat's units that explore this turn, by id.
fn exploring(view: &PlayerView, orders: &[Order]) -> BTreeSet<UnitId> {
    view.units
        .iter()
        .filter(|u| u.owner == view.me)
        .filter(
            |u| match orders.iter().find(|o| order_unit(o) == Some(u.id)) {
                Some(order) => matches!(
                    order,
                    Order::SetAutomation {
                        automation: Some(Automation::Explore),
                        ..
                    }
                ),
                None => u.automation == Some(Automation::Explore),
            },
        )
        .map(|u| u.id)
        .collect()
}

fn with_exploration(view: &PlayerView, orders: Vec<Order>) -> Vec<Order> {
    let units = exploring(view, &orders);
    if units.is_empty() {
        return orders;
    }
    let mut out: Vec<Order> = orders
        .into_iter()
        .filter(|o| order_unit(o).is_none_or(|u| !units.contains(&u)))
        .collect();
    out.extend(units.into_iter().map(|unit| {
        legatum_ai::explore_step(view, unit).unwrap_or(Order::SetAutomation {
            unit,
            automation: None,
            path: Vec::new(),
            stop_reason: Some(ExploreStopReason::Complete),
        })
    }));
    out
}

#[derive(serde::Deserialize)]
struct CityPlan {
    orders: Vec<Order>,
}

fn with_city_automation(view: &PlayerView, orders: Vec<Order>) -> Vec<Order> {
    let automated = view.cities.iter().any(|c| {
        c.owner == view.me
            && orders
                .iter()
                .rev()
                .find_map(|o| match o {
                    Order::SetCityAutomation { city, focus, .. } if *city == c.id => Some(*focus),
                    _ => None,
                })
                .unwrap_or_else(|| c.detail.as_ref().and_then(|d| d.build_automation))
                .is_some()
    });
    if !automated {
        return orders;
    }
    let view_json = serde_json::to_string(view).expect("views serialize");
    let orders_json = serde_json::to_string(&orders).expect("orders serialize");
    let Ok(plan) = legatum_wasm::city_automation_orders_json(&view_json, &orders_json)
        .and_then(|json| serde_json::from_str::<CityPlan>(&json).map_err(|e| e.to_string()))
    else {
        return orders;
    };
    let picked: BTreeSet<_> = plan
        .orders
        .iter()
        .filter_map(|o| match o {
            Order::SetCityAutomation { city, .. } => Some(*city),
            _ => None,
        })
        .collect();
    orders
        .into_iter()
        .filter(|o| !matches!(o, Order::SetCityAutomation { city, .. } if picked.contains(city)))
        .chain(plan.orders)
        .collect()
}
