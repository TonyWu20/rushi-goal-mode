//! `harness-hook-goal-compact` — goal compaction-veto hook.
//!
//! Registered on the `compact.before` window. Historically this hook
//! vetoed compaction when the goal's remaining token budget was low.
//!
//! Since goal mode has **no budget cap** (docs/goal-ux.md §1.6), the
//! loop runs until the goal is completed, blocked, paused, or
//! cleared by the user. Compaction is safe and encouraged during a
//! long goal run, so this hook now always allows compaction
//! (prints `{}`).
//!
//! The hook is kept in the config so the `compact.before` window
//! still has a goal-aware slot; it is a no-op pass-through.
//!
//! §12 pipeline ABI (rushi-goal-mode issue #5): `{}` is the no-op
//! state (accumulated state unchanged). Window dispatch is
//! `$HARNESS_WINDOW` (kernel-injected, authoritative) with the
//! payload `window` field as a manual-invocation fallback.
//!
//! §12.5 effect table: `compact.before` effect fields are `cancel`
//! and `replace`; this hook emits neither, so the window default
//! (proceed) applies.

use std::io::Read;

fn main() {
    // Self-documentation.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let payload = read_stdin_json();

    // Not our window: no-op. Dispatch: $HARNESS_WINDOW is authoritative
    // (kernel-injected, §12); the payload field covers manual invocation.
    if !window_is(&payload, "compact.before") {
        println!("{{}}");
        return;
    }

    // No budget cap in goal mode (§1.6): always allow compaction.
    println!("{{}}");
}

/// §12 window dispatch: `$HARNESS_WINDOW` is authoritative; fall back
/// to the payload's `window` field for manual invocations.
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

fn print_help() {
    println!("harness-hook-goal-compact — goal compaction hook (compact.before)");
    println!();
    println!("Window: compact.before (dispatch: $HARNESS_WINDOW, else payload window)");
    println!("Input (stdin): window JSON with keys window, session, reason, force");
    println!("Output (stdout):");
    println!("  {{}}  — no budget cap (§1.6); compaction always proceeds");
    println!("Exit codes: 0 = ok");
}
