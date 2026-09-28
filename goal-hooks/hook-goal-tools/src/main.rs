//! `harness-hook-goal-tools` — the stale-goal-call guard.
//!
//! Registered on the `tool.before` window. It inspects the pending tool
//! batch and blocks goal-tool calls that are inconsistent with the
//! current goal state:
//! - `goal` when a goal is already open (double start).
//! - `goal_complete` / `goal_blocked` when there is no open goal.
//!
//! §12 pipeline ABI (rushi-goal-mode issue #5; kernel spec §12.5,
//! line 647): the retired §4.3 `{"decision":"block",...}` envelope is
//! gone. The state field is now `blocked_calls = [{id, reason}]`; the
//! kernel synthesizes a failed `tool_result` per entry and routes the
//! rest. State keys outside the window's effect set
//! (`blocked_calls`, `approval`) are logged once as
//! `hook.tool.before.unknown_fields` — which is exactly the marker that
//! used to fire for this hook's envelope in real sessions.
//!
//! §12.4: in a multi-step `tool.before` chain, a step's stdout
//! *replaces* the accumulated state, so a later step only sees what an
//! earlier step forwarded. The minimal migration emits
//! `blocked_calls` only (matching the committed `no-find-grep`
//! behavior).

use std::io::Read;

use rushi_goal_state::GoalState;

fn main() {
    // Self-documentation.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();

    // §12 window dispatch: the `HARNESS_WINDOW` env var (kernel-
    // injected) is authoritative; fall back to the payload's `window`
    // field for manual invocation.
    if !window_is(&payload, "tool.before") {
        println!("{{}}");
        return;
    }

    let calls = payload
        .get("calls")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let session_dir = match resolve_session_dir() {
        Some(d) => d,
        None => {
            println!("{{}}");
            return;
        }
    };

    let goal = GoalState::load(&session_dir);

    // Collect the ids of the calls to block, with a reason each.
    let blocked = blocked_calls(&calls, goal.as_ref());

    if blocked.is_empty() {
        println!("{{}}");
        return;
    }

    // §12.5: the state field is `blocked_calls = [{id, reason}]`. The
    // kernel (step/tool.rs `route_and_append`) synthesizes a failed
    // `tool_result` per entry and routes the rest.
    println!("{}", serde_json::json!({ "blocked_calls": blocked }));
}

