//! `harness-hook-goal-idle` — the goal-continuation hook.
//!
//! Registered on the `run.idle` window. When the loop is about to stop
//! on an idle claim, this hook checks whether a goal is still open.
//!
//! §12 pipeline ABI (rushi-goal-mode issue #5; kernel spec §12.5,
//! `docs/loop-lifecycle-hooks.md:649`): the retired §4.3 `continue`
//! decision envelope is gone. Instead the hook appends its own follow-up
//! `user_message` to the session log via `LOG_BIN`; the loop then
//! re-claims and stays alive iff the re-claim has `pending_follow_ups`
//! or state ≠ idle. So the *append* is the mechanism that keeps the loop
//! going — not a returned `decision: continue`.
//!
//! Termination comes from user controls (§1.6):
//!   - `goal_complete` closes the goal
//!   - `goal_blocked` closes the goal
//!   - `goal pause` / `goal clear` (TUI) stop the loop
//!
//! Refire guarantee (rushi-goal-mode issue #1): a terminal goal
//! (completed, blocked, or paused) appends *nothing*, so the loop stops
//! cleanly and the kernel runs no further model turns. An open goal
//! appends a logged continuation `user_message` — the goal-mode
//! mechanism for keeping the loop alive — never a silent `refire`.
//!
//! Contract (spec §12.5):
//! - terminal or absent goal → append nothing, print `{}` (noop), exit 0.
//! - open goal → increment the iteration counter *before* building the
//!   continuation prompt, then append one follow-queue `user_message`
//!   via `"$LOG_BIN" --session "$SESSION"` (event JSON on stdin). On any
//!   spawn/write/exit failure → print `{"reason": ...}` and exit 3: the
//!   chain stops, the window default (`stop`) applies, and a
//!   `hook.run.idle.error` marker is logged carrying that reason.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use rushi_goal_state::GoalState;

fn main() {
    // Self-documentation.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();

    // §12 window dispatch: the `HARNESS_WINDOW` env var (kernel-injected)
    // is authoritative; fall back to the payload's `window` field for
    // manual invocation (a shell with no kernel env).
    if !window_is(&payload, "run.idle") {
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
    // assign `used_tokens` to the cumulative input+output total across
    // all `assistant_message` and `compaction_summary` events in the log.
    // Runs *before* the `is_open()` check so the final turn's tokens are
    // captured even when the goal just closed.
    let events_path = session_dir.join("events.jsonl");
    if let Some(total) = GoalState::sum_usage_tokens(&events_path) {
        goal.used_tokens = total;
    }

    // A completed, blocked, or paused goal appends nothing: the loop
    // stops (issue #1 — no continuation, no refire).
    if !should_continue(&goal) {
        let _ = goal.save(&session_dir);
        println!("{{}}");
        return;
    }

    // Open goal: increment the counter *before* building the prompt,
    // persist it, then append the follow-queue `user_message` that keeps
    // the loop going. The log append is what the kernel's re-claim sees
    // as a pending follow-up.
    goal.iteration += 1;
    let _ = goal.save(&session_dir);

    let log_bin = match std::env::var("LOG_BIN") {
        Ok(b) if !b.is_empty() => b,
        _ => {
            fail("LOG_BIN is not set; cannot append the continuation user_message");
        }
    };
    let session = session_dir.to_string_lossy().into_owned();
    if let Err(reason) = append_continuation(&goal, &log_bin, &session) {
        fail(&format!("LOG_BIN append failed: {reason}"));
    }

    // Success: noop stdout. The kernel re-claims and finds the follow-up.
    println!("{{}}");
}

/// Append the goal's continuation prompt to the session log as a
/// follow-queue `user_message` (§12.5). The event line is written to
/// `LOG_BIN`'s stdin; its exit status is the append result.
fn append_continuation(
    goal: &GoalState,
    log_bin: &str,
    session: &str,
) -> Result<(), String> {
    let event = continuation_event(
        goal,
        &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    let mut child = Command::new(log_bin)
        .arg("--session")
        .arg(session)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {log_bin:?} failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = writeln!(stdin, "{event}") {
            return Err(format!("write to {log_bin} stdin failed: {e}"));
        }
    }
    match child.wait().map_err(|e| format!("wait on {log_bin} failed: {e}"))? {
        s if s.success() => Ok(()),
        s => Err(format!("{log_bin} exited {s}")),
    }
}

/// The §12.5 continuation event: a follow-queue `user_message` carrying
/// the goal's continuation prompt. Pure function of `(goal, ts)` so the
/// shape is testable without spawning `LOG_BIN`.
fn continuation_event(goal: &GoalState, ts: &str) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "type": "user_message",
        "ts": ts,
        "content": goal.build_continue_prompt(),
        "queue": "follow",
    })
}

/// Whether the goal warrants a continuation append (an open goal).
/// Terminal goals append nothing (issue #1).
fn should_continue(goal: &GoalState) -> bool {
    goal.is_open()
}

/// §12 window dispatch. The `HARNESS_WINDOW` env var is authoritative
/// (the kernel sets it per firing); when it is unset (manual
/// invocation from a shell), fall back to the payload's `window` field.
/// The model.before request object carries no `window` key, so the env
/// is the only source there; for run.idle the payload also has one,
/// which keeps manual stdin invocation working.
fn window_is(payload: &serde_json::Value, want: &str) -> bool {
    let env_window = std::env::var("HARNESS_WINDOW");
    let effective = match &env_window {
        Ok(w) if !w.is_empty() => w.as_str(),
        _ => payload.get("window").and_then(|w| w.as_str()).unwrap_or(""),
    };
    effective == want
}

