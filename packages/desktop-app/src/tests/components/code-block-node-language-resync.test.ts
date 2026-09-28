/**
 * Mounts the real code-block-node.svelte and drives the language-resync bug directly.
 *
 * `language` used to be a one-time `$state<string>(parseLanguage(content))` — it never
 * re-derived when `content` (a `$bindable` prop) changed for a reason other than the
 * local language dropdown, most importantly a remote/database-sourced update applied
 * while this window wasn't the active editor (see remote-update-policy.ts). Worse,
 * `handleContentChange` re-injects that stale `language` into the fence on every local
 * edit, silently reverting a collaborator's real language change and persisting the
 * regression.
 *
 * These tests mount the component and assert the observable contract: a `content` prop
 * update coming from outside (simulating a remote update) must resync the displayed
 * language, and a subsequent local edit must not revert it back to the stale value.
 *
 * The real BaseNode is too heavy to mount under Happy-DOM (Tauri-backed services,
 * autocomplete, slash commands, calendar) — it's stubbed with a fixture that still
 * dispatches `contentChanged`, so `handleContentChange` itself runs for real.
 */

import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('$lib/design/components/base-node.svelte', async () => {
  return await import('../fixtures/fake-base-node.svelte');
});

// The main render $effect already skips while `isEditing` is true (which these tests
// keep true throughout), so these are never actually invoked — mocked defensively so
// a test never waits on real shiki/mermaid loading if that guard ever changes.
vi.mock('$lib/services/syntax-highlight', () => ({
  highlightCode: vi.fn().mockResolvedValue([])
}));
vi.mock('$lib/services/mermaid-render', () => ({
  renderMermaid: vi.fn().mockResolvedValue(null)
}));

import CodeBlockNode from '$lib/design/components/code-block-node.svelte';
import { focusManager } from '$lib/services/focus-manager.svelte';

const NODE_ID = 'code-block-under-test';

/** Let pending $effect/$derived reactivity and microtasks settle. */
async function settle() {
  for (let i = 0; i < 5; i++) {
    await tick();
    await Promise.resolve();
  }
}

describe('CodeBlockNode language resync', () => {
  afterEach(() => {
    cleanup();
    focusManager.clearEditing();
  });

  it('re-derives the displayed language when content is updated externally', async () => {
    // isEditing must be true for the language button to render at all.
    focusManager.focusNode(NODE_ID, 'default');

    const { getByText, rerender } = render(CodeBlockNode, {
      props: { nodeId: NODE_ID, content: '```python\nprint(1)\n```' }
    });
    await settle();
    expect(getByText('python')).toBeTruthy();

    // A remote, database-sourced update landing on the bindable `content` prop —
    // exactly how sharedNodeStore/remote-update-policy pushes a change into an
    // unfocused node, not through handleContentChange.
    await rerender({ nodeId: NODE_ID, content: '```rust\nfn main() {}\n```' });
    await settle();

    expect(getByText('rust')).toBeTruthy();
  });

  it('does not revert a collaborator\'s language change on a subsequent local edit', async () => {
    focusManager.focusNode(NODE_ID, 'default');

    const { getByText, getByTestId, rerender } = render(CodeBlockNode, {
      props: { nodeId: NODE_ID, content: '```python\nprint(1)\n```' }
    });
    await settle();

    // Collaborator changes the language while this window is desynced.
    await rerender({ nodeId: NODE_ID, content: '```rust\nfn main() {}\n```' });
    await settle();
    expect(getByText('rust')).toBeTruthy();

    // Local user now edits the block body (fires the same `contentChanged` event
    // BaseNode dispatches on every keystroke-driven edit).
    await fireEvent.click(getByTestId('simulate-local-edit'));
    await settle();

    // The edit must not have reverted the language back to the stale 'python'.
    expect(getByText('rust')).toBeTruthy();
  });
});
