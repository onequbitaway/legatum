//! Small `sh` scripts that stand in for agents. They need a Unix shell.
#![allow(dead_code)]

use legatum_agent::process::AgentCommand;

/// Answers hello with its name and each turn with no orders, for the turn it was sent.
pub const ECHO: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"hello"'*) echo '{"type":"hello","name":"echo"}' ;;
    *'"type":"turn"'*)
      t=$(printf '%s' "$line" | sed 's/^{"type":"turn","turn":\([0-9]*\).*/\1/')
      echo "{\"type\":\"orders\",\"turn\":$t,\"orders\":[]}" ;;
    *'"type":"end"'*) exit 0 ;;
  esac
done
"#;

/// As [`ECHO`], but every answer holds one order for a unit that does not exist.
pub const BAD_ORDER: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"hello"'*) echo '{"type":"hello","name":"bad order"}' ;;
    *'"type":"turn"'*)
      t=$(printf '%s' "$line" | sed 's/^{"type":"turn","turn":\([0-9]*\).*/\1/')
      echo "{\"type\":\"orders\",\"turn\":$t,\"orders\":[{\"type\":\"SkipUnit\",\"unit\":4000000}]}" ;;
    *'"type":"end"'*) exit 0 ;;
  esac
done
"#;

pub fn sh(script: &str) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        dir: None,
    }
}
