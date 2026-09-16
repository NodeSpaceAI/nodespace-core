/**
 * Shared "push a client-computed title into the store" kernel for the
 * ADR-077 client-side title preview.
 *
 * Both `person-schema-form.svelte` and `generic-schema-form.svelte` compute
 * a template-interpolated title locally (via `evaluateTitleTemplate`) and
 * need to reflect it in `sharedNodeStore` instantly, independent of any
 * backend round trip. Each form's own field-gathering logic differs
 * (person hardcodes two known fields; generic iterates an arbitrary
 * schema's fields) and stays in each component — but the mechanics of
 * getting a computed title into the store safely are identical, and are
 * kept here as the single place that can change (e.g. if the
 * `isComputedField` contract itself ever changes) without needing to find
 * and update every caller individually.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types';

/**
 * Pushes a client-computed title into `sharedNodeStore` as a pure,
 * unpersisted UI echo — via `isComputedField`, which skips persistence and
 * OCC entirely, so this can never fight the field write it accompanies.
 * Every reader of the store (header, tab, inline row) reflects the change
 * in the same tick. No-op when `title` already matches the store's current
 * value, so an unrelated keystroke that doesn't change the interpolated
 * result never issues a redundant store write or subscriber notification.
 *
 * KNOWN GAP: this write has no rollback. If the real field write that
 * triggered it (the enum/text change the template reads from) fails to
 * persist — offline, an OCC conflict — the speculative title stays showing
 * whatever this call last set, with nothing to revert it back once the
 * failed field write's own resync-from-server settles. The two writes are
 * independent `updateNode()` calls, so the field write's failure handling
 * has no reference back to this one. Left unaddressed pending a follow-up:
 * fixing it well needs the failure path to also recompute-and-repush (or
 * resync) the title, not just something scoped to this function.
 */
export function pushComputedTitle(
  nodeId: string,
  node: Node,
  title: string,
  viewerId: string
): void {
  if (title === (node.title ?? '')) return;
  sharedNodeStore.updateNode(
    nodeId,
    { title },
    { type: 'viewer', viewerId },
    { isComputedField: true }
  );
}
