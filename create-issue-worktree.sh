#!/usr/bin/env bash
#
# create-issue-worktree.sh ISSUE
#
# Create a git worktree for issue N and symlink every Rust target/
# directory from the main (base) worktree into it, so that builds in
# the issue worktree reuse already-compiled artifacts.
#
# Usage:
#   ./create-issue-worktree.sh 42
#
# What it does:
#   1. git worktree add -b issue/42 goal-app-issue/42
#   2. For each target/ dir found by `fd -I target -t d` in the base,
#      create a *relative* symlink in the new worktree that points
#      back to the base's target/ dir.
#
# Safety guarantees:
#   - The script always resolves to the main worktree root (via
#     `git rev-parse --git-common-dir`), even if invoked from inside
#     a linked worktree.  All symlinks therefore point into the main
#     worktree, never into a sibling issue worktree.
#   - Symlinks are relative, so the whole tree stays portable.
#   - Pre-existing target/ dirs or stale symlinks at the destination
#     are removed before creating the new symlink.
#   - Re-running the script for an already-existing issue number
#     fails fast with a clear error (no destructive overwrite).

set -euo pipefail

# --------------------------------------------------------------- params
if [[ $# -ne 1 ]]; then
    echo "Usage: $(basename "$0") ISSUE_NUMBER" >&2
    exit 1
fi
ISSUE="$1"
if ! [[ "$ISSUE" =~ ^[0-9]+$ ]]; then
    echo "Error: issue number must be a positive integer (got '$ISSUE')" >&2
    exit 1
fi

# ------------------------------------------------- resolve main worktree
# git rev-parse --git-common-dir returns the path to the shared .git
# directory.  In the main worktree that is ".git" (relative to CWD);
# in a linked worktree it is an absolute path to the main .git.
# In either case, dirname of the *absolute* path is the main root.
GIT_COMMON="$(realpath "$(git rev-parse --git-common-dir)")"
BASE="$(dirname "$GIT_COMMON")"
cd "$BASE"

WT_REL="goal-app-issue/${ISSUE}"
WT="${BASE}/${WT_REL}"
BRANCH="issue/${ISSUE}"

# ---------------------------------------------------------------- guards
if [[ -e "$WT" ]]; then
    echo "Error: ${WT_REL} already exists — remove it or pick another issue number." >&2
    exit 1
fi

if git rev-parse --verify "$BRANCH" >/dev/null 2>&1; then
    echo "Error: branch '${BRANCH}' already exists." >&2
    echo "  Delete it with:  git branch -D ${BRANCH}" >&2
    exit 1
fi

command -v fd >/dev/null 2>&1 || {
    echo "Error: 'fd' not found in PATH (required to locate target/ dirs)." >&2
    exit 1
}

# ------------------------------------------------------- create worktree
echo "==> git worktree add -b ${BRANCH} ${WT_REL}"
git worktree add -b "$BRANCH" "$WT_REL"

# ------------------------------------------------- find target dirs in base
echo "==> Finding target/ directories in ${BASE}"
mapfile -t TARGETS < <(cd "$BASE" && fd -I target -t d)

if [[ ${#TARGETS[@]} -eq 0 ]]; then
    echo "    No target/ directories found — nothing to symlink."
    exit 0
fi

echo "    Found ${#TARGETS[@]} target dir(s):"
for t in "${TARGETS[@]}"; do
    echo "      ${t}"
done

# ------------------------------------------------- create relative symlinks
linked=0
skipped=0
for rel in "${TARGETS[@]}"; do
    rel="${rel%/}"                  # strip trailing '/' that fd adds
    base_target="${BASE}/${rel}"
    wt_target="${WT}/${rel}"
    wt_parent="$(dirname "$wt_target")"

    # Ensure the parent directory tree exists inside the new worktree
    mkdir -p "$wt_parent"

    # If a real dir or stale symlink is already there, remove it
    if [[ -e "$wt_target" || -L "$wt_target" ]]; then
        echo "    ! removing existing ${rel}"
        rm -rf "$wt_target"
    fi

    # Skip if the source doesn't actually exist (shouldn't happen)
    if [[ ! -d "$base_target" ]]; then
        echo "    ! skipping ${rel} (missing in base)"
        skipped=$((skipped + 1))
        continue
    fi

    # Compute the relative path from wt_parent to base_target
    link_target="$(realpath --relative-to="$wt_parent" "$base_target")"
    ln -s "$link_target" "$wt_target"
    echo "    + ${rel}  ->  ${link_target}"
    linked=$((linked + 1))
done

# ------------------------------------------------------------- summary
echo
echo "Worktree  : ${WT}"
echo "Branch    : ${BRANCH}"
echo "Symlinked : ${linked} target dir(s)"
if [[ "$skipped" -gt 0 ]]; then
    echo "Skipped   : ${skipped} (missing in base)"
fi
echo
echo "All symlinks point into the main worktree (${BASE})."
echo "Builds in the issue worktree will share the base's compiled artifacts."
