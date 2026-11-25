//! The order menu: single orders a seat may give this turn, each already checked by
//! [`validate_orders`]. It is for outside players such as programs (`crates/agent`), so they can
//! pick from a list instead of guessing ids and hexes.
//!
//! Every candidate comes from the seat's own [`PlayerView`], and the checks are the ones a client
//! gets from the host, which never reveal hidden state. So the menu tells a seat nothing its view
//! and the validator do not already tell it.
//!
//! The menu lists one order at a time. Orders that share a budget (Gold, strategic supply, a city's
//! one purchase per turn) are each valid alone and may not all be valid together; the host
//! validates the whole list when it is sent. Air orders are not listed yet.

use crate::diplomacy::TreatyKind;
use crate::hex::Hex;
use crate::orders::Order;
use crate::rules::validate_orders;
use crate::state::GameState;
use crate::types::{CityBuildFocus, CityId, PlayerId, TeamId, UnitId, UnitKind};
use crate::view::{PlayerView, UnitView, player_view};
use serde::{Deserialize, Serialize};

/// Every listed order is valid on its own for `player` on planning turn `turn`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderMenu {
    pub turn: u32,
    pub player: PlayerId,
    /// Own units on the map, by id.
    pub units: Vec<UnitMenu>,
    /// Own cities, by id.
    pub cities: Vec<CityMenu>,
    /// Research and diplomacy.
    pub empire: Vec<Order>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitMenu {
    pub unit: UnitId,
    pub kind: UnitKind,
    pub pos: Hex,
    pub orders: Vec<Order>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityMenu {
    pub city: CityId,
    pub name: String,
    pub orders: Vec<Order>,
}

impl OrderMenu {
    /// All listed orders, units first, then cities, then the empire.
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.units
            .iter()
            .flat_map(|u| &u.orders)
            .chain(self.cities.iter().flat_map(|c| &c.orders))
            .chain(&self.empire)
    }

    pub fn len(&self) -> usize {
        self.orders().count()
    }

    pub fn is_empty(&self) -> bool {
        self.orders().next().is_none()
    }
}

const CITY_FOCUSES: [CityBuildFocus; 5] = [
    CityBuildFocus::Military,
    CityBuildFocus::Economy,
    CityBuildFocus::Gold,
    CityBuildFocus::Science,
    CityBuildFocus::Food,
];

const TREATIES: [TreatyKind; 3] = [
    TreatyKind::Peace,
    TreatyKind::Alliance,
    TreatyKind::OpenBorders,
];

/// The order menu of `player` for the current planning turn. Empty for an eliminated player or a
/// finished game.
pub fn order_menu(state: &GameState, player: PlayerId) -> OrderMenu {
    let mut menu = OrderMenu {
        turn: state.turn,
        player,
        units: Vec::new(),
        cities: Vec::new(),
        empire: Vec::new(),
    };
    let known = (player.0 as usize) < state.players.len();
    if state.finished || !known || !state.player(player).alive {
        return menu;
    }
    let view = player_view(state, player);
    let units: Vec<&UnitView> = view
        .units
        .iter()
        .filter(|u| u.owner == player && !crate::data::is_aircraft(u.kind))
        .collect();
    let cities: Vec<_> = view.cities.iter().filter(|c| c.owner == player).collect();
    let candidates: Vec<Vec<Order>> = units
        .iter()
        .map(|u| unit_candidates(&view, u))
        .chain(cities.iter().map(|c| city_candidates(&view, c.id)))
        .chain([empire_candidates(&view)])
        .collect();
    let mut valid = keep_valid(state, player, candidates).into_iter();
    menu.units = units
        .iter()
        .zip(valid.by_ref())
        .map(|(u, orders)| UnitMenu {
            unit: u.id,
            kind: u.kind,
            pos: u.pos,
            orders,
        })
        .collect();
    menu.cities = cities
        .iter()
        .zip(valid.by_ref())
        .map(|(c, orders)| CityMenu {
            city: c.id,
            name: c.name.clone(),
            orders,
        })
        .collect();
    menu.empire = valid.next().unwrap_or_default();
    menu
}

