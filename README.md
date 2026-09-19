# goal-app

The goal-mode application bundle for the `rushi` kernel.

This repo contains the four pieces that form one capability:

| Directory | Role | Consumed by |
|---|---|---|
| `goal-state/` | Shared state library (serde, no I/O) | every other crate in this repo |
| `goal-hooks/` | Four loop hooks: `harness-hook-goal-{arm,compact,idle,tools}` | kernel `[hooks]` section, resolved on PATH |
| `goal-tools/` | Three loop tools: `goal`, `goal_complete`, `goal_blocked` | kernel `[paths] extra_tools_roots` |
| `goal-ext/` | TUI extension: command palette + row slot | TUI `[ext] dir` (via the `ui_extensions/goal` symlink) |

All four depend on `rushi-goal-state` (the `goal-state/` directory) via a
local path dep. No crate in this repo
depends on anything outside this repo.

## Build

Each sub-directory is a standalone cargo package with an empty `[workspace]`
table. Build them all in one shot:

```sh
cargo build -p rushi-goal-state \
  -p hook-goal-arm -p hook-goal-compact -p hook-goal-idle -p hook-goal-tools \
  -p goal -p goal_complete -p goal_blocked \
  -p goal-ext
```

Or, if you use the kernel's `ext-env.sh`, it already includes every package
in this repo on its build list.

## Wiring into the kernel

The kernel config (typically `config-exts.example.toml` or a local
`config.toml`) must point at this repo:

```toml
[paths]
extra_tools_roots = ["<path-to-this-repo>/goal-tools"]

[hooks]
# the four hook binaries must be on PATH (via ext-env.sh or a direct
# path); see the [hooks] section of config-exts.example.toml.
```

The TUI discovers `goal-ext` through the `ui_extensions/goal` symlink that
the `rushi-exts` parking folder maintains. The host's `[ext] dir` points
at `ui_extensions/`; the symlink transparently includes `goal-ext` in the
layer.

## Repo boundaries

- This repo is self-contained: no path deps outside the kernel.
- `rushi-goal-state` (the `goal-state/` directory) is the single source of truth for the goal JSON schema
  and lives in this repo (it left the kernel at the split). The hooks,
  tools, and ext all read/write the same file through it.
- The kernel repo (`rust-unix-harness`) no longer carries `rushi-goal-state`.
  It only knows the hook binary names and tool manifests.
