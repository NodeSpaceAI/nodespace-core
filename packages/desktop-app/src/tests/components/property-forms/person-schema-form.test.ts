/**
 * PersonSchemaForm — adopt-existing suggestion (ADR-065).
 *
 * `person.email` carries a store-aware `unique` schema rule. This form is the
 * creation/edit surface where a collision must surface as a dismissible
 * suggestion — never a blocking error, and never a skipped save. These tests
 * drive a real blur through the component and assert:
 *   - a colliding email shows "use existing / keep as new"
 *   - the field save is never gated on the lookup (suggest-don't-block)
 *   - "Use existing" navigates to the match and never deletes/merges anything
 *   - "Keep as new" just dismisses the suggestion
 *   - no collision (or a "collision" with itself) shows nothing
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const navigateToNodeInOtherPane = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ navigateToNodeInOtherPane })
}));

// PersonSchemaForm's Relationships trigger is gated on this service, matching
// TypedFormShell's gate for Task/GenericSchemaForm. Stub it so the gate
// never reaches a daemon and the other 19 tests below — none of which care about
// Relationships — don't incidentally exercise its fail-open error path.
const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import PersonSchemaForm from '$lib/components/property-forms/person-schema-form.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

function personNode(overrides: Partial<Node> = {}): Node {
  return {
    id: 'person-1',
    nodeType: 'person',
    content: '',
    title: 'Alice',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { person: { first_name: 'Alice', last_name: '', email: '' } },
    ...overrides
  } as Node;
}

function existingMatch(overrides: Partial<Node> = {}): Node {
  return {
    id: 'person-existing',
    nodeType: 'person',
    content: '',
    title: 'Bob Existing',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { person: { first_name: 'Bob', last_name: 'Existing', email: 'bob@example.com' } },
    ...overrides
  } as Node;
}

let updateNodeSpy: ReturnType<typeof vi.fn>;
let findDuplicateForSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateNodeSpy = vi.fn().mockResolvedValue(personNode());
  vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(personNode());
  vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof backendAdapter.updateNode
  );
  findDuplicateForSpy = vi.fn().mockResolvedValue(null);
  vi.spyOn(backendAdapter, 'findDuplicateFor').mockImplementation(
    findDuplicateForSpy as unknown as typeof backendAdapter.findDuplicateFor
  );
  // Re-armed every test, not just restored — `vi.restoreAllMocks()` below clears a bare
  // `vi.fn()`'s implementation entirely, so a later test with no override would otherwise
  // see `undefined` and throw calling `.then()` on it inside the gate's own effect.
  loadNodeRelationshipsView.mockReset();
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'person', groups: [] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function blurEmail(value: string) {
  const input = screen.getByLabelText('Email');
  await fireEvent.blur(input, { target: { value } });
}

describe('PersonSchemaForm — field placeholders', () => {
  // `email`'s placeholder ("email@example.com") was always a good example. `first_name`
  // and `last_name` used to hardcode their own label text back as the placeholder
  // ("First name" / "Last name") — pins that this no longer restates the label, and
  // instead shows a real example, matching email's pattern.
  it('shows example values, not the field label repeated back', () => {
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const firstName = screen.getByLabelText('First name') as HTMLInputElement;
    const lastName = screen.getByLabelText('Last name') as HTMLInputElement;
    const email = screen.getByLabelText('Email') as HTMLInputElement;

    expect(firstName.placeholder).not.toBe('First name');
    expect(firstName.placeholder.length).toBeGreaterThan(0);
    expect(lastName.placeholder).not.toBe('Last name');
    expect(lastName.placeholder.length).toBeGreaterThan(0);
    expect(email.placeholder).toBe('email@example.com');
  });
});

describe('PersonSchemaForm — adopt-existing suggestion', () => {
  it('surfaces the suggestion when the blurred email collides with another person', async () => {
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');

    await waitFor(() =>
      expect(screen.getByText(/already exists: Bob Existing/i)).toBeTruthy()
    );
    expect(screen.getByRole('button', { name: 'Use existing' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Keep as new' })).toBeTruthy();
  });

  it('never blocks or skips the save, even when a collision is found', async () => {
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());

    // The write happens regardless of the suggestion — suggest, never block.
    expect(updateNodeSpy).toHaveBeenCalledTimes(1);
    const [, , update] = updateNodeSpy.mock.calls[0];
    expect((update.properties.person as Record<string, unknown>).email).toBe('bob@example.com');
  });

  it('"Use existing" navigates to the match and dismisses the suggestion', async () => {
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());

    await fireEvent.click(screen.getByRole('button', { name: 'Use existing' }));

    expect(navigateToNodeInOtherPane).toHaveBeenCalledWith('person-existing');
    expect(screen.queryByText(/already exists/i)).toBeNull();

    // Non-destructive: adopting never touches the current node.
    expect(updateNodeSpy.mock.calls.every(([id]) => id === 'person-1')).toBe(true);
  });

  it('"Keep as new" dismisses the suggestion without navigating', async () => {
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());

    await fireEvent.click(screen.getByRole('button', { name: 'Keep as new' }));

    expect(navigateToNodeInOtherPane).not.toHaveBeenCalled();
    expect(screen.queryByText(/already exists/i)).toBeNull();
  });

  it('shows no suggestion when the email has no conflict', async () => {
    findDuplicateForSpy.mockResolvedValue(null);
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('nobody@example.com');
    await waitFor(() => expect(findDuplicateForSpy).toHaveBeenCalled());

    expect(screen.queryByText(/already exists/i)).toBeNull();
  });

  it('passes its own nodeId as excludeId, so the lookup cannot match itself', async () => {
    // The primary self-exclusion mechanism is server-side (excludeId threads
    // through to a SQL exclusion), proven at the backend layer; this asserts
    // the frontend actually participates by sending its own id.
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');
    await waitFor(() => expect(findDuplicateForSpy).toHaveBeenCalled());

    expect(findDuplicateForSpy).toHaveBeenCalledWith('person', 'email', 'bob@example.com', 'person-1');
  });

  it('never suggests the node adopt itself, even if the backend misbehaves', async () => {
    // A defensive backstop, not the primary exclusion mechanism (that's
    // excludeId, above): a pathological/self-referential lookup result must
    // still never render, even if a hypothetical backend bug returned it.
    findDuplicateForSpy.mockResolvedValue(personNode({ id: 'person-1' }));
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('alice@example.com');
    await waitFor(() => expect(findDuplicateForSpy).toHaveBeenCalled());

    expect(screen.queryByText(/already exists/i)).toBeNull();
  });

  it('check and save fire concurrently — the check does not wait for the save', async () => {
    // Regression guard: an earlier version awaited the save before starting
    // the duplicate check, so by the time the check ran, this node's own
    // freshly-saved row already held the value too — a real false-negative
    // risk given the lookup has no ORDER BY. Assert the check is issued
    // before the save's promise has settled, not after.
    let resolveSave!: () => void;
    updateNodeSpy.mockReturnValue(
      new Promise((resolve) => {
        resolveSave = () => resolve(personNode());
      })
    );
    findDuplicateForSpy.mockResolvedValue(null);
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const input = screen.getByLabelText('Email');
    const blurPromise = fireEvent.blur(input, { target: { value: 'bob@example.com' } });

    // The duplicate check must already have been issued while the save is
    // still in flight, not queued behind it.
    await waitFor(() => expect(findDuplicateForSpy).toHaveBeenCalled());
    expect(updateNodeSpy).toHaveBeenCalledTimes(1);

    resolveSave();
    await blurPromise;
  });

  it('resets a stale suggestion when nodeId changes to a different person', async () => {
    // The component instance can be reused across different person nodes (no
    // {#key nodeId} at any call site). A suggestion computed for the FIRST
    // person must not linger — and must not let "Use existing" navigate using
    // a match id that no longer has anything to do with the person now shown.
    findDuplicateForSpy.mockResolvedValue(existingMatch());
    const { rerender } = render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());

    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      personNode({ id: 'person-2', content: 'Carol' })
    );
    await rerender({ nodeId: 'person-2' });

    expect(screen.queryByText(/already exists/i)).toBeNull();
  });

  it('a stale rejected lookup does not clobber a newer, still-valid suggestion', async () => {
    // Sequence: blur an email whose lookup will eventually REJECT, then blur a
    // different email whose lookup resolves first with a real match. The
    // earlier (now-superseded) rejection must not wipe the valid suggestion
    // the later blur produced — the staleness guard must cover the catch
    // branch, not just the success branch.
    let rejectFirst!: (err: Error) => void;
    findDuplicateForSpy
      .mockReturnValueOnce(
        new Promise((_resolve, reject) => {
          rejectFirst = reject;
        })
      )
      .mockResolvedValueOnce(existingMatch());

    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });
    const input = screen.getByLabelText('Email');

    await fireEvent.blur(input, { target: { value: 'first@example.com' } });
    await fireEvent.blur(input, { target: { value: 'second@example.com' } });
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());

    // The stale first lookup finally rejects AFTER the valid suggestion is
    // already showing — it must not clear it.
    rejectFirst(new Error('stale lookup failed'));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.getByText(/already exists/i)).toBeTruthy();
  });

  it('does not look up a duplicate for an empty email', async () => {
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('');

    expect(findDuplicateForSpy).not.toHaveBeenCalled();
    expect(screen.queryByText(/already exists/i)).toBeNull();
  });
});

/**
 * Relationships trigger gating. PersonSchemaForm used to show the
 * Relationships button unconditionally, unlike GenericSchemaForm
 * (which already gated it) — and, once TaskSchemaForm started composing
 * through TypedFormShell, unlike Task too. This closes that remaining
 * inconsistency directly in PersonSchemaForm (which stays hardcoded, not
 * TypedFormShell-composed, by deliberate design), using the exact same gate
 * logic, copied verbatim.
 */
describe('PersonSchemaForm — Relationships trigger gate', () => {
  it('hides the Relationships entry point when the type has no typed relationships', async () => {
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'person', groups: [] });
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() => expect(loadNodeRelationshipsView).toHaveBeenCalledWith('person-1'));
    await loadNodeRelationshipsView.mock.results[0].value;
    await Promise.resolve();
    expect(screen.queryByText('Relationships')).toBeNull();
  });

  it('shows the Relationships entry point when the type has a typed relationship', async () => {
    loadNodeRelationshipsView.mockResolvedValue({
      nodeType: 'person',
      groups: [{ key: 'assigned_to' }]
    });
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });

  it('fails open (shows the trigger) when the relationship check errors', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });
});
