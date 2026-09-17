/**
 * Guards SharedNodeStore's opt-in eviction-reachability invariant by construction.
 *
 * SharedNodeStore evicts a cached node once it is unreachable through BOTH of its
 * reachability channels: the `structureTree` walk (automatic — a node is safe as
 * long as it's a descendant of an open tab's document root) and the `pinNodes`/
 * `unpinAll` API (opt-in — a component displaying a node OUTSIDE that structural
 * relationship, e.g. a query-view row, a `[[wikilink]]` reference, or a node-card,
 * must pin it explicitly for as long as it's displayed).
 *
 * The pin channel is unenforced: nothing stops a new component from reading
 * `sharedNodeStore.getNode`/`ensureNode` on a node outside the structureTree walk
 * without also pinning it, silently reintroducing the "node vanishes/shows as
 * deleted while still on screen" bug three separate historical instances of this
 * exact mistake were fixed for (a query-view row, a wikilink reference, and a
 * node-card reference — each caught only by manual review, not by any automated
 * signal). This test is that automated signal.
 *
 * ## Mechanism
 *
 * Every non-test `.ts`/`.svelte` file under `src/` that calls `getNode`/
 * `ensureNode` on the store (either via the `sharedNodeStore` singleton import or
 * the `SharedNodeStore.getInstance()` form) must satisfy one of:
 *
 *   1. It also calls `pinReachableNodes(...)` or `...pinNodes(...)` in the SAME
 *      file — the normal shape for a component instance that reads a per-instance
 *      dynamic id it doesn't structurally own (TableRow, NodeRefInline,
 *      QueryNodeViewer, NodeCardInline, node-ref-preview.svelte.ts all follow
 *      this). This is the shape a new consumer of the bug class is expected to
 *      hit, and the one this guard is primarily written to catch.
 *   2. It is `reactive-node-service.svelte.ts` — the one call site the original
 *      eviction design explicitly carves out: it only ever reads structureTree-
 *      adjacent nodes within its own open document.
 *   3. It is listed in `REACHABLE_WITHOUT_PIN` below, with a reason. Every entry
 *      here was manually audited (see the commit that introduced `pinNodes`) as
 *      reading a node that either IS the open tab's own root/property-panel
 *      target, or receives an id that a parent component already pinned as part
 *      of the same result set (ListView/KanbanView read ids QueryNodeViewer
 *      already pinned wholesale). Adding a new entry here is a real, reviewable
 *      claim that a new call site is safe for one of those same reasons — it is
 *      not a silent opt-out.
 *
 * A file calling `getNode`/`ensureNode` that matches none of the three is a
 * violation: either a genuinely new instance of the bug, or a legitimately safe
 * new consumer that hasn't justified itself in `REACHABLE_WITHOUT_PIN` yet.
 *
 * ## A known blind spot (by design, not oversight)
 *
 * `DATABASE_SETTINGS_NODE_ID` — a fixed, app-wide singleton, not a per-instance
 * dynamic id — is read in `ui-extensions.svelte.ts` but pinned in
 * `database.svelte.ts`'s `refreshDatabaseSettings()`, deliberately in a
 * DIFFERENT file (the read side is a pure derivation with no lifecycle to hang a
 * pin/unpin off; the pin is owned centrally, alongside the singleton's refresh,
 * for its whole app lifetime). No same-file (or even same-directory) heuristic
 * can validate a cross-file pin/read pairing for an arbitrary future singleton,
 * so `ui-extensions.svelte.ts` is listed in `REACHABLE_WITHOUT_PIN` and the pin
 * side is guarded directly by the second test below instead.
 */

import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const srcRoot = path.join(packageRoot, 'src');

/** The one call site the eviction design itself exempts — see the module doc. */
const REACTIVE_NODE_SERVICE = 'lib/services/reactive-node-service.svelte.ts';

/** The store's own implementation — defines getNode/ensureNode, doesn't call them. */
const STORE_IMPLEMENTATION = 'lib/services/shared-node-store.svelte.ts';

/**
 * Files that read `getNode`/`ensureNode` without a same-file pin, each with the
 * structural reason an audit found it safe. See the module doc for what each
 * reason means and why it doesn't need a same-file pin call.
 */
