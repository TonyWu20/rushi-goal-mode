//! `rushi-goal` — the headless goal seeder (rushi-goal-mode issue #8).
//!
//! The user's setup script calls this once, right before it execs
//! `rushi run`, to arm the goal files for the session. It resolves
//! the config by the kernel's ladder (`rushi_common::paths::
//! resolve_config_path` — the design says not to reinvent the
//! ladder), reads `sessions_root` from that config, resolves the
//! session dir exactly the way the kernel does, creates the session
//! dir if needed, and writes `goal.json` plus `goal-<id>.json` as a
//! fresh active goal. That is the same pair of files the `goal` loop
//! tool writes, so the seeder and the loop agree by construction.
//!
//! It prints nothing on success and exits 0. The task text is never
//! echoed: the script passes it to its own `exec rushi run` line.
//! Errors go to stderr with a non-zero exit.
//!
//! Usage:
//!   rushi-goal <SESSION> <GOAL_PROMPT> [--config /path]

use std::env;
use std::path::{Path, PathBuf};

use rushi_common::paths::resolve_config_path;
use rushi_goal_state::GoalState;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let ParsedArgs {
        session,
        goal,
        cli_config,
    } = match parse_args(&args) {
        Some(p) => p,
        None => std::process::exit(2),
    };

    if goal.is_empty() {
        fail("empty goal prompt: pass a non-empty task text as the second argument.");
    }

    // 1. Resolve the config by the kernel's ladder. Do not reinvent
    //    it: this is the same function the loop uses, so the seeder
    //    and the loop land on the same config.
    let config_path = PathBuf::from(resolve_config_path(cli_config.as_deref()));

    // 2. Read sessions_root from the resolved config.
    let sessions_root = match read_sessions_root(&config_path) {
        Ok(root) => root,
        Err(e) => fail(&e),
    };

    // 3. Resolve the session dir the way the kernel's resolve_session
    //    does, then create it. The loop creates it lazily on its first
    //    log append, but the seeder must write into it, so it creates
    //    it up front.
    let session_dir = resolve_session_dir(&sessions_root, &session);
    if let Err(e) = std::fs::create_dir_all(&session_dir) {
        fail(&format!(
            "cannot create the session dir {}: {e}",
            session_dir.display()
        ));
    }

    // 4. Arm the goal files: a fresh active goal, exactly as the
    //    `goal` tool writes them.
    let state = GoalState::new(&goal);
    if let Err(e) = state.save(&session_dir) {
        fail(&format!(
            "cannot write the goal files in {}: {e}",
            session_dir.display()
        ));
    }

    // Success: no output, exit 0.
}

/// The parsed CLI: a session name, a goal prompt, and an optional
/// explicit config path (`--config /path`).
struct ParsedArgs {
    session: String,
    goal: String,
    cli_config: Option<PathBuf>,
}

/// Parse `rushi-goal <SESSION> <GOAL_PROMPT> [--config /path]`.
///
/// Only `--config` (space or `=` form), `-h`, and `--help` are
/// treated as flags; every other argument is positional, so a goal
/// prompt that starts with a dash is not mistaken for a flag. On
/// success it returns the fields. On a missing positional it prints
/// the usage to stderr and returns `None` (`main` turns that into an
/// exit code 2).
fn parse_args(args: &[String]) -> Option<ParsedArgs> {
    let mut session: Option<String> = None;
    let mut goal: Option<String> = None;
    let mut cli_config: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if a == "--config" {
            i += 1;
            if i >= args.len() {
                return usage_err("missing value for --config");
            }
            cli_config = Some(PathBuf::from(args[i].clone()));
        } else if let Some(v) = a.strip_prefix("--config=") {
            cli_config = Some(PathBuf::from(v));
        } else if a == "-h" || a == "--help" {
            print_usage();
            std::process::exit(0);
        } else if session.is_none() {
            session = Some(a);
        } else if goal.is_none() {
            goal = Some(a);
        } else {
            return usage_err(&format!("unexpected extra argument: {a}"));
        }
        i += 1;
    }
    match (session, goal) {
        (Some(s), Some(g)) => Some(ParsedArgs {
            session: s,
            goal: g,
            cli_config,
        }),
        _ => usage_err("missing <SESSION> and/or <GOAL_PROMPT>"),
    }
}

