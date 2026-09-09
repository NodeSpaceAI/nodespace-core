# Shared Workspaces (Multi-User)

Loaded on demand — read this when the daemon you're talking to is bound to a NodeSpace collection that is synced and shared with a teammate through NodeSpace Pro. Skip it entirely for a private, single-user database.

A NodeSpace collection can be **synced and shared** with a teammate through NodeSpace Pro: the daemon is launched already bound to it, so another engineer — or their agent — reads and writes the same graph. It is opt-in, private to its members, and syncs only once each engineer has signed in and enabled sync. In a shared workspace:

**Everything you save is visible, and you are not the only writer.** You don't pick the shared collection per write — nodes you create sync into it automatically. Keep private scratch in a separate database (`nodespace database create`/`--database`). A node here may have been created or last edited by your teammate, so don't assume it is yours or stable across your session, and search at session start to pull what they already saved. Don't move sensitive or unrelated notes in without intent.

**Attribute what you save, and prefer additive writes.** Put provenance in the content — who wrote it and when — since a human-readable marker is easier to scan than the per-node creator NodeSpace records. Add a new node rather than rewriting one your teammate authored; when you must update a shared node, pass the `version` you read via `nodespace node batch-update`, so a concurrent edit surfaces as an OCC conflict instead of silently overwriting. (`node update` without a version bypasses that check.)

**Recall is eventually consistent, and semantic search lags further.** A teammate's write appears after sync latency. For immediate cross-engineer recall use structured queries — `nodespace query`, or `nodespace node query --content-contains "..."` — which see a peer's node as soon as it syncs. Semantic `nodespace search` works only after your machine has embedded it: embeddings are generated locally, not synced, so fall back to `nodespace query` for a recent teammate note.

**Don't file shared memory under date nodes.** Attaching findings under `--parent "YYYY-MM-DD"` does **not** round-trip through sync yet — date-container nodes stay local. Save findings as regular nodes (optionally under a shared project or collection node) or your teammate won't see them.
