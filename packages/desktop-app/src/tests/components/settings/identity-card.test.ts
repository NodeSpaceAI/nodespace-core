/**
 * IdentityCard (ADR-037) — the Settings → Database read-only summary of
 * the seeded local-user PersonNode, with a link that opens the node so
 * PersonSchemaForm (the registered person editor) can edit it.
 */
/* global HTMLButtonElement */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import type { Node } from '$lib/types';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

const mockNavigateToNode = vi.fn();
const mockNavigateToNodeInOtherPane = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({
    navigateToNode: mockNavigateToNode,
    navigateToNodeInOtherPane: mockNavigateToNodeInOtherPane
  })
}));

import IdentityCard from '$lib/components/settings/sections/identity-card.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

const BLANK = { nodeId: 'person-1', firstName: '', lastName: '', email: '', isBlank: true };
const FILLED = {
  nodeId: 'person-1',
  firstName: 'Alice',
  lastName: 'Example',
  email: 'alice@example.com',
  isBlank: false
};

/** A person node in wire shape: core fields top-level, `properties` extension-only. */
function personNode(overrides: Partial<Node> & Record<string, unknown> = {}): Node {
  return {
    id: 'person-1',
    nodeType: 'person',
    content: '',
    title: 'Alice Example',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    firstName: 'Alice',
    lastName: 'Example',
    email: 'alice@example.com',
    ...overrides
  } as Node;
}

function editButton(container: HTMLElement): HTMLButtonElement | undefined {
  return Array.from(container.querySelectorAll('button')).find(
    (b) => b.textContent?.trim() === 'Edit identity'
  );
}

let ensureNodeSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  mockInvoke.mockReset();
  mockNavigateToNode.mockReset();
  mockNavigateToNodeInOtherPane.mockReset();
  ensureNodeSpy = vi.fn().mockResolvedValue(undefined);
  // Not hydrated by default — most tests exercise the get_local_identity
  // snapshot fallback; the "live store" tests below override this.
  vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(undefined);
  vi.spyOn(sharedNodeStore, 'ensureNode').mockImplementation(
    ensureNodeSpy as unknown as typeof sharedNodeStore.ensureNode
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('IdentityCard', () => {
  it('shows "Not set" and no name/email when the seeded person is blank', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Not set');
    expect(container.textContent).not.toContain('Alice');
    // No editable inputs remain — this card only displays and links out.
    expect(container.querySelector('input')).toBeNull();
  });

  it('loads and displays the current name/email when already set', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Set');
    expect(container.textContent).not.toContain('Not set');
    expect(container.textContent).toContain('Alice Example');
    expect(container.textContent).toContain('alice@example.com');
  });

  it('never calls set_local_identity — there is no Save affordance anymore', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(
      Array.from(container.querySelectorAll('button')).find(
        (b) => b.textContent?.trim() === 'Save'
      )
    ).toBeUndefined();
    expect(mockInvoke).not.toHaveBeenCalledWith('set_local_identity', expect.anything());
  });

  it('disables the "Edit identity" link until the identity has loaded', async () => {
    let resolveInvoke: (value: typeof BLANK) => void = () => {};
    mockInvoke.mockImplementation(
      (cmd: string) =>
        new Promise((resolve) => {
          if (cmd === 'get_local_identity') resolveInvoke = resolve;
        })
    );

    const { container } = render(IdentityCard);
    await tick();

    expect(editButton(container)?.disabled).toBe(true);

    resolveInvoke(BLANK);
    await tick();
    await tick();

    expect(editButton(container)?.disabled).toBe(false);
  });

  it('opens the person node in the other pane on a plain click', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    const button = editButton(container)!;
    await fireEvent.click(button);

    expect(mockNavigateToNodeInOtherPane).toHaveBeenCalledWith('person-1', expect.anything());
    expect(mockNavigateToNode).not.toHaveBeenCalled();
  });

  it('opens the person node as a new tab in this pane on a Cmd/Ctrl+click', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    const button = editButton(container)!;
    await fireEvent.click(button, { metaKey: true });

    expect(mockNavigateToNode).toHaveBeenCalledWith('person-1', true, expect.anything());
    expect(mockNavigateToNodeInOtherPane).not.toHaveBeenCalled();
  });

  it('is clickable and navigates even while the identity is blank (the backfill entry point)', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(BLANK);
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    const button = editButton(container)!;
    expect(button.disabled).toBe(false);

    await fireEvent.click(button);

    expect(mockNavigateToNodeInOtherPane).toHaveBeenCalledWith('person-1', expect.anything());
  });

  it('hydrates the person node into sharedNodeStore so later edits stay in sync', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });

    render(IdentityCard);
    await tick();
    await tick();

    expect(ensureNodeSpy).toHaveBeenCalledWith('person-1');
  });

  it('prefers the shared node store\'s live data over the initial get_local_identity snapshot', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });
    // Simulate the store already holding fresher data than the Tauri
    // snapshot — e.g. because PersonSchemaForm, open in the other pane,
    // already committed an edit through the store's typed person update.
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      personNode({ firstName: 'Alicia', lastName: 'Updated', email: 'alicia@example.com' })
    );

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Alicia Updated');
    expect(container.textContent).toContain('alicia@example.com');
    expect(container.textContent).not.toContain('Alice Example');
  });

  it('falls back to the get_local_identity snapshot before the store has hydrated the node', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });
    // Default beforeEach mock: sharedNodeStore.getNode returns undefined
    // (not yet hydrated) — the card must still show something from the
    // initial load rather than going blank.

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Alice Example');
    expect(container.textContent).toContain('alice@example.com');
  });

  it('shows a distinct failed state (not a permanent "Loading…") when get_local_identity rejects, with a working retry', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.reject(new Error('daemon unreachable'));
      return Promise.resolve();
    });

    const { container } = render(IdentityCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Failed to load');
    expect(container.textContent).not.toContain('Loading…');
    // No stale/disabled "Edit identity" button stuck forever — a Retry
    // action replaces it instead.
    expect(editButton(container)).toBeUndefined();
    const retryButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Retry'
    )!;
    expect(retryButton).toBeDefined();

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_local_identity') return Promise.resolve(FILLED);
      return Promise.resolve();
    });
    await fireEvent.click(retryButton);
    await tick();
    await tick();

    expect(container.textContent).toContain('Alice Example');
    expect(container.textContent).not.toContain('Failed to load');
  });
});