/// Read `[paths] sessions_root` from the config TOML. A missing key
/// defaults to `sessions`, matching the kernel. A relative value is
/// joined with the current working directory, the same way the
/// kernel resolves it: the setup script leaves the process in the
/// worktree, which is the loop's CWD, so the seeder and the loop
/// agree on where the session dir lands.
fn read_sessions_root(config_path: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(config_path).map_err(|e| {
        format!(
            "cannot read the config {}: {e}",
            config_path.display()
        )
    })?;
    let v: toml::Value = toml::from_str(&text).map_err(|e| {
        format!(
            "invalid TOML in the config {}: {e}",
            config_path.display()
        )
    })?;
    let raw = v
        .get("paths")
        .and_then(|p| p.get("sessions_root"))
        .and_then(|s| s.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sessions"));
    if raw.is_relative() {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot read the CWD to resolve a relative sessions_root: {e}"))?;
        Ok(cwd.join(raw))
    } else {
        Ok(raw)
    }
}

/// Mirror of the kernel's `HarnessConfig::resolve_session`. A session
/// that names a path (contains a slash or already exists as a dir) is
/// used as-is; a plain name is joined under `sessions_root`.
fn resolve_session_dir(sessions_root: &Path, session: &str) -> PathBuf {
    let p = PathBuf::from(session);
    if session.contains('/') || session.contains('\\') || p.is_dir() {
        p
    } else {
        sessions_root.join(session)
    }
}

/// Print a one-line error to stderr and exit 1 (a runtime failure).
fn fail(msg: &str) -> ! {
    eprintln!("rushi-goal: {msg}");
    std::process::exit(1);
}

/// Print the usage to stderr and return `None` for `parse_args` to
/// hand back. `main` maps `None` to an exit code 2 (a caller error).
fn usage_err(why: &str) -> Option<ParsedArgs> {
    eprintln!("rushi-goal: {why}");
    eprintln!("usage: rushi-goal <SESSION> <GOAL_PROMPT> [--config /path]");
    None
}

/// Print the usage to stdout (an explicit `-h`/`--help`) and exit 0.
fn print_usage() {
    println!("rushi-goal — arm the goal files for a session before the loop starts.");
    println!();
    println!("usage: rushi-goal <SESSION> <GOAL_PROMPT> [--config /path]");
    println!();
    println!("Writes goal.json and goal-<id>.json into the session dir so goal mode is");
    println!("active when the loop starts. Prints nothing on success and exits 0.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let p = parse_args(&["hq-3-1758790200".into(), "fix the bug".into()]).unwrap();
        assert_eq!(p.session, "hq-3-1758790200");
        assert_eq!(p.goal, "fix the bug");
        assert!(p.cli_config.is_none());
    }

    #[test]
    fn parse_config_space_form() {
        let p = parse_args(
            &["s".into(), "g".into(), "--config".into(), "/tmp/c.toml".into()],
        )
        .unwrap();
        assert_eq!(p.cli_config, Some(PathBuf::from("/tmp/c.toml")));
    }

    #[test]
    fn parse_config_equals_form() {
        let p = parse_args(&["s".into(), "g".into(), "--config=/tmp/c.toml".into()]).unwrap();
        assert_eq!(p.cli_config, Some(PathBuf::from("/tmp/c.toml")));
    }

    #[test]
    fn config_flag_between_positionals() {
        let p = parse_args(&["s".into(), "--config".into(), "/c".into(), "g".into()]).unwrap();
        assert_eq!(p.session, "s");
        assert_eq!(p.goal, "g");
        assert_eq!(p.cli_config, Some(PathBuf::from("/c")));
    }

    #[test]
    fn goal_prompt_may_start_with_dash() {
        // A task text that starts with `-` is still positional, not a
        // flag, as long as it is not a known flag.
        let p = parse_args(&["s".into(), "-not-a-flag task".into()]).unwrap();
        assert_eq!(p.goal, "-not-a-flag task");
    }

    #[test]
    fn missing_goal_fails() {
        assert!(parse_args(&["only-session".into()]).is_none());
    }

    #[test]
    fn extra_positional_fails() {
        assert!(parse_args(&["s".into(), "g".into(), "extra".into()]).is_none());
    }

    #[test]
    fn resolve_session_dir_joins_plain_name() {
        let root = Path::new("/work/sessions");
        assert_eq!(
            resolve_session_dir(root, "hq-3"),
            PathBuf::from("/work/sessions/hq-3")
        );
    }

    #[test]
    fn resolve_session_dir_uses_explicit_path() {
        // A session that names a path is used as-is, not joined.
        let root = Path::new("/work/sessions");
        assert_eq!(
            resolve_session_dir(root, "../elsewhere/sess"),
            PathBuf::from("../elsewhere/sess")
        );
    }

    #[test]
    fn sessions_root_default_and_custom() {
        // A temp config with no [paths] section defaults to "sessions"
        // relative to the CWD.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[model]\napi = \"responses\"\n").unwrap();
        let root = read_sessions_root(&cfg).unwrap();
        assert!(root.ends_with("sessions") || root == PathBuf::from("sessions"));

        // An explicit key is used.
        let cfg2 = dir.path().join("c2.toml");
        std::fs::write(&cfg2, "[paths]\nsessions_root = \"custom-sessions\"\n").unwrap();
        let root2 = read_sessions_root(&cfg2).unwrap();
        assert!(root2.to_string_lossy().contains("custom-sessions"));
    }

    #[test]
    fn missing_config_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("nope.toml");
        assert!(read_sessions_root(&absent).is_err());
    }

    #[test]
    fn invalid_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "this is not [ valid toml").unwrap();
        assert!(read_sessions_root(&bad).is_err());
    }
}