/// The stale-goal-call decision (issue #5): which of the pending calls
/// are inconsistent with the goal state, each with a guidance reason.
///
/// - `goal` while a goal is open → double-start, block it.
/// - `goal_complete` / `goal_blocked` with no open goal → block it.
///
/// Pure function of `(calls, goal state)` so the guard is testable
/// without a session directory.
fn blocked_calls(
    calls: &[serde_json::Value],
    goal: Option<&GoalState>,
) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    for call in calls {
        let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let id = call.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
        match name {
            "goal" => {
                if let Some(g) = goal {
                    if g.is_open() {
                        out.push(serde_json::json!({
                            "id": id,
                            "reason": format!(
                                "A goal is already open: \"{}\". Use goal_complete or goal_blocked to close it before starting a new goal.",
                                g.goal
                            ),
                        }));
                    }
                }
            }
            "goal_complete" | "goal_blocked" => {
                match goal {
                    None => out.push(serde_json::json!({
                        "id": id,
                        "reason": format!(
                            "{name} failed: no goal is open. Start one with the `goal` tool first."
                        ),
                    })),
                    Some(g) if !g.is_open() => out.push(serde_json::json!({
                        "id": id,
                        "reason": format!(
                            "{name} failed: the goal \"{}\" is already {}.",
                            g.goal,
                            if g.blocked { "blocked" } else { "complete" }
                        ),
                    })),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// §12 window dispatch: the `HARNESS_WINDOW` env var (kernel-injected)
/// is authoritative; when unset, fall back to the payload's `window`
/// field so manual invocations keep working.
fn window_is(payload: &serde_json::Value, want: &str) -> bool {
    if let Ok(env_w) = std::env::var("HARNESS_WINDOW") {
        if !env_w.is_empty() {
            return env_w == want;
        }
    }
    payload.get("window").and_then(|w| w.as_str()) == Some(want)
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
    println!("harness-hook-goal-tools — stale goal-call guard (tool.before)");
    println!();
    println!("Window: tool.before (dispatch: $HARNESS_WINDOW, else payload window)");
    println!("Input (stdin): window JSON with keys window, session, calls[]");
    println!("Output (stdout):");
    println!("  {{}}  — proceed with the batch");
    println!(
        "  {{\"blocked_calls\":[{{\"id\":...,\"reason\":...}}]}} \
         — §12.5 state field: the kernel synthesizes a failed tool_result per entry"
    );
    println!("Exit codes: 0 = ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Double-start: a `goal` call while a goal is open is blocked with
    /// guidance to close the open goal first.
    #[test]
    fn double_start_blocked() {
        let dir = TempDir::new().unwrap();
        let g = GoalState::new("open goal");
        g.save(dir.path()).unwrap();
        let goal = GoalState::load(dir.path()).unwrap();

        let calls = vec![json!({"id": "c1", "name": "goal", "args": {}})];
        let blocked = blocked_calls(&calls, Some(&goal));
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0]["id"], "c1");
        assert!(blocked[0]["reason"]
            .as_str()
            .unwrap()
            .contains("already open"));
    }

    /// `goal` when no goal exists (or the goal is closed) passes
    /// through unblocked.
    #[test]
    fn start_without_goal_passes() {
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        let calls = vec![json!({"id": "c1", "name": "goal", "args": {}})];
        let blocked = blocked_calls(&calls, None);
        assert!(blocked.is_empty());
    }

    /// `goal_complete` with no open goal is blocked with guidance.
    #[test]
    fn close_without_open_goal_blocked() {
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        let calls = vec![
            json!({"id": "c2", "name": "goal_complete", "args": {}}),
            json!({"id": "c3", "name": "goal_blocked", "args": {}}),
        ];
        let blocked = blocked_calls(&calls, None);
        assert_eq!(blocked.len(), 2);
        assert!(blocked[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no goal is open"));
    }

    /// A completed goal: `goal_blocked` is blocked (goal is no longer
    /// open), with the closed-state reason.
    #[test]
    fn close_on_completed_goal_blocked() {
        let dir = TempDir::new().unwrap();
        let mut g = GoalState::new("done goal");
        g.mark_completed();
        g.save(dir.path()).unwrap();
        let goal = GoalState::load(dir.path()).unwrap();

        let calls = vec![json!({"id": "c4", "name": "goal_complete", "args": {}})];
        let blocked = blocked_calls(&calls, Some(&goal));
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0]["reason"]
            .as_str()
            .unwrap()
            .contains("already complete"));
    }

    /// An open goal: the close tools are allowed through (not blocked).
    #[test]
    fn close_while_open_passes() {
        let dir = TempDir::new().unwrap();
        let g = GoalState::new("open goal");
        g.save(dir.path()).unwrap();
        let goal = GoalState::load(dir.path()).unwrap();

        let calls = vec![
            json!({"id": "c5", "name": "goal_complete", "args": {}}),
            json!({"id": "c6", "name": "goal_blocked", "args": {}}),
        ];
        let blocked = blocked_calls(&calls, Some(&goal));
        assert!(blocked.is_empty(), "close while open must pass through");
    }

    /// Non-goal tools are never touched by the guard.
    #[test]
    fn non_goal_calls_pass_through() {
        let dir = TempDir::new().unwrap();
        let g = GoalState::new("open goal");
        g.save(dir.path()).unwrap();
        let goal = GoalState::load(dir.path()).unwrap();

        let calls = vec![
            json!({"id": "c7", "name": "read", "args": {}}),
            json!({"id": "c8", "name": "bash", "args": {}}),
        ];
        let blocked = blocked_calls(&calls, Some(&goal));
        assert!(blocked.is_empty());
    }

    /// §12.5: the emission is the `blocked_calls` state field — no
    /// decision envelope, no joined reason string. This mirrors what
    /// main() prints when calls are stale.
    #[test]
    fn blocked_emission_is_the_state_field() {
        let dir = TempDir::new().unwrap();
        let g = GoalState::new("open goal");
        g.save(dir.path()).unwrap();
        let goal = GoalState::load(dir.path()).unwrap();

        let calls = vec![json!({"id": "c1", "name": "goal", "args": {}})];
        let blocked = blocked_calls(&calls, Some(&goal));
        let out = json!({ "blocked_calls": blocked });
        assert!(out.get("decision").is_none(), "no decision envelope");
        assert!(out.get("payload").is_none(), "no payload envelope");
        assert_eq!(out["blocked_calls"][0]["id"], "c1");
        assert!(out["blocked_calls"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("already open"));
    }
}
