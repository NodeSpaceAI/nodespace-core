#!/bin/bash
# Removes the pinned snapshots of one night when the swarm is finished (keeps all result files).
# usage: teardown.sh --part 1|2|3     or: SWARM_DIR=/some/dir teardown.sh
set -uo pipefail
PART=0
while [ $# -gt 0 ]; do
  case "$1" in
    --part) PART="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1 (usage: teardown.sh --part 1|2|3)" >&2; exit 2 ;;
  esac
done
if [ -z "${SWARM_DIR:-}" ]; then
  if [ "$PART" = 0 ]; then SWARM_DIR="$HOME/nodespace-swarm/night-all"; else SWARM_DIR="$HOME/nodespace-swarm/night-$PART"; fi
fi
S="$SWARM_DIR"
[ -d "$S" ] || { echo "no state dir at $S" >&2; exit 1; }
for r in core sync; do
  snap="$S/src-$r"
  [ -d "$snap" ] || continue
  # The snapshot's .git file points at the repo that owns it.
  repo="$(git -C "$snap" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git/\{0,1\}$##')"
  if [ -n "$repo" ]; then
    git -C "$repo" worktree remove --force "$snap" 2>&1 | tail -1
    git -C "$repo" worktree prune
  else
    echo "could not find the owning repo of $snap; remove it by hand with git worktree remove" >&2
  fi
done
