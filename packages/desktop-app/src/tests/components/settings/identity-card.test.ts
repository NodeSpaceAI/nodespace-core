/**
 * IdentityCard (ADR-037) — the Settings → Database read-only summary of
 * the seeded local-user PersonNode, with a link that opens the node so
 * PersonSchemaForm (the registered person editor) can edit it.
 */
/* global HTMLButtonElement */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { render, fireEvent } from '@testing-library/svelte';

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

const BLANK = { nodeId: 'person-1', firstName: '', lastName: '', email: '', isBlank: true };
const FILLED = {
  nodeId: 'person-1',
  firstName: 'Alice',
  lastName: 'Example',
  email: 'alice@example.com',
  isBlank: false
};

function editButton(container: HTMLElement): HTMLButtonElement | undefined {
  return Array.from(container.querySelectorAll('button')).find(
    (b) => b.textContent?.trim() === 'Edit identity'
  );
}

describe('IdentityCard', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockNavigateToNode.mockReset();
    mockNavigateToNodeInOtherPane.mockReset();
  });

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
});
