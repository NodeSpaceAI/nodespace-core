#!/bin/bash
# Creates the pinned read-only code snapshots and the known-issues list for one
# night of the review swarm. Safe to re-run: it only creates what is missing, so
# a resume keeps the SAME snapshots.
#
# usage: preflight.sh --part 1|2|3        (state dir: $HOME/nodespace-swarm/night-<part>)
#    or: SWARM_DIR=/some/dir preflight.sh
# env:   SWARM_DIR  state dir, one per night
#        CORE_REPO  core checkout (default: the git toplevel containing this kit)
#        SYNC_REPO  sync checkout (default: $HOME/nodespace/sync, then a sibling of core named like the sync repo)
#        DOCS_REPO  docs checkout, optional (default: $HOME/nodespace/docs, then ../nodespace-docs)
#        BASE_REF   ref to pin in both repos (default: origin/main)
set -euo pipefail

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PART=0
while [ $# -gt 0 ]; do
  case "$1" in
    --part) PART="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1 (usage: preflight.sh --part 1|2|3)" >&2; exit 2 ;;
  esac
done
case "$PART" in 0|1|2|3) ;; *) echo "--part must be 1, 2 or 3 (or 0 for everything)" >&2; exit 2 ;; esac

for tool in git gh jq; do
  command -v "$tool" >/dev/null || { echo "$tool is required but not on PATH" >&2; exit 1; }
done

if [ -z "${SWARM_DIR:-}" ]; then
  if [ "$PART" = 0 ]; then SWARM_DIR="$HOME/nodespace-swarm/night-all"; else SWARM_DIR="$HOME/nodespace-swarm/night-$PART"; fi
fi
S="$SWARM_DIR"
BASE_REF="${BASE_REF:-origin/main}"

CORE_REPO="${CORE_REPO:-$(git -C "$KIT" rev-parse --show-toplevel 2>/dev/null || true)}"
if [ -z "$CORE_REPO" ] || { [ ! -d "$CORE_REPO/.git" ] && [ ! -f "$CORE_REPO/.git" ]; }; then
  echo "Cannot find the core checkout. Set CORE_REPO=/path/to/nodespace-core." >&2
  exit 1
fi

# The sibling directory names are built from a prefix because core must not spell out the sync repo name.
SIBLING_PREFIX=nodespace
if [ -z "${SYNC_REPO:-}" ]; then
  for c in "$HOME/nodespace/sync" "$CORE_REPO/../${SIBLING_PREFIX}-sync"; do
    if [ -d "$c/.git" ] || [ -f "$c/.git" ]; then SYNC_REPO="$(cd "$c" && pwd)"; break; fi
  done
fi
if [ -z "${SYNC_REPO:-}" ] || { [ ! -d "$SYNC_REPO/.git" ] && [ ! -f "$SYNC_REPO/.git" ]; }; then
  echo "Cannot find the sync checkout (looked in \$HOME/nodespace/sync and a sibling of core named like the sync repo). Clone the sync repo and set SYNC_REPO=/path/to/it." >&2
  exit 1
fi

if [ -z "${DOCS_REPO:-}" ]; then
  for c in "$HOME/nodespace/docs" "$CORE_REPO/../nodespace-docs"; do
    if [ -d "$c/.git" ] || [ -f "$c/.git" ]; then DOCS_REPO="$(cd "$c" && pwd)"; break; fi
  done
fi
DOCS_REPO="${DOCS_REPO:-}"
[ -n "$DOCS_REPO" ] || echo "note: no docs checkout found; reviewers will skip the ADR checks (set DOCS_REPO to fix)" >&2

mkdir -p "$S"/{findings,verdicts,review,filed,scratch,logs}
S="$(cd "$S" && pwd)"

# Remember which checkouts this night uses, so make-args.ts and teardown.sh agree.
jq -n --arg core "$CORE_REPO" --arg sync "$SYNC_REPO" --arg docs "$DOCS_REPO" \
  '{coreRepo:$core, syncRepo:$sync} + (if $docs == "" then {} else {docsRepo:$docs} end)' > "$S/config.json"

for r in core sync; do
  if [ "$r" = core ]; then repo="$CORE_REPO"; else repo="$SYNC_REPO"; fi
  if [ ! -d "$S/src-$r" ]; then
    git -C "$repo" fetch -q origin main
    git -C "$repo" worktree add -q --detach "$S/src-$r" "$BASE_REF"
  fi
done

if [ ! -f "$S/shas.json" ]; then
  printf '{"core":"%s","sync":"%s"}\n' "$(git -C "$S/src-core" rev-parse HEAD)" "$(git -C "$S/src-sync" rev-parse HEAD)" > "$S/shas.json"
fi

if [ ! -f "$S/existing-issues.json" ]; then
  tmp="$S/existing-issues.json.tmp"
  # The issues endpoint also returns pull requests, so those are filtered out; paginating has no cap.
  { for r in core sync cloud; do gh api --paginate "repos/NodeSpaceAI/nodespace-$r/issues?state=all&per_page=100" --jq ".[]|select(has(\"pull_request\")|not)|{repo:\"$r\",number,title,state:(.state|ascii_upcase)}"; done; } | jq -s . > "$tmp"
  mv "$tmp" "$S/existing-issues.json"
fi

echo "state dir: $S"
cat "$S/shas.json"
echo "known issues: $(jq length "$S/existing-issues.json")"
