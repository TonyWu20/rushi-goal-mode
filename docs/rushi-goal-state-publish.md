# rushi-goal-state publish decision

Status: staged, not yet published. The crates.io token step is
outstanding.

## Decisions (session 2026-09-19)

- License: MIT. Repo root `LICENSE` byte-copies the kernel
  (`rust-unix-harness`) LICENSE. The manifest carries
  `license = "MIT"`.
- Published name: `rushi-goal-state` (not the bare `goal-state`).
  The package name in `goal-state/Cargo.toml` was renamed to
  `rushi-goal-state`. The directory keeps the short `goal-state/`
  name. That matches the kernel precedent of the `crates/rushi`
  dir holding the `rushi-common` package.
- In-repo consumers (the 5 hooks, 3 tools, and `goal-ext`) keep
  local path deps on `goal-state/`. Cross-repo consumers
  (Rushi-WebUI's planned web goal-ext) will use the published
  crates.io version. It will replace the hand-mirrored
  `Rushi-WebUI/bin/rushi-web/src/goal.rs`.

The publish follows the `rushi-common` pattern (MIT, description,
repository, homepage in the manifest). Rushi-WebUI records the
upstream decision in its `docs/webui-dev-guide.md` and
`docs/ui-extension-web.md` (D9).

## Remaining steps

1. Commit this change set.
2. `cargo publish` in `goal-state/` (a crates.io auth token is
   needed, and none is on this machine yet).
3. Point Rushi-WebUI's web goal-ext at
   `rushi-goal-state = "0.1"` on crates.io.
