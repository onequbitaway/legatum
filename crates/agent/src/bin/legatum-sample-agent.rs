//! `legatum-sample-agent`: a small agent that shows the protocol in Rust. It is a starting
//! point, not a strong player. Each turn it:
//!
//! - lets Scouts explore and lets every city choose its builds (Military while an enemy unit is
//!   in sight, Economy otherwise), through the game's Ready helpers;
//! - keeps a research plan: the cheapest technology the menu offers;
//! - founds a city with each Settler as soon as the menu allows it at least three tiles from its
//!   own cities, and else walks the Settler away from them;
//! - shoots and attacks visible enemies its units can reach, and otherwise leaves army units to
//!   rest where they are;
//! - accepts every Peace offer.
//!
//! Run it with `legatum-agent match --agent 0=target/release/legatum-sample-agent`.

use legatum_agent::protocol::{FromAgent, ToAgent, encode};
use legatum_core::diplomacy::TreatyKind;
use legatum_core::view::UnitView;
use legatum_core::{Automation, CityBuildFocus, Hex, Order, OrderMenu, PlayerView, UnitKind};
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let reply = match serde_json::from_str::<ToAgent>(&line?) {
            Ok(ToAgent::Hello { .. }) => FromAgent::Hello {
                name: "Sample agent".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            },
            Ok(ToAgent::Turn {
                turn,
                view,
                menu,
                rejected,
                ..
            }) => {
                for refused in rejected {
                    eprintln!(
                        "turn {turn}: order {} refused: {}",
                        refused.index, refused.message
                    );
                }
                FromAgent::Orders {
                    turn,
                    orders: plan(&view, &menu),
                }
            }
            Ok(ToAgent::End { .. }) => return Ok(()),
            Err(error) => {
                eprintln!("sample agent: cannot read a message: {error}");
                continue;
            }
        };
        writeln!(out, "{}", encode(&reply))?;
        out.flush()?;
    }
    Ok(())
}

fn plan(view: &PlayerView, menu: &OrderMenu) -> Vec<Order> {
    let mut orders = Vec::new();
    let enemies: Vec<&UnitView> = view
        .units
        .iter()
        .filter(|u| u.owner != view.me && view.at_war(view.me, u.owner))
        .collect();
    for unit in &menu.units {
        let Some(me) = view.units.iter().find(|u| u.id == unit.unit) else {
            continue;
        };
        let order = match unit.kind {
            UnitKind::Scout if me.automation.is_none() => Some(Order::SetAutomation {
                unit: unit.unit,
                automation: Some(Automation::Explore),
                path: Vec::new(),
                stop_reason: None,
            }),
            UnitKind::Settler => settle(view, &unit.orders, me),
            _ if me.military => fight(&unit.orders, &enemies)
                .or_else(|| {
                    (me.standing_order.is_none() && me.goto.is_none()).then_some(Order::SleepUnit {
                        unit: unit.unit,
                        until_healed: false,
                    })
                })
                .filter(|o| unit.orders.contains(o)),
            _ => None,
        };
        orders.extend(order);
    }
    let focus = if enemies.is_empty() {
        CityBuildFocus::Economy
    } else {
        CityBuildFocus::Military
    };
    for city in &menu.cities {
        let current = view
            .cities
            .iter()
            .find(|c| c.id == city.city)
            .and_then(|c| c.detail.as_ref())
            .and_then(|d| d.build_automation);
        let wanted = Order::SetCityAutomation {
            city: city.city,
            focus: Some(focus),
            picked: None,
        };
        if current != Some(focus) && city.orders.contains(&wanted) {
            orders.push(wanted);
        }
    }
    if view.research.current.is_none()
        && let Some(tech) = view
            .research
            .available
            .iter()
            .filter(|t| t.researchable)
            .min_by_key(|t| (t.cost.total, t.tech))
    {
        orders.extend(
            menu.empire
                .iter()
                .find(|o| matches!(o, Order::SetResearch { tech: t } if *t == tech.tech))
                .cloned(),
        );
    }
    orders.extend(menu.empire.iter().filter_map(|o| {
        match o {
            Order::RespondToProposal {
                proposal,
                accept: true,
                ..
            } if view
                .diplomacy
                .proposals
                .iter()
                .any(|p| p.id == *proposal && p.kind == TreatyKind::Peace) =>
            {
                Some(o.clone())
            }
            _ => None,
        }
    }));
    orders
}

/// Found here if the menu allows it far enough from home, else step away from home.
fn settle(view: &PlayerView, options: &[Order], me: &UnitView) -> Option<Order> {
    let homes: Vec<Hex> = view
        .cities
        .iter()
        .filter(|c| c.owner == view.me)
        .map(|c| c.pos)
        .collect();
    let distance = |h: Hex| {
        homes
            .iter()
            .map(|c| c.distance(h))
            .min()
            .unwrap_or(i32::MAX)
    };
    if distance(me.pos) >= 3
        && let Some(found) = options
            .iter()
            .find(|o| matches!(o, Order::FoundCity { .. }))
    {
        return Some(found.clone());
    }
    options
        .iter()
        .filter_map(|o| match o {
            Order::MoveUnit { to, .. } => Some((distance(*to), *to, o)),
            _ => None,
        })
        .filter(|(d, _, _)| *d <= 5)
        .max_by_key(|(d, to, _)| (*d, std::cmp::Reverse(*to)))
        .map(|(_, _, o)| o.clone())
}

/// A shot, or a step onto a visible enemy unit, from the unit's menu.
fn fight(options: &[Order], enemies: &[&UnitView]) -> Option<Order> {
    let on_enemy = |h: &Hex| enemies.iter().any(|e| e.pos == *h);
    options
        .iter()
        .find(|o| matches!(o, Order::RangedAttack { target, .. } if on_enemy(target)))
        .or_else(|| {
            options
                .iter()
                .find(|o| matches!(o, Order::MoveUnit { to, .. } if on_enemy(to)))
        })
        .cloned()
}
