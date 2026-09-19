//! `harness-hook-goal-arm` — goal-mode prompt fragment + tool filter.
//!
//! Registered on the `model.before` window. Reads the session's
//! current goal (the `goal.json` pointer plus its `goal-<id>.json`
//! state file) from the session directory.
//!
//! Two effects, both idempotent transforms of `request`
//! (docs/system-prompt-generation.md D5):
//!
//! 1. **Prompt fragment.** The cache-stable goal fragment
//!    (`rushi_goal_state::GoalState::build_goal_fragment`, a pure function
//!    of `(goal, goal_id)`) is set under `request.prompt_fragments`
//!    as the ordered `[id, text]` pair `["goal", <block>]`. The
//!    kernel joins all fragments into `request.instructions` after
//!    the hook chain and strips the field before the model call.
//!    This hook only ever touches its own `"goal"` key; fragments
//!    owned by other extensions are preserved in order.
//!
//! 2. **Tool filter (D1).** The `goal` schema is removed from
//!    `request.tools` in every state. The `goal_complete` /
//!    `goal_blocked` schemas are present only while the goal is open
//!    (`active && !completed && !blocked`); they are removed once
//!    the goal closes.
//!
//! The transform runs on every model call while the goal extension
//! is installed (steady-state tool filter), so the emitted request is
//! a pure function of (base request, goal state) and stays
//! byte-identical turn to turn (provider prefix cache stays warm).
//!
//! Fragment lifetime (docs/system-prompt-generation.md D6):
//! - no goal → no fragment
//! - goal open **or blocked** → fragment present (`goal_blocked`
//!   keeps the goal open; its state is blocked, not completed)
//! - `goal_complete` (or `goal clear`) → `completed`/pointer gone,
//!   the fragment is removed on the next model call
//!
//! Decision contract (docs/loop-lifecycle-hooks.md §4.3):
//! - exit 0 + `{}` → proceed unchanged (nothing goal-related in the
//!   request and no goal state to project).
//! - exit 0 + `{"decision":"transform","payload":{"request":{...}}}`
//!   → the harness replaces the request with the hook's version,
//!   joins `prompt_fragments` into `instructions`, and strips the
//!   field.

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

    // Not our window: no-op.
    if payload.get("window").and_then(|w| w.as_str()) != Some("model.before") {
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

    // Read the session's current goal — the sole source of truth for
    // goal state (docs/goal-ux.md §1.1b: goal-file-driven, not
    // log-derived).
    let goal = GoalState::load(&session_dir);

    let request = payload
        .get("request")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // No goal state, no goal tool schema, no stale "goal" fragment:
    // nothing for this hook to do. Emitting `{}` keeps the kernel's
    // transform log quiet for sessions without the goal extension.
    if !needs_transform(&request, goal.as_ref()) {
        println!("{{}}");
        return;
    }

    let req = apply_goal_transform(request, goal.as_ref());
    let resp = serde_json::json!({
        "decision": "transform",
        "payload": {
            "request": req,
        },
    });
    println!("{}", resp);
}

/// Whether this hook must transform the request at all. The tool
/// filter has to run on every call while any goal tool schema is
/// visible to the model (steady state), so a request carrying
/// `goal` / `goal_complete` / `goal_blocked` always transforms. The
/// fragment is managed whenever a goal state file exists and is not
/// completed (open **or** blocked, D6), or a stale `"goal"` fragment
/// needs removing.
fn needs_transform(request: &serde_json::Value, goal: Option<&GoalState>) -> bool {
    if goal.map_or(false, |g| !g.completed) {
        return true;
    }
    if has_goal_tools(request) || has_goal_fragment(request) {
        return true;
    }
    false
}

/// D1: the `goal` tool is invisible to the agent in every state, and
/// the two close tools are present only while a goal is open.
fn has_goal_tools(request: &serde_json::Value) -> bool {
    request
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter().any(|t| {
                matches!(
                    t.get("name").and_then(|n| n.as_str()),
                    Some("goal") | Some("goal_complete") | Some("goal_blocked")
                )
            })
        })
        .unwrap_or(false)
}

fn has_goal_fragment(request: &serde_json::Value) -> bool {
    request
        .get("prompt_fragments")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .any(|p| p.get(0).and_then(|i| i.as_str()) == Some("goal"))
        })
        .unwrap_or(false)
}

