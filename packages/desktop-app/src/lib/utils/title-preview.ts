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
