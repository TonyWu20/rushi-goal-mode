# Failure tracking

Local failure log for the `goal-app` extensions.
The scope is `goal-state` and the goal hooks:
`hook-goal-idle`, `hook-goal-arm`, `hook-goal-compact`,
`hook-goal-tools`.
Kernel-side failures live in the kernel's own
`docs/failure-tracking.md` (rust-unix-harness).
Writing-gate failures live in
`rushi-simple-english/docs/failure-tracking.md`.

IDs are scoped to this repo.
The first entry is a migration of an older record.
That record lives in `rushi-simple-english/docs/failure-tracking.md`.
There, the same incident is numbered FT-002.
The number is kept so the two records stay traceable to the
same incident.
New goal-app-local failures continue from FT-003.

## FT-002 — Goal `run.idle` refire: model re-posts its own final reply

**Symptom**
Session `rushi/sessions/readme-writing` (kernel checkout
`~/programming/rust-unix-harness`).
The model posted a final recommendation at 23:45:12 (log line 667).
That reply ended with a question to the user.
No new user message arrived in that window.

The model re-verified and re-posted the same recommendation at
23:45:27 and 23:45:41 (log lines 673 and 689).
Two silent refires fired between the first post and the cap.
To the human reader it looked like the user was echoing the model's
own reply.
The real user message arrived at 23:48:15 (log line 691).

**Root cause**
A `run.idle` goal-continuation hook returned `continue` with `refire`.
The kernel re-ran the model in place, with no new user message.
Each refired call re-presented the model's own last reply.
It also re-presented the original user turn (user_seq 540, the
v0.1.1 question).
The model read that re-presentation as "keep working."
It re-posted its own recommendation instead of waiting for the user.

No hook-origin marker covered this path.
The writing-gate marker (simple-english FT-001) targets the
`pending_feedback` path only.
The goal continuation had no equivalent.
The model did not know the re-presented text was its own output.

Note: the current `hook-goal-idle` returns `continue` with a logged
`message` (the continuation prompt), not `refire`.
The `refire` path is the silent one.
No user message is logged on it.
The model only re-encounters its own reply.
It also sees the standing goal fragment from the `model.before`
transform.
Both paths need the origin marker.

**Driver (from the log)**
The refire was driven by the goal continuation hook, not the writing
gate.
The `model_call_context` markers anchor each refired call to
user_seq 540.
The `run.refire` and `run.refire_cap` markers confirm the
silent-refire path.
The cap is `[run] max_silent_refires` (default 2, kernel
`docs/reference/README.md` section 6.4).

**Evidence** (`~/programming/rust-unix-harness/sessions/readme-writing/events.jsonl`)

- line 667, 23:45:12: `assistant_message`, the recommendation
  ending in a question to the user.
- line 668, 23:45:12: `ext_status` id `run.refire`, value `{"n":2}`.
- lines 669-672: `loop_phase "wait"`, `hook.model.before "transform"`,
  `hook_applied` (goal-arm), `model_call_context {"user_seq":540}`.
- line 673, 23:45:27: `assistant_message`, "Let me verify two things
  before recommending…", with no user input.
- line 689, 23:45:41: `assistant_message`, the recommendation
  re-posted a second time.
- line 690, 23:45:41: `ext_status` id `run.refire_cap`,
  value `{"cap":2,"refires":2}`.
- line 691, 23:48:15: `user_message`, the real input.
  It names this confusion. It says the FT-001 mitigation was not enough.

**Fix**
A hook-origin marker, mirroring the simple-english FT-001 pattern.
The patch is `docs/ft002-goal-hook.patch` in this repo.
It applies to `goal-state/src/lib.rs` and
`goal-hooks/hook-goal-idle/src/main.rs`.

1. `build_continue_prompt()` opens with the marker
   `[goal-continuation hook, not a user message]`.
   It states that no new user message arrived.
   It states that the user confirmed or approved nothing.
   It says not to treat the prompt as a reply to the model's last
   question.
   This covers the logged-continuation path (`continue` + `message`).
2. `build_goal_fragment()` carries a static "Continuation origin"
   note.
   It says the re-presented text is the model's own reply plus the
   original user turn.
   It is not a new user message.
   The note is static, so the fragment stays byte-stable (P16/P17).
   This covers the silent-refire path.
   There, `goal-arm` re-injects the fragment on `model.before`.

**Verification**
Verified on a clean copy of the goal-app tree (HEAD plus patch):
- `cargo test` in `goal-state`: 32 passed.
  New tests: `continue_prompt_names_the_continuation` (marker
  assertions), `continue_prompt_marker_does_not_echo_the_reply_body`,
  `goal_fragment_carries_continuation_origin_note`.
- `cargo test` in `hook-goal-idle`: 4 passed.
  The continuation test now asserts the marker.
- `cargo test` in `hook-goal-arm`: 9 passed.
  Byte-stability and fragment tests stay intact.
- `run-idle-continue-e2e.sh` (exts repo): 28 passed, 0 failed.
  Its `active-goal` checks match the "continuation #N" substring.
  That substring survives the marker prefix.

The e2e config needed an update before the suite could run.
The legacy keys were `[paths] tools_root` and `extra_tools_roots`.
The kernel renamed them in commit 353424a.
The new keys are `native_tool_paths` and `extension_tool_paths`.

**Related (out of scope here)**
An optional kernel/TUI enhancement would label each refired reply as
"gated revision n".
Then a human reader sees the origin.
That is a kernel-side change (see simple-english FT-001 "Related").

## FT-003 — run.idle refire after goal completion (issue #1)

**Symptom**
Session `address-issue-16`
(`~/programming/rust-unix-harness/sessions/address-issue-16/events.jsonl`).

After the `goal_complete` tool closed the goal, the run.idle goal hook
kept returning `continue` with `refire: true`.
The kernel ran up to `max_silent_refires` (default 2) more model turns
with no new user message.

Each refire re-sent only the accumulated context plus the goal fragment
from the model.before transform.
Three model turns ran after the goal was complete.
One of them produced a false user-confirmation.

It read "The user has confirmed..." with no such user message.
The cap `run.refire_cap` stopped the loop.
The false acknowledgment still leaked into the transcript.

**Root cause**
The run.idle goal-continuation hook did not check the goal state.
It returned `refire: true` even when the goal was terminal.

A refire is only useful while a goal still has work pending.
Once the goal closes, a refire has nothing to deliver.
It only wastes a model call and risks a false confirmation.

**Fix (consumer-side, this repo)**
The goal extension must not request refires it does not need.
In `goal-hooks/hook-goal-idle/src/main.rs` the decision is now an
explicit pure function `idle_decision(goal)`.

- Terminal goal (completed, blocked, or paused) emits `{}`.
  The kernel stops the loop and runs no further model turns.
  No `refire` is ever requested.

- Open goal emits `continue` with a logged `message`.
  The kernel appends it as a `user_message`.
  No `refire` payload.

**Verification**
- `cargo test` in `hook-goal-idle`: 8 passed (4 new issue-1 tests).
- `cargo test` across all nine goal-app crates: all pass.
- Smoke test: run.idle with a completed goal emits `{}`.
  A blocked goal also emits `{}`.
  An open goal emits the logged continuation with no refire key.

The kernel refire mechanism is unchanged and works as designed.
See kernel issue #6.
The cap bounded the damage.
The goal extension now requests only the decisions it needs.