/// The idempotent goal transform (docs/system-prompt-generation.md).
///
/// 1. Filters `request.tools` per D1.
/// 2. Sets or removes the `"goal"` entry of the ordered
///    `request.prompt_fragments` array; other extensions' keys pass
///    through untouched, in their original order.
///
/// Pure function of (request, goal state): a second call with the
/// same inputs re-emits byte-identical request JSON (P17).
fn apply_goal_transform(
    mut req: serde_json::Value,
    goal: Option<&GoalState>,
) -> serde_json::Value {
    let goal_open = goal.map_or(false, |g| g.is_open());

    // 1. Tool filter (D1): drop `goal` always; keep the close tools
    //    only while the goal is open.
    if let Some(tools) = req.get_mut("tools").and_then(|t| t.as_array_mut()) {
        tools.retain(|t| match t.get("name").and_then(|n| n.as_str()) {
            Some("goal") => false,
            Some("goal_complete") | Some("goal_blocked") => goal_open,
            _ => true,
        });
    }

    // 2. The "goal" fragment: present while the goal exists and is
    //    not completed (open or blocked, D6); removed otherwise.
    let mut frags: Vec<serde_json::Value> = req
        .get("prompt_fragments")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    frags.retain(|p| p.get(0).and_then(|i| i.as_str()) != Some("goal"));
    if let Some(g) = goal {
        if !g.completed {
            frags.push(serde_json::json!(["goal", g.build_goal_fragment()]));
        }
    }
    if frags.is_empty() {
        if let Some(obj) = req.as_object_mut() {
            obj.remove("prompt_fragments");
        }
    } else {
        req["prompt_fragments"] = serde_json::json!(frags);
    }

    req
}

/// Resolve the session directory from the hook env.
fn resolve_session_dir() -> Option<std::path::PathBuf> {
    let session = std::env::var("SESSION").ok()?;
    let sessions_root = std::env::var("SESSIONS_ROOT").unwrap_or_else(|_| "sessions".into());
    Some(
        if session.contains('/') || session.contains('\\') {
            std::path::PathBuf::from(&session)
        } else {
            std::path::PathBuf::from(&sessions_root).join(&session)
        },
    )
}

fn read_stdin_json() -> serde_json::Value {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(&buf).unwrap_or(serde_json::json!({}))
}

