//! `harness-hook-goal-tokens` — live goal token accounting.
//!
//! Registered on the `model.after` window. After every model call,
//! recompute cumulative input+output token usage from the session's
//! `events.jsonl` and write it into the goal state file so the TUI
//! goal row shows a live figure (docs/goal-ux.md §1.6,
//! docs/auto-compact-plan.md section 4.6).
//!
//! This makes the goal's `used_tokens` real-time: it updates on every
//! model call, not just at the `run.idle` window. The `run.idle` hook
//! still performs the same recompute as a correction step.
//!
//! Observation window: no decision. The hook prints `{}` and exits 0.
//! The goal file is updated as a side effect.

use std::io::Read;

use rushi_goal_state::GoalState;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();

    // Not our window: no-op.
    if payload.get("window").and_then(|w| w.as_str()) != Some("model.after") {
        println!("{{}}");
        return;
    }

    let session_dir = match resolve_session_dir() {
        Some(d) => d,
        None => {
            println!("{{}}");
            return;
        }
    };

    // No goal file: nothing to update.
    let mut goal = match GoalState::load(&session_dir) {
        Some(g) => g,
        None => {
            println!("{{}}");
            return;
        }
    };

    // Recompute cumulative input+output from the log (same metric as
    // the statusline's `sum` figure). Idempotent: the log is
    // append-only, so re-reading is safe.
    let events_path = session_dir.join("events.jsonl");
    if let Some(total) = GoalState::sum_usage_tokens(&events_path) {
        goal.used_tokens = total;
        let _ = goal.save(&session_dir);
    }

    // Observation window: no decision.
    println!("{{}}");
}

fn read_stdin_json() -> serde_json::Value {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(&buf).unwrap_or(serde_json::json!({}))
}

/// Resolve the session directory from the hook env.
fn resolve_session_dir() -> Option<std::path::PathBuf> {
    let session = std::env::var("SESSION").ok()?;
    let sessions_root = std::env::var("SESSIONS_ROOT").unwrap_or_else(|_| "sessions".into());
    let p = if session.contains('/') || session.contains('\\') {
        std::path::PathBuf::from(&session)
    } else {
        std::path::PathBuf::from(&sessions_root).join(&session)
    };
    Some(p)
}

fn print_help() {
    println!("harness-hook-goal-tokens — live goal token accounting (model.after)");
    println!();
    println!("Window: model.after");
    println!("Input (stdin): window JSON with keys window, session, stop_reason, usage");
    println!("Output (stdout): {{}} (observation only, no decision)");
    println!("Side effect: rewrites the goal state file's used_tokens");
    println!("Exit codes: 0 = ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// No goal file: the hook is a no-op.
    #[test]
    fn test_no_goal_is_noop() {
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());
    }

    /// Active goal: recompute writes the new total.
    #[test]
    fn test_active_goal_updates_tokens() {
        let dir = TempDir::new().unwrap();
        let g = GoalState::new("live goal");
        g.save(dir.path()).unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::write(
            &events_path,
            concat!(
                "{\"v\":1,\"type\":\"assistant_message\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":200}}\n",
                "{\"v\":1,\"type\":\"assistant_message\",\"usage\":{\"input_tokens\":3000,\"output_tokens\":400}}\n",
            ),
        )
        .unwrap();

        // Simulate what main() does: recompute and save.
        let mut loaded = GoalState::load(dir.path()).unwrap();
        let total = GoalState::sum_usage_tokens(&events_path).unwrap();
        loaded.used_tokens = total;
        loaded.save(dir.path()).unwrap();

        let reloaded = GoalState::load(dir.path()).unwrap();
        // (1000+200) + (3000+400) = 4600
        assert_eq!(reloaded.used_tokens, 4600);
    }

    /// Closed goal: the hook still records tokens before the loop stops.
    #[test]
    fn test_closed_goal_still_records() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("done goal");
        g.mark_completed();
        g.save(dir.path()).unwrap();

        let events_path = dir.path().join("events.jsonl");
        std::fs::write(
            &events_path,
            "{\"v\":1,\"type\":\"assistant_message\",\"usage\":{\"input_tokens\":500,\"output_tokens\":100}}\n",
        )
        .unwrap();

        let mut loaded = GoalState::load(dir.path()).unwrap();
        assert!(!loaded.is_open());
        let total = GoalState::sum_usage_tokens(&events_path).unwrap();
        loaded.used_tokens = total;
        loaded.save(dir.path()).unwrap();

        let reloaded = GoalState::load(dir.path()).unwrap();
        assert_eq!(reloaded.used_tokens, 600);
    }
}
