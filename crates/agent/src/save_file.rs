//! The app's save-file envelope (`app/src/store.ts`, `SaveFile`) around an engine save, so a game
//! that agents played opens with **Load Game** in the app. The app refuses a bare engine save.

use serde_json::{Value, json};

/// Wraps `game` (an engine save, `Session::save_json` or the online host's save) in the app's
/// envelope. A game with two or more human seats (agent seats count as human) is marked
/// `online`, as the app's own online saves are: the app offers to host it.
pub fn app_save_file(game: &str) -> Result<String, String> {
    let state: Value =
        serde_json::from_str(game).map_err(|e| format!("not an engine save: {e}"))?;
    let humans: Vec<&Value> = state["players"]
        .as_array()
        .map(|players| players.iter().filter(|p| p["is_human"] == true).collect())
        .unwrap_or_default();
    let names: Vec<&Value> = humans.iter().map(|p| &p["name"]).collect();
    let saved_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| format!("cannot write the time: {e}"))?;
    let mut envelope = json!({
        "format": "legatum-save",
        "version": 1,
        "client": "wasm",
        "savedAt": saved_at,
        "turn": state["turn"],
        "names": names,
        "game": game,
        "rulesVersion": legatum_core::RULES_VERSION,
    });
    if humans.len() > 1 {
        envelope["online"] = json!(true);
    }
    Ok(envelope.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use legatum_core::GameSetup;
    use legatum_wasm::session::{BrainKind, Session};

    fn save(humans: usize) -> String {
        let mut setup = GameSetup::all_ai(4);
        setup.players.truncate(3);
        for p in setup.players.iter_mut().take(humans) {
            p.is_human = true;
        }
        Session::new(&setup, |_| BrainKind::Scripted).save_json()
    }

    #[test]
    fn the_envelope_holds_the_exact_engine_save() {
        let game = save(1);
        let file: Value = serde_json::from_str(&app_save_file(&game).unwrap()).unwrap();
        assert_eq!(file["format"], "legatum-save");
        assert_eq!(file["version"], 1);
        assert_eq!(file["game"], game.as_str());
        assert_eq!(file["turn"], 1);
        assert_eq!(file["rulesVersion"], legatum_core::RULES_VERSION);
        assert_eq!(file["names"].as_array().unwrap().len(), 1);
        assert!(file.get("online").is_none());
    }

    #[test]
    fn a_game_with_two_agent_seats_is_an_online_save() {
        let file: Value = serde_json::from_str(&app_save_file(&save(2)).unwrap()).unwrap();
        assert_eq!(file["online"], true);
        assert_eq!(file["names"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn text_that_is_not_a_save_is_refused() {
        assert!(app_save_file("not json").is_err());
    }
}