fn print_help() {
    println!("harness-hook-goal-arm — goal-mode prompt fragment + tool filter (model.before)");
    println!();
    println!("Window: model.before");
    println!("Input (stdin): {{window, session, model, projected_tokens, request}}");
    println!("Output (stdout):");
    println!("  {{}}  — nothing goal-related; proceed unchanged");
    println!(
        "  {{\"decision\":\"transform\",\"payload\":{{\"request\":{{...}}}}}} \
         — prompt_fragments[\"goal\"] set/removed per goal state; goal tool \
         schemas filtered per D1. The kernel joins prompt_fragments into \
         instructions and strips the field."
    );
    println!("Exit codes: 0 = ok");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_active_goal(dir: &std::path::Path) -> GoalState {
        let g = GoalState::new("fix the parser bug");
        let mut g = g;
        g.iteration = 3;
        g.used_tokens = 50_000;
        g.save(dir).unwrap();
        g
    }

    fn req_with_tools() -> serde_json::Value {
        json!({
            "instructions": "base system prompt",
            "input": [
                {"type": "message", "role": "user", "content": "hello"}
            ],
            "tools": [
                {"type": "function", "name": "read", "description": "read", "parameters": {}},
                {"type": "function", "name": "goal", "description": "start", "parameters": {}},
                {"type": "function", "name": "goal_complete", "description": "close", "parameters": {}},
                {"type": "function", "name": "goal_blocked", "description": "block", "parameters": {}},
            ]
        })
    }

    #[test]
    fn test_open_goal_sets_fragment_and_filters_tools() {
        // P6 (rewritten for prompt_fragments, docs/system-prompt-generation.md):
        // active goal → the fenced fragment rides request.prompt_fragments,
        // the `goal` schema is dropped, the close tools stay.
        let dir = TempDir::new().unwrap();
        let goal = make_active_goal(dir.path());

        let req = apply_goal_transform(req_with_tools(), Some(&goal));
        assert!(goal.is_open());

        // The fragment is the sole entry, carrying the fenced block.
        let frags = req["prompt_fragments"].as_array().unwrap();
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0][0].as_str().unwrap(), "goal");
        let block = frags[0][1].as_str().unwrap().to_string();
        assert!(block.contains("fix the parser bug"));
        assert!(block.contains("<goal_instructions>"));
        assert!(block.contains("<goal_objective>"));
        assert!(block.contains("There is no token budget"));

        // The goal tool is filtered; the close tools remain.
        let tool_names: Vec<&str> = req["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(tool_names, vec!["read", "goal_complete", "goal_blocked"]);

        // The conversation input is untouched — no trailing item.
        assert_eq!(req["input"].as_array().unwrap().len(), 1);
        assert_eq!(req["instructions"].as_str().unwrap(), "base system prompt");
    }

    #[test]
    fn test_closed_goal_strips_fragment_and_close_tools() {
        // goal_complete marks the goal completed: the next model call
        // drops the fragment and the close schemas (D6).
        let dir = TempDir::new().unwrap();
        let mut goal = make_active_goal(dir.path());
        goal.mark_completed();
        goal.save(dir.path()).unwrap();

        let req = apply_goal_transform(req_with_tools(), Some(&goal));
        assert!(req.get("prompt_fragments").is_none(), "no fragment after completion");
        let tool_names: Vec<&str> = req["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(tool_names, vec!["read"]);
    }

    #[test]
    fn test_blocked_goal_keeps_fragment() {
        // D6: goal_blocked keeps the goal open (state blocked, not
        // completed) — the fragment stays; the close schemas are
        // removed because the goal is no longer open.
        let dir = TempDir::new().unwrap();
        let mut goal = make_active_goal(dir.path());
        goal.mark_blocked("missing dep");
        goal.save(dir.path()).unwrap();

        let req = apply_goal_transform(req_with_tools(), Some(&goal));
        let frags = req["prompt_fragments"].as_array().unwrap();
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0][0].as_str().unwrap(), "goal");

        let tool_names: Vec<&str> = req["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(tool_names, vec!["read"]);
    }

    #[test]
    fn test_no_goal_removes_stale_fragment() {
        // P15: no goal state → any stale "goal" fragment is removed,
        // goal tools filtered.
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        let mut req = req_with_tools();
        req["prompt_fragments"] = json!([["other-ext", "their text"], ["goal", "stale"]]);
        let out = apply_goal_transform(req, None);
        let frags = out["prompt_fragments"].as_array().unwrap();
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0][0].as_str().unwrap(), "other-ext");
        assert_eq!(frags[0][1].as_str().unwrap(), "their text");
    }

    #[test]
    fn test_other_extensions_fragments_pass_through() {
        // Cross-extension contract: foreign keys keep their order;
        // the "goal" key is re-appended at the end.
        let dir = TempDir::new().unwrap();
        let goal = make_active_goal(dir.path());

        let mut req = req_with_tools();
        req["prompt_fragments"] = json!([["style", "be terse"]]);
        let out = apply_goal_transform(req, Some(&goal));
        let frags = out["prompt_fragments"].as_array().unwrap();
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0][0].as_str().unwrap(), "style");
        assert_eq!(frags[1][0].as_str().unwrap(), "goal");
    }

    #[test]
    fn test_transform_is_idempotent_byte_stable() {
        // P17: applying the transform twice (steady state, goal open)
        // yields byte-identical request JSON.
        let dir = TempDir::new().unwrap();
        let goal = make_active_goal(dir.path());
        let once = apply_goal_transform(req_with_tools(), Some(&goal));
        let twice = apply_goal_transform(once.clone(), Some(&goal));
        assert_eq!(
            serde_json::to_string(&once).unwrap(),
            serde_json::to_string(&twice).unwrap(),
            "steady-state transforms must be byte-identical"
        );
    }

    #[test]
    fn test_fragment_byte_stable_across_turns() {
        // P17: consecutive calls with unchanged (goal, id) emit
        // identical fragment bytes.
        let mut g = GoalState::new("implement the feature");
        g.iteration = 0;
        let b1 = g.build_goal_fragment();

        g.iteration = 1;
        g.add_used(1000);
        let b2 = g.build_goal_fragment();

        g.iteration = 2;
        g.add_used(2000);
        let b3 = g.build_goal_fragment();

        assert_eq!(b1, b2, "iteration/used_tokens must not leak into the fragment");
        assert_eq!(b2, b3);
    }

    #[test]
    fn test_noop_when_nothing_goal_related() {
        // No goal state and no goal tool schema: the hook emits {}
        // (main's needs_transform path).
        let dir = TempDir::new().unwrap();
        assert!(GoalState::load(dir.path()).is_none());

        let req = json!({
            "instructions": "base",
            "tools": [
                {"type": "function", "name": "read", "description": "read", "parameters": {}}
            ]
        });
        assert!(!needs_transform(&req, None));

        // Same request carrying a goal tool: transform required.
        let req2 = req_with_tools();
        assert!(needs_transform(&req2, None));
    }

    #[test]
    fn test_pure_and_stable_fragment() {
        // P16: two GoalStates with the same (goal, id) but different
        // iteration/used_tokens produce identical fragments.
        let mut g1 = GoalState::new("build a parser");
        g1.id = "g-deadbeef".to_string();
        g1.iteration = 0;
        g1.used_tokens = 0;

        let mut g2 = g1.clone();
        g2.iteration = 42;
        g2.used_tokens = 999_999;

        assert_eq!(g1.build_goal_fragment(), g2.build_goal_fragment());
    }
}