/// Keeps the candidates `validate_orders` accepts alone, per actor and in order.
fn keep_valid(state: &GameState, player: PlayerId, candidates: Vec<Vec<Order>>) -> Vec<Vec<Order>> {
    candidates
        .into_iter()
        .map(|orders| {
            orders
                .into_iter()
                .filter(|o| validate_orders(state, player, std::slice::from_ref(o)).is_empty())
                .collect()
        })
        .collect()
}

/// Standing orders, founding, moves to every explored tile the unit could reach on open ground
/// this turn, shots at visible enemies in range, and upgrades.
fn unit_candidates(view: &PlayerView, u: &UnitView) -> Vec<Order> {
    let unit = u.id;
    let mut orders = vec![
        Order::SkipUnit { unit },
        Order::SleepUnit {
            unit,
            until_healed: false,
        },
        Order::SleepUnit {
            unit,
            until_healed: true,
        },
        Order::FoundCity { unit },
        Order::UpgradeUnit { unit },
        Order::UpgradeSiege { unit },
        Order::DisbandUnit { unit },
    ];
    let reach = u.max_moves.max(1);
    orders.extend(
        u.pos
            .within_iter(reach)
            .filter(|&to| to != u.pos && view.tile(to).is_some_and(|t| t.explored))
            .map(|to| Order::MoveUnit { unit, to }),
    );
    if u.range > 0 {
        let enemy_units = view
            .units
            .iter()
            .filter(|e| e.owner != view.me && !view.same_team(e.owner, view.me))
            .map(|e| e.pos);
        let enemy_cities = view
            .cities
            .iter()
            .filter(|c| c.owner != view.me && !view.same_team(c.owner, view.me))
            .map(|c| c.pos);
        let mut targets: Vec<Hex> = enemy_units
            .chain(enemy_cities)
            .filter(|t| u.pos.distance(*t) <= u.range)
            .collect();
        targets.sort();
        targets.dedup();
        orders.extend(
            targets
                .into_iter()
                .map(|target| Order::RangedAttack { unit, target }),
        );
    }
    orders
}

/// Each production choice the city lists, to build or to buy, and each automation focus.
fn city_candidates(view: &PlayerView, city: CityId) -> Vec<Order> {
    let Some(detail) = view
        .cities
        .iter()
        .find(|c| c.id == city)
        .and_then(|c| c.detail.as_ref())
    else {
        return Vec::new();
    };
    let build = detail
        .options
        .iter()
        .map(|o| Order::SetProduction { city, item: o.item });
    let buy = detail
        .options
        .iter()
        .filter(|o| o.buy_cost.is_some() && o.buy_blocker.is_none())
        .map(|o| Order::BuyItem { city, item: o.item });
    let automate = CITY_FOCUSES
        .into_iter()
        .map(Some)
        .chain([None])
        .map(|focus| Order::SetCityAutomation {
            city,
            focus,
            picked: None,
        });
    build.chain(buy).chain(automate).collect()
}

/// Each technology open to research, and War, offers and replies toward every team this one
/// has met.
fn empire_candidates(view: &PlayerView) -> Vec<Order> {
    let mut orders: Vec<Order> = view
        .research
        .available
        .iter()
        .filter(|t| t.researchable)
        .map(|t| Order::SetResearch { tech: t.tech })
        .collect();
    let mine = view.team;
    let mut met: Vec<TeamId> = view
        .diplomacy
        .met
        .iter()
        .copied()
        .filter(|t| *t != mine && *t != TeamId::BARBARIAN)
        .collect();
    met.sort();
    met.dedup();
    for to in met {
        orders.push(Order::DeclareWar {
            against: to,
            breach: false,
        });
        orders.extend(
            TREATIES
                .into_iter()
                .map(|kind| Order::ProposeTreaty { to, kind, gold: 0 }),
        );
    }
    for proposal in &view.diplomacy.proposals {
        if proposal.to == mine {
            orders.extend([true, false].map(|accept| Order::RespondToProposal {
                proposal: proposal.id,
                accept,
                explanation: None,
            }));
        } else if proposal.from == mine {
            orders.push(Order::WithdrawProposal {
                proposal: proposal.id,
            });
        }
    }
    orders
}
