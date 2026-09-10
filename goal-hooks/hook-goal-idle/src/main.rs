//! `harness-hook-goal-idle` — the goal-continuation hook.
//!
//! Registered on the `run.idle` window. When the loop is about to stop
//! on an idle claim, this hook checks whether a goal is still open.
//! If the goal is `active` and not `blocked`/`completed`, it
//! increments the iteration counter, updates token accounting
//! (informational only — no budget cap, docs/goal-ux.md §1.6),
//! and returns a `continue` decision with a continuation prompt so
//! the loop keeps working toward the goal.
//!
//! Termination comes from user controls (§1.6):
//!   - `goal_complete` closes the goal
//!   - `goal_blocked` closes the goal
//!   - `goal pause` / `goal clear` (TUI) stop the loop
//!
//! Decision contract (docs/loop-lifecycle-hooks.md §4.3):
//! - exit 0 + `{}` → no decision, the loop stops (window default)
//! - exit 0 + `{"decision":"continue","payload":{"message":"..."}}`
//!   → the loop appends a follow `user_message` and continues.

use std::io::Read;

use goal_state::GoalState;

fn main() {
    // Self-documentation.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();

    // Not our window: no-op.
    if payload.get("window").and_then(|w| w.as_str()) != Some("run.idle") {
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

    // No goal file: nothing to continue.
    let mut goal = match GoalState::load(&session_dir) {
        Some(g) => g,
        None => {
            println!("{{}}");
            return;
        }
    };

    // Token accounting (informational only, docs/goal-ux.md §1.6):
    // set used_tokens to the cumulative output_tokens total across all
    // assistant_message and compaction_summary events in the log.
    // This is idempotent and compaction-robust: the log is
    // append-only, so re-reading after a compact still covers the
    // full session history. Placed before the is_open check so the
    // final turn's tokens are captured even when the goal just closed.
    let events_path = session_dir.join("events.jsonl");
    if let Some(total) = GoalState::sum_assistant_output_tokens(&events_path) {
        goal.used_tokens = total;
    }

    // A completed, blocked, or paused goal stops the loop.
    if !goal.is_open() {
        let _ = goal.save(&session_dir);
        println!("{{}}");
        return;
    }

    // Increment the continuation counter (docs/goal-ux.md §2: used
    // only by the logged continuation message and the TUI display,
    // never by the injected goal block).
    goal.iteration += 1;

    let _ = goal.save(&session_dir);

    let prompt = goal.build_continue_prompt();
    let resp = serde_json::json!({
        "decision": "continue",
        "payload": {
            "message": prompt,
        },
    });
    println!("{}", resp);
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
    println!("harness-hook-goal-idle — goal-continuation hook (run.idle)");
    println!();
    println!("Window: run.idle");
    println!("Input (stdin): window JSON with keys window, session, last_assistant_message_id");
    println!("Output (stdout):");
    println!("  {{}}  — stop the loop (no goal, goal not active, or goal closed)");
    println!("  {{\"decision\":\"continue\",\"payload\":{{\"message\":\"...\"}}}} — keep going");
    println!("Exit codes: 0 = ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// P10: active goal → continue with continuation prompt.
    #[test]
    fn test_active_goal_continues() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("fix the bug");
        g.iteration = 3;
        g.save(dir.path()).unwrap();

        let loaded = GoalState::load(dir.path()).unwrap();
        assert!(loaded.is_open());

        // Simulate what main() does: increment, save, build prompt.
        let mut g2 = loaded;
        g2.iteration += 1;
        let prompt = g2.build_continue_prompt();
        assert!(prompt.contains("continuation #4"), "{prompt}");
        assert!(prompt.contains("fix the bug"), "{prompt}");
    }

    /// P11: no active goal → stop.
    #[test]
    fn test_no_active_goal_stops() {
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        // Also test a paused goal (active = false).
        let mut g = GoalState::new("test");
        g.active = false;
        g.save(dir.path()).unwrap();
        let loaded = GoalState::load(dir.path()).unwrap();
        assert!(!loaded.is_open());
    }

    #[test]
    fn test_no_budget_stop() {
        // P10: even with a very high used_tokens, the goal continues.
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("big goal");
        g.used_tokens = 999_999_999;
        g.iteration = 100;
        g.save(dir.path()).unwrap();

        let loaded = GoalState::load(dir.path()).unwrap();
        assert!(loaded.is_open());
        // No budget_exhausted() method anymore — the loop never stops
        // on token count.
        let mut g2 = loaded;
        g2.iteration += 1;
        let prompt = g2.build_continue_prompt();
        assert!(prompt.contains("continuation #101"), "{prompt}");
    }

    /// A closed goal (completed/blocked) still records cumulative
    /// token usage from the log before the loop stops.
    /// This ensures used_tokens is recorded even when the goal closes
    /// on the very turn the idle hook fires.
    #[test]
    fn test_closed_goal_still_gets_token_accounting() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("short goal");
        g.mark_completed();
        g.save(dir.path()).unwrap();

        // Simulate assistant messages and a compaction summary with
        // usage data in the log.
        let events_path = dir.path().join("events.jsonl");
        std::fs::write(
            &events_path,
            concat!(
                "{\"v\":1,\"type\":\"assistant_message\",\"content\":\"work\",\"stop_reason\":\"stop\",\"usage\":{\"input_tokens\":100,\"output_tokens\":42}}\n",
                "{\"v\":1,\"type\":\"compaction_summary\",\"ts\":\"t2\",\"usage\":{\"input_tokens\":5000,\"output_tokens\":800}}\n",
                "{\"v\":1,\"type\":\"assistant_message\",\"content\":\"done\",\"stop_reason\":\"stop\",\"usage\":{\"input_tokens\":100,\"output_tokens\":58}}\n",
            ),
        )
        .unwrap();

        // Simulate what main() does:
        // token accounting runs BEFORE the is_open() check.
        let mut loaded = GoalState::load(dir.path()).unwrap();
        assert!(!loaded.is_open(), "goal should be closed");
        assert_eq!(loaded.used_tokens, 0, "no tokens yet");

        // The sum of all output_tokens: 42 + 800 + 58 = 900.
        let total = GoalState::sum_assistant_output_tokens(&events_path);
        assert_eq!(total, Some(900));
        loaded.used_tokens = total.unwrap();

        // Save and verify the token count was recorded despite the
        // goal being closed.
        loaded.save(dir.path()).unwrap();
        let reloaded = GoalState::load(dir.path()).unwrap();
        assert_eq!(
            reloaded.used_tokens, 900,
            "closed goal should still record cumulative tokens"
        );
        assert!(reloaded.completed);
    }
}
