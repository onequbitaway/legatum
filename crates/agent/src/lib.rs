//! Agent seats: outside programs that play a seat of a Legatum game.
//!
//! An agent is a separate process. It reads one JSON message per line on standard input and
//! writes one JSON message per line on standard output ([`protocol`]). It gets only what a human
//! in its seat gets: its own `PlayerView`, the events its team saw, and the order menu
//! (`legatum_core::order_menu`). It plays under the same rules, with no bonus and no hidden
//! information. The guide for agent authors is `docs/agents/`.