const REACHABLE_WITHOUT_PIN: Record<string, string> = {
  'lib/design/components/base-node-viewer.svelte':
    'Reads the viewer\'s own nodeId and its structureTree children/descendants — the document tree it renders.',
  'lib/design/components/person-node.svelte':
    "Reads its own nodeId prop — the *Node wrapper's own node, a structureTree member of whatever document rendered it.",
  'lib/design/components/checkbox-node.svelte':
    "Reads its own nodeId prop, same as person-node.svelte.",
  'lib/design/components/schema-field-update.ts':
    'Writes a field on the node the calling property-form is already displaying (itself structureTree-reachable or the open tab root).',
  'lib/components/schema/generic-schema-form.svelte':
    "Property panel for the currently-open node's own nodeId.",
  'lib/components/property-forms/task-schema-form.svelte':
    "Property panel for the currently-open node's own nodeId.",
  'lib/components/property-forms/person-schema-form.svelte':
    "Property panel for the currently-open node's own nodeId.",
  'lib/components/layout/pane-content.svelte':
    'Resolves the pane\'s own tab content nodeId — about to become (or already is) an open tab root.',
  'lib/components/layout/tab-system.svelte':
    "Reads each open tab's own content nodeId to compute its title.",
  'lib/components/viewers/ai-chat-node-viewer.svelte':
    "Reads the viewer's own nodeId — the open tab root.",
  'lib/components/viewers/ai-chat-pty-session.svelte':
    "Reads the viewer's own nodeId, same as ai-chat-node-viewer.svelte.",
  'lib/services/navigation-service.ts':
    'Resolves a navigation target synchronously to open a tab with it — a one-time read, not a live display binding; once opened the node is a tab root.',
  'lib/components/query/list-view.svelte':
    "Reads ids from its `nodeIds` prop — the exact set QueryNodeViewer already pinned wholesale before passing it down.",
  'lib/components/query/kanban-view.svelte':
    'Reads ids from its `nodeIds` prop, same as list-view.svelte.',
  'lib/plugins/ui-extensions.svelte.ts':
    'Reads the fixed DATABASE_SETTINGS_NODE_ID singleton, pinned centrally (by design, in a different file) by database.svelte.ts\'s refreshDatabaseSettings() — see the second test below.',
};

/** Every .ts and .svelte file under src/, recursively, excluding tests. */
function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (full.split(path.sep).includes('tests')) return [];
      return sourceFiles(full);
    }
    return /\.(ts|svelte)$/.test(entry.name) ? [full] : [];
  });
}

/**
 * Drops full-line comments (`// ...`) and jsdoc/block-comment continuation lines
 * (`* ...`) before matching, so a doc example mentioning `sharedNodeStore.getNode(`
 * in prose isn't mistaken for a real call site. Not a full comment parser — good
 * enough for this codebase's comment shapes, matching node-accent-colors.test.ts's
 * precedent of a targeted regex over a full AST parse.
 */
function stripCommentLines(source: string): string {
  return source
    .split('\n')
    .filter((line) => {
      const trimmed = line.trim();
      return !trimmed.startsWith('*') && !trimmed.startsWith('//');
    })
    .join('\n');
}

/** `sharedNodeStore.getNode(`/`.ensureNode(`, or the equivalent `SharedNodeStore.getInstance()` form. */
const READ_CALL = /(sharedNodeStore|SharedNodeStore\s*\.\s*getInstance\s*\(\s*\))\s*\.\s*(getNode|ensureNode)\s*\(/;

/** `pinReachableNodes(...)`, or a direct `sharedNodeStore.pinNodes(...)`/`SharedNodeStore.getInstance().pinNodes(...)` call. */
const PIN_CALL =
  /(pinReachableNodes\s*\(|(sharedNodeStore|SharedNodeStore\s*\.\s*getInstance\s*\(\s*\))\s*\.\s*pinNodes\s*\()/;

describe('SharedNodeStore pin-reachability guard', () => {
  it('never reads a node from SharedNodeStore without an in-file pin or a documented reachability exemption', () => {
    const offenders = sourceFiles(srcRoot)
      .map((file) => path.relative(srcRoot, file))
      .filter((rel) => rel !== REACTIVE_NODE_SERVICE && rel !== STORE_IMPLEMENTATION)
      .filter((rel) => {
        const code = stripCommentLines(fs.readFileSync(path.join(srcRoot, rel), 'utf8'));
        return READ_CALL.test(code) && !PIN_CALL.test(code);
      })
      .filter((rel) => !(rel in REACHABLE_WITHOUT_PIN));

    expect(
      offenders,
      offenders.length > 0
        ? `${offenders.join(', ')} read a SharedNodeStore node without pinning it and without a documented ` +
            `REACHABLE_WITHOUT_PIN exemption in this test file. A node this displays outside its own document's ` +
            `structureTree can be silently evicted while still on screen. Either pin the id(s) it reads with ` +
            `pinReachableNodes(ownerId, ids) (see pin-node-reachability.ts), or — only if it genuinely reads its ` +
            `own tab's node or an id a parent already pinned — add it to REACHABLE_WITHOUT_PIN with the reason.`
        : undefined
    ).toEqual([]);
  });

  it('keeps the DATABASE_SETTINGS_NODE_ID singleton pinned by its owning refreshDatabaseSettings()', () => {
    // DATABASE_SETTINGS_NODE_ID has no structureTree relationship to any open tab
    // and is read from a different file (ui-extensions.svelte.ts, exempted above)
    // than the one that pins it, so the generic same-file check above cannot see
    // this pairing. Assert it directly instead: database.svelte.ts must still pin
    // the singleton, unconditionally, every time it refreshes it.
    const source = fs.readFileSync(path.join(srcRoot, 'lib/stores/database.svelte.ts'), 'utf8');

    expect(source).toContain('DATABASE_SETTINGS_NODE_ID');
    expect(source).toMatch(/sharedNodeStore\s*\.\s*pinNodes\s*\(\s*DATABASE_SETTINGS_PIN_OWNER\s*,\s*\[\s*DATABASE_SETTINGS_NODE_ID\s*\]\s*\)/);
  });
});
