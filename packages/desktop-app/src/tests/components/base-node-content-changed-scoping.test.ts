/**
 * Regression test: base-node.svelte's controllerEvents.contentChanged must not
 * read/clear a DIFFERENT node's pending focusManager.cursorPosition signal.
 *
 * Three of contentChanged's four call sites (TextareaController.handleInput,
 * insertSlashCommand, toggleFormatting) are driven synchronously by a real DOM event
 * or a synchronous caller, so this node's own textarea must already be
 * document.activeElement (and therefore focusManager.editingNodeId === nodeId) at the
 * moment they fire - the read is provably safe there.
 *
 * The fourth call site, TextareaController.insertNodeReference(), is NOT
 * synchronously event-driven: base-node.svelte's handleAutocompleteSelect awaits a
 * real backend round trip (createNewNodeFromMention) before calling
 * controller.insertNodeReference(...). During that await, a DIFFERENT node's
 * node-type-conversion signal can become pending (e.g. the user types "# " in an
 * unrelated node while this node's mention-creation request is in flight). Without the
 * `focusManager.editingNodeId === nodeId` guard, this node's stale post-await callback
 * would read and clear that unrelated node's signal before it's consumed.
 *
 * This test reaches the REAL, wired-up controllerEvents.contentChanged handler (not a
 * re-implemented copy) by rendering BaseNode and calling insertNodeReference() directly
 * on the TextareaController instance attached to the mounted textarea element (the
 * `_textareaController` hook TextareaController's own constructor sets) - the same
 * production callback base-node.svelte wires into the controller via
 * createTextareaController(), reached without needing to drive the full
 * mention-autocomplete UI and mock the backend round trip.
 */

import { describe, it, expect, afterEach } from 'vitest';
import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import BaseNode from '../../lib/design/components/base-node.svelte';
import { focusManager } from '../../lib/services/focus-manager.svelte';
import { DEFAULT_PANE_ID } from '../../lib/stores/navigation.svelte';
import type { TextareaController } from '../../lib/design/components/textarea-controller';

interface ControllerHost {
  _textareaController?: TextareaController;
}

async function renderEditingNode(nodeId: string): Promise<HTMLTextAreaElement> {
  focusManager.focusNode(nodeId, DEFAULT_PANE_ID);
  render(BaseNode, { props: { nodeId, content: 'hello' } });
  await tick();
  const textarea = document.getElementById(
    `textarea__${DEFAULT_PANE_ID}__${nodeId}`
  ) as HTMLTextAreaElement | null;
  if (!textarea) {
    throw new Error(`Expected an editing textarea for node ${nodeId}`);
  }
  return textarea;
}

function getController(textarea: HTMLTextAreaElement): TextareaController {
  const controller = (textarea as unknown as ControllerHost)._textareaController;
  if (!controller) {
    throw new Error('TextareaController was not attached to the textarea element');
  }
  return controller;
}

describe('base-node.svelte contentChanged is scoped to its own node', () => {
  afterEach(() => {
    focusManager.clearEditing();
    document.body.innerHTML = '';
  });

  it('does not clear a different node\'s pending node-type-conversion signal via insertNodeReference\'s post-await callback', async () => {
    // Node A: the node whose mention-creation "await" is about to resolve.
    const textareaA = await renderEditingNode('node-a');
    const controllerA = getController(textareaA);

    // Give node A a mention session, as real typing of "@query" would (insertNodeReference
    // no-ops without one).
    textareaA.value = 'Hello @world';
    textareaA.selectionStart = 12;
    textareaA.selectionEnd = 12;
    textareaA.dispatchEvent(new Event('input', { bubbles: true }));

    // Simulate the async gap: while node A's createNewNodeFromMention() backend call was
    // in flight, the user triggered a node-type-conversion on a DIFFERENT node ("node-b").
    // This is exactly what base-node-viewer.svelte's conversion handlers do.
    focusManager.focusNodeFromTypeConversion('node-b', 3, DEFAULT_PANE_ID);
    expect(focusManager.cursorPosition?.type).toBe('node-type-conversion');

    // Node A's await resolves; base-node.svelte's handleAutocompleteSelect calls this
    // directly on node A's own (stale) controller - exactly like the real post-await path.
    controllerA.insertNodeReference('new-node-id', 'New Node');

    // Node B's signal must survive: it was never node A's to consume.
    expect(focusManager.editingNodeId).toBe('node-b');
    expect(focusManager.cursorPosition?.type).toBe('node-type-conversion');
  });

  it('still clears its own node-type-conversion signal via insertNodeReference when it actually owns it', async () => {
    const textareaA = await renderEditingNode('node-a');
    const controllerA = getController(textareaA);

    textareaA.value = 'Hello @world';
    textareaA.selectionStart = 12;
    textareaA.selectionEnd = 12;
    textareaA.dispatchEvent(new Event('input', { bubbles: true }));

    // This time the pending signal actually belongs to node A itself.
    focusManager.focusNodeFromTypeConversion('node-a', 3, DEFAULT_PANE_ID);
    expect(focusManager.cursorPosition?.type).toBe('node-type-conversion');

    controllerA.insertNodeReference('new-node-id', 'New Node');

    expect(focusManager.cursorPosition).toBeNull();
  });
});
