# Issue #5 — §12 pipeline ABI migration for goal-app hooks

Status: implemented (2026-09-28). Branch `issue/5`.

The three goal hooks that emitted the retired §4.3 decision envelopes
were migrated to the §12 pipeline ABI (kernel issue #38, committed as
kernel PR #39). Under §12 the kernel seeds each window pipeline with
a state object; a step's stdout *replaces* (or no-ops) the
accumulated state. Envelopes are gone: effects travel as concrete
state fields or log-side-effects.

## Changes by hook

### `hook-goal-idle` (run.idle)

- Removed the `{"decision":"continue","payload":{"message":...}}`
  emission.
- Now: on an open goal the hook appends a follow-queue
  `user_message` to the session log via `LOG_BIN --session $SESSION`
  (event JSON on stdin, `ts` in RFC3339 Z). The kernel re-claims and
  keeps the loop alive iff the re-claim has `pending_follow_ups`.
- Terminal / absent goal: append nothing, print `{}`, exit 0.
- Append failure (LOG_BIN missing / spawn / write / non-zero exit):
  print `{"reason":"..."}` and exit 3. The kernel logs
  `hook.run.idle.error` with the reason; the chain stops and the
  window default (`stop`) applies.
- Window dispatch: `$HARNESS_WINDOW` (kernel-injected) is
  authoritative; the payload `window` field is a manual-invocation
  fallback.

### `hook-goal-arm` (model.before)

- Removed the `{"decision":"transform","payload":{"request":{...}}}`
  envelope and the `payload.get("request")` unwrap.
- Now: the stdin state *is* the request object (the kernel seeds
  model.before with `request.json`). The hook emits the full
  transformed request object (fragment set, tools filtered) or `{}`
  (no-op, state unchanged) when nothing goal-related needs doing.
- Window dispatch: `$HARNESS_WINDOW` (the request object carries no
  `window` key); payload `window` field is a manual-invocation
  fallback.

### `hook-goal-tools` (tool.before)

- Removed the `{"decision":"block","payload":{"reason","calls"}}`
  envelope (which triggered the `hook.tool.before.unknown_fields`
  marker in real sessions).
- Now: emits `{"blocked_calls":[{"id","reason"},...]}` — the §12.5
  state field. The kernel synthesizes a failed `tool_result` per
  entry and routes the rest. Empty → `{}`.
- Window dispatch: same pattern as above.

### `hook-goal-compact` / `hook-goal-tokens` (dispatch only)

- These hooks already emitted `{}` (§12-compatible). Their
  `payload.get("window")` dispatch was updated to the same
  `$HARNESS_WINDOW`-first pattern for consistency and to guard
  against a future kernel that drops the `window` field from the
  payload.

## `ts` for the appended `user_message`

The appended `user_message` carries an RFC3339 Z `ts` (the kernel e2e
convention writes `date -u +%Y-%m-%dT%H:%M:%SZ`; the TUI parses event
`ts` via `chrono::DateTime::parse_from_rfc3339`).

`hook-goal-idle` generates it with
`chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)` — the
same one-liner the kernel already uses. `chrono` is added as a dep of
`hook-goal-idle` only (an unpublished hook crate), **not** of
`rushi-goal-state`, which stays dependency-free (serde + serde_json
only) because it is published to crates.io and consumed by the web
goal-ext (wasm). See "Decisions" below.

## Decisions

- **chrono lives in the hook, not in goal-state.** The issue text
  prescribed a pure-std `rfc3339_now()` in `goal-state`, but
  `rushi-goal-state` is a published, dep-free schema crate also
  consumed by the (wasm) web goal-ext — dragging chrono into it would
  bloat every consumer. Instead `hook-goal-idle` (unpublished) takes
  the `chrono` dep and uses the kernel's own one-liner for the
  timestamp. `goal-state` stays free of the civil-date math.
- Compact/tokens dispatch migration is beyond the issue's numbered
  scope (three hooks) but included because the issue intro says
  "every goal hook is inert under the current kernel" and the
  fix is one function + a one-line call-site change. Trivial to
  revert if undesired.
- The idle hook's `iteration` counter is incremented *before*
  building the continuation prompt (so the prompt says
  "continuation #N+1"), matching the original ordering.