/// Exit-3 failure path: the kernel's `step` status is `fail` (P4), which
/// stops the chain, applies the window default (`stop`), and logs a
/// `hook.run.idle.error` marker carrying the printed `reason`.
fn fail(reason: &str) -> ! {
    println!("{}", serde_json::json!({ "reason": reason }));
    eprintln!("harness-hook-goal-idle: {reason}");
    std::process::exit(3);
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
    println!("Window: run.idle (dispatch: $HARNESS_WINDOW, else payload window)");
    println!("Input (stdin): window JSON; $LOG_BIN and $SESSION are the append targets");
    println!("Output (stdout): {{}} — no goal, terminal goal, or continuation appended");
    println!("Side effect: an open goal appends one follow-queue user_message");
    println!("  to the session log via LOG_BIN (keeps the loop alive, §12.5).");
    println!("Exit codes: 0 = ok; 3 = could not append (chain stops, loop stops)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// P10: active goal → the continuation prompt names the counter and
    /// goal. (Prompt shape is unchanged by the §12 migration.)
    #[test]
    fn test_active_goal_continues() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("fix the bug");
        g.iteration = 3;
        g.save(dir.path()).unwrap();

        let mut loaded = GoalState::load(dir.path()).unwrap();
        assert!(loaded.is_open());
        assert!(should_continue(&loaded));

        loaded.iteration += 1;
        let prompt = loaded.build_continue_prompt();
        assert!(prompt.contains("continuation #4"), "{prompt}");
        assert!(prompt.contains("fix the bug"), "{prompt}");
    }

    /// P11: no active goal / paused goal → no continuation.
    #[test]
    fn test_no_active_goal_stops() {
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        let mut g = GoalState::new("test");
        g.active = false;
        g.save(dir.path()).unwrap();
        let loaded = GoalState::load(dir.path()).unwrap();
        assert!(!loaded.is_open());
        assert!(!should_continue(&loaded));
    }

    /// No budget cap: even with a very high `used_tokens`, the open goal
    /// continues.
    #[test]
    fn test_no_budget_stop() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("big goal");
        g.used_tokens = 999_999_999;
        g.iteration = 100;
        g.save(dir.path()).unwrap();

        let mut loaded = GoalState::load(dir.path()).unwrap();
        assert!(loaded.is_open());
        loaded.iteration += 1;
        let prompt = loaded.build_continue_prompt();
        assert!(prompt.contains("continuation #101"), "{prompt}");
    }

    /// A closed goal still records cumulative token usage from the log
    /// before the loop stops.
    #[test]
    fn test_closed_goal_still_gets_token_accounting() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("short goal");
        g.mark_completed();
        g.save(dir.path()).unwrap();

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

        let mut loaded = GoalState::load(dir.path()).unwrap();
        assert!(!loaded.is_open(), "goal should be closed");
        let total = GoalState::sum_usage_tokens(&events_path);
        loaded.used_tokens = total.unwrap();
        loaded.save(dir.path()).unwrap();

        let reloaded = GoalState::load(dir.path()).unwrap();
        // (100+42) + (5000+800) + (100+58) = 6100.
        assert_eq!(reloaded.used_tokens, 6100);
        assert!(reloaded.completed);
    }

    // ── issue #1: terminal goals must not append or refire ──

    /// A completed goal appends nothing.
    #[test]
    fn issue1_completed_goal_appends_nothing() {
        let mut g = GoalState::new("done");
        g.mark_completed();
        assert!(!should_continue(&g));
    }

    /// A blocked goal appends nothing.
    #[test]
    fn issue1_blocked_goal_appends_nothing() {
        let mut g = GoalState::new("stuck");
        g.mark_blocked("missing dependency");
        assert!(!should_continue(&g));
    }

    /// A paused goal (active == false) appends nothing.
    #[test]
    fn issue1_paused_goal_appends_nothing() {
        let mut g = GoalState::new("paused");
        g.active = false;
        assert!(!should_continue(&g));
    }

    /// An open goal's continuation event is a follow-queue
    /// `user_message` carrying the prompt, with no §4.3 `decision`/
    /// `refire` envelope fields (the §12 append *is* the mechanism).
    #[test]
    fn issue1_open_goal_event_shape() {
        let mut g = GoalState::new("keep working");
        g.id = "g-00c0ffee".to_string();
        g.iteration = 3;
        g.iteration += 1; // main() increments before building.
        let ev = continuation_event(&g, "2025-01-01T00:00:00Z");
        assert_eq!(ev["v"], 1);
        assert_eq!(ev["type"], "user_message");
        assert_eq!(ev["ts"], "2025-01-01T00:00:00Z");
        assert_eq!(ev["queue"], "follow");
        let msg = ev["content"].as_str().unwrap();
        assert!(msg.contains("continuation #4"), "{msg}");
        assert!(msg.contains("keep working"), "{msg}");
        assert!(msg.contains("g-00c0ffee"), "{msg}");
        // No §4.3 envelope fields anywhere in the event.
        assert!(ev.get("decision").is_none());
        assert!(ev.get("refire").is_none());
        assert!(ev.get("payload").is_none());
    }

    /// A failing `LOG_BIN` (non-existent path) is a hard append error,
    /// exercising the exit-3 path without the real log binary.
    #[test]
    fn append_continuation_fails_on_bad_log_bin() {
        let g = GoalState::new("x");
        let err = append_continuation(&g, "/nonexistent/log-bin-xyz", "s").unwrap_err();
        assert!(err.contains("/nonexistent/log-bin-xyz"), "{err}");
    }
}
