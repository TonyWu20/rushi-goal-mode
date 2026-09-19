# rushi-goal-state publish decision

Status: published to crates.io as `rushi-goal-state 0.1.0`
(2026-09-19). Follow-up consumer work is below.

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

## Done

- 2026-09-19: committed the rename/license/metadata change set.
- 2026-09-19: published `rushi-goal-state 0.1.0` to crates.io.

## Remaining steps

1. Rushi-WebUI: point its web goal-ext at
   `rushi-goal-state = "0.1"` on crates.io and delete its
   hand-mirrored `bin/rushi-web/src/goal.rs`.
