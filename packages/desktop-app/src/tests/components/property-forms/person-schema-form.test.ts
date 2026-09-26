/**
 * PersonSchemaForm — adopt-existing suggestion (ADR-065).
 *
 * `person.email` carries a store-aware `unique` schema rule, enabled by the
 * loaded person schema's flag (not hardcoded per type). This form is the
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

import type { PersonNode } from '$lib/types';
import PersonSchemaForm from '$lib/components/property-forms/person-schema-form.svelte';
import { buildRelationshipsView } from '$lib/services/relationship-grouping';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

type PersonOverrides = Partial<Node> & Partial<Pick<PersonNode, 'firstName' | 'lastName' | 'email'>>;

/** A person node in wire shape: core fields top-level, `properties` extension-only. */
function personNode(overrides: PersonOverrides = {}): Node {
  return {
    id: 'person-1',
    nodeType: 'person',
    content: '',
    title: 'Alice',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    firstName: 'Alice',
    ...overrides
  } as Node;
}

function existingMatch(overrides: PersonOverrides = {}): Node {
  return {
    id: 'person-existing',
    nodeType: 'person',
    content: '',
    title: 'Bob Existing',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    firstName: 'Bob',
    lastName: 'Existing',
    email: 'bob@example.com',
    ...overrides
  } as Node;
}

/** The person schema as getSchema returns it: email flagged `unique`, case-insensitive. */
function personSchema(emailFlags: { unique?: boolean; uniqueCaseInsensitive?: boolean } = {
  unique: true,
  uniqueCaseInsensitive: true
}) {
  return {
    id: 'person',
    content: 'person',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: true,
    schemaVersion: 1,
    description: '',
    titleTemplate: '{first_name} {last_name}',
    fields: [
      { name: 'first_name', friendlyName: 'First name', type: 'string', protection: 'core' },
      { name: 'last_name', friendlyName: 'Last name', type: 'string', protection: 'core' },
      { name: 'email', friendlyName: 'Email', type: 'string', protection: 'core', ...emailFlags }
    ]
  };
}

let updateNodeSpy: ReturnType<typeof vi.fn>;
let updatePersonNodeSpy: ReturnType<typeof vi.fn>;
let findDuplicateForSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  // The save path is the store's typed person update (ADR-049) — not
  // backendAdapter directly. It's synchronous (void), not a Promise: the store
  // applies the change optimistically and hands persistence off in the
  // background. `updateNode` is still stubbed because the title preview
  // pushes through it.
  updateNodeSpy = vi.fn();
  updatePersonNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(personNode());
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );
  vi.spyOn(sharedNodeStore, 'updatePersonNode').mockImplementation(
    updatePersonNodeSpy as unknown as typeof sharedNodeStore.updatePersonNode
  );
  vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(personSchema() as never);
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
  await schemaLoaded();
  const input = screen.getByLabelText('Email');
  await fireEvent.blur(input, { target: { value } });
}

/** The unique rule is armed only once the schema that declares it has loaded. */
async function schemaLoaded() {
  await waitFor(() => expect(backendAdapter.getSchema).toHaveBeenCalledWith('person'));
  await Promise.resolve();
}

describe('PersonSchemaForm — shared shell', () => {
  it('renders through TypedFormShell: a collapsible panel with a field-count badge', async () => {
    vi.mocked(sharedNodeStore.getNode).mockReturnValue(personNode({ email: 'a@example.com' }));
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const trigger = screen.getByText('2/3 fields').closest('button') as HTMLElement;
    // Starts open (the header is read-only for a title_template type).
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByLabelText('First name')).toBeTruthy();

    await fireEvent.click(trigger);
    await waitFor(() => expect(trigger.getAttribute('aria-expanded')).toBe('false'));
  });
});

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

describe('PersonSchemaForm — typed person fields', () => {
  // Every transport delivers a person's core fields as top-level typed fields
  // (firstName/lastName/email); `properties` holds extension fields only.
  it('populates first name, last name and email from the typed fields', () => {
    vi.mocked(sharedNodeStore.getNode).mockReturnValue(
      personNode({
        firstName: 'Michael', lastName: 'Libio', email: 'm@example.com'
      })
    );
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('Michael');
    expect((screen.getByLabelText('Last name') as HTMLInputElement).value).toBe('Libio');
    expect((screen.getByLabelText('Email') as HTMLInputElement).value).toBe('m@example.com');
  });

  it('never reads a core field from properties', () => {
    vi.mocked(sharedNodeStore.getNode).mockReturnValue(
      personNode({ firstName: undefined, properties: { first_name: 'stale copy' } })
    );
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    expect((screen.getByLabelText('First name') as HTMLInputElement).value).toBe('');
  });

  it('writes only the edited field, through the typed person update', async () => {
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });
    await fireEvent.blur(screen.getByLabelText('Last name'), { target: { value: 'Smith' } });

    expect(updatePersonNodeSpy).toHaveBeenCalledTimes(1);
    const [, update] = updatePersonNodeSpy.mock.calls[0];
    expect(update).toEqual({ lastName: 'Smith' });
    expect(updateNodeSpy.mock.calls.some(([, changes]) => 'properties' in changes)).toBe(false);
  });

  it('clears an emptied field instead of storing an empty string', async () => {
    vi.mocked(sharedNodeStore.getNode).mockReturnValue(personNode({ lastName: 'Smith' }));
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });
    await fireEvent.blur(screen.getByLabelText('Last name'), { target: { value: '' } });

    const [, update] = updatePersonNodeSpy.mock.calls[0];
    expect(update).toEqual({ lastName: null });
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
    expect(updatePersonNodeSpy).toHaveBeenCalledTimes(1);
    const [, update] = updatePersonNodeSpy.mock.calls[0];
    expect(update.email).toBe('bob@example.com');
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
    expect(updatePersonNodeSpy.mock.calls.every(([id]) => id === 'person-1')).toBe(true);
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

  it('the save is never gated behind the duplicate lookup', async () => {
    // Regression guard: an earlier version of this form awaited the save
    // (a real network round trip via backendAdapter.updateNode) before
    // starting the duplicate check, so by the time the check ran, this
    // node's own freshly-saved row already held the value too — a real
    // false-negative risk given the lookup has no ORDER BY. The save now
    // goes through the store's typed person update, which applies optimistically
    // and returns synchronously (persistence happens in the background) —
    // so it must already have landed by the time a still-pending duplicate
    // lookup resolves, not queued behind it.
    let resolveLookup!: (match: Node | null) => void;
    findDuplicateForSpy.mockReturnValue(
      new Promise((resolve) => {
        resolveLookup = resolve;
      })
    );
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });
    await schemaLoaded();

    const input = screen.getByLabelText('Email');
    const blurPromise = fireEvent.blur(input, { target: { value: 'bob@example.com' } });

    // The save is synchronous — it must already have happened even though
    // the duplicate lookup is still pending.
    await waitFor(() => expect(findDuplicateForSpy).toHaveBeenCalled());
    expect(updatePersonNodeSpy).toHaveBeenCalledTimes(1);

    resolveLookup(null);
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
    await schemaLoaded();
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

  it('does not look up a duplicate when the schema does not flag email unique', async () => {
    // The rule is the schema's, not person's: without the flag there is no lookup.
    vi.mocked(backendAdapter.getSchema).mockResolvedValue(personSchema({}) as never);
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('bob@example.com');

    expect(updatePersonNodeSpy).toHaveBeenCalledTimes(1);
    expect(findDuplicateForSpy).not.toHaveBeenCalled();
  });

  it('does not look up a duplicate for an empty email', async () => {
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await blurEmail('');

    expect(findDuplicateForSpy).not.toHaveBeenCalled();
    expect(screen.queryByText(/already exists/i)).toBeNull();
  });
});

/**
 * Relationships trigger gating — owned by TypedFormShell, which
 * PersonSchemaForm composes like every other typed form.
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
    loadNodeRelationshipsView.mockResolvedValue(
      buildRelationshipsView({
        nodeId: 'person-1',
        nodeType: 'person',
        groups: [
          {
            relationshipName: 'tasks',
            direction: 'out',
            targetType: 'task',
            reverseName: 'assignee',
            sourceType: 'person',
            cardinality: 'many',
            farCardinality: 'many',
            required: null,
            edgeFields: null,
            description: null,
            related: [],
            count: 0
          }
        ]
      })
    );
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });

  it('fails open (shows the trigger) when the relationship check errors', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });

  it('renders a single-valued relationship as a field, not behind the trigger', async () => {
    // e.g. a user schema declaring `employees → person` with a `one` reverse.
    loadNodeRelationshipsView.mockResolvedValue(
      buildRelationshipsView({
        nodeId: 'person-1',
        nodeType: 'person',
        groups: [
          {
            relationshipName: 'employees',
            direction: 'in',
            targetType: 'company',
            reverseName: 'employer',
            sourceType: 'company',
            cardinality: 'one',
            farCardinality: 'many',
            required: null,
            edgeFields: null,
            description: null,
            related: [
              { id: 'co-1', nodeType: 'company', title: 'Acme', contentPreview: '', edgeProperties: {} }
            ],
            count: 1
          }
        ]
      })
    );
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Employer: Acme' })).toBeTruthy()
    );
    expect(screen.queryByText('Relationships')).toBeNull();
  });
});

/**
 * Field saves route through the store (ADR-049), title resolution regression.
 *
 * Before this fix, `updateField()` called `backendAdapter.updateNode(...)`
 * directly and discarded its response — the only property form in the
 * codebase that bypassed `sharedNodeStore.updateNode`, the store-mediated
 * path every viewer (including BaseNodeViewer, which reads titles reactively
 * from the store) actually reads from. Confirmed by runtime repro: editing
 * first/last name left the store's cached copy of the node — including its
 * title AND its own first_name/last_name properties — completely
 * untouched; nothing (not even a delayed update) reflected the edit without
 * an external `NodeUpdated` domain-event re-hydration, which doesn't fire at
 * all outside a live Tauri runtime (see tauri-sync-listener.ts's Tauri-
 * environment guard) and, even when it does fire, races an in-memory cache
 * (`ensureNode`) that serves the stale entry indefinitely once populated.
 * The persisted value itself was never at risk — the backend computed and
 * stored the correct title on every save — this was purely a frontend
 * store-staleness bug.
 */
describe('PersonSchemaForm — save path routes through the store (title-update regression)', () => {
  it('saves through the store, not the backend adapter, on a name edit', async () => {
    const adapterSpy = vi.spyOn(backendAdapter, 'updatePersonNode');
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const firstName = screen.getByLabelText('First name');
    await fireEvent.blur(firstName, { target: { value: 'Carol' } });

    expect(updatePersonNodeSpy).toHaveBeenCalledTimes(1);
    const [calledNodeId, update, source] = updatePersonNodeSpy.mock.calls[0];
    expect(calledNodeId).toBe('person-1');
    expect(update).toEqual({ firstName: 'Carol' });
    expect(source).toEqual({ type: 'viewer', viewerId: 'person-schema-form' });
    expect(adapterSpy).not.toHaveBeenCalled();
  });

  it('resolves the title to "{first_name} {last_name}" in the store immediately after editing, with no reload', async () => {
    // Exercise the REAL sharedNodeStore (not the spy the rest of this file
    // uses) so this test proves the actual store-mediated round trip, not
    // just that the component calls the right method name. Only the network
    // boundary (backendAdapter.updatePersonNode) is stubbed — and deliberately
    // returns NO `title` field at all, matching the real daemon's
    // previously-broken wire contract, which never sent one. The title must
    // still resolve correctly without it: per ADR-077 the editing client
    // computes its own title locally (see the "client-side title preview"
    // block below) rather than depending on this response.
    (sharedNodeStore.updateNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.updatePersonNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.getNode as unknown as ReturnType<typeof vi.fn>).mockRestore();

    const seeded = personNode({
      title: 'Untitled',
      properties: {}
    });
    sharedNodeStore.setNode(seeded, { type: 'database', reason: 'test-seed' }, true);

    // The response carries the typed shape every transport delivers — the
    // daemon's authoritative node, which is what used to wipe the fields.
    // The server keeps its own state across the two writes, as the daemon does.
    let serverFields: Record<string, unknown> = {};
    vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(async (id, version, update) => {
      serverFields = { ...serverFields, ...update };
      // Deliberately omit `title` from the response — `...seeded` would
      // otherwise leak `seeded.title` ("Untitled") back in, which is exactly
      // the stale-response-fighting-the-preview failure mode this test
      // exists to rule out. The real daemon previously never sent a
      // `title` at all; the fixed daemon's own title (once it lands) must
      // AGREE with, not fight, the value the client already computed below.
      const { title: _seededTitle, ...seededWithoutTitle } = seeded;
      return {
        ...seededWithoutTitle,
        ...serverFields,
        id,
        version: (version as number) + 1
      } as unknown as PersonNode;
    });

    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const firstName = screen.getByLabelText('First name') as HTMLInputElement;
    const lastName = screen.getByLabelText('Last name') as HTMLInputElement;
    await fireEvent.input(firstName, { target: { value: 'Jane' } });
    await fireEvent.blur(firstName, { target: { value: 'Jane' } });
    await fireEvent.input(lastName, { target: { value: 'Doe' } });
    await fireEvent.blur(lastName, { target: { value: 'Doe' } });

    await waitFor(() => expect(sharedNodeStore.getNode('person-1')?.title).toBe('Jane Doe'));
    // No manual reload/re-fetch performed above — the assertion above already
    // covers "no reload required".

    // The committed values stay visible once the daemon's node has landed.
    await waitFor(() => expect(sharedNodeStore.getNode('person-1')?.version).toBeGreaterThan(1));
    expect(firstName.value).toBe('Jane');
    expect(lastName.value).toBe('Doe');
  });
});

/**
 * Client-side reactive title preview (ADR-077).
 *
 * Per the ADR, the editing client computes its own title INSTANTLY from
 * in-progress field values — via `evaluateTitleTemplate`, wired to the
 * first/last name inputs' `oninput` — with no dependency on a completed
 * round trip or a `NodeUpdated` echo. The backend still independently
 * computes and persists the authoritative title on save (unchanged), but
 * this form's own UI (and every other reader of the store: header, tab,
 * inline row) must never need to wait for that response.
 */
describe('PersonSchemaForm — client-side title preview (ADR-077)', () => {
  it('computes and displays the title as the user types, before any blur and with the backend write never resolving', async () => {
    (sharedNodeStore.updateNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.updatePersonNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.getNode as unknown as ReturnType<typeof vi.fn>).mockRestore();

    const seeded = personNode({
      title: '',
      properties: {}
    });
    sharedNodeStore.setNode(seeded, { type: 'database', reason: 'test-seed' }, true);

    // Never resolves — proves the preview does not wait on this at all.
    vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(() => new Promise(() => {}));

    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const firstName = screen.getByLabelText('First name') as HTMLInputElement;
    const lastName = screen.getByLabelText('Last name') as HTMLInputElement;

    await fireEvent.input(firstName, { target: { value: 'Jane' } });
    expect(sharedNodeStore.getNode('person-1')?.title).toBe('Jane');

    await fireEvent.input(lastName, { target: { value: 'Doe' } });
    expect(sharedNodeStore.getNode('person-1')?.title).toBe('Jane Doe');

    // No blur fired at all above — no persisted write, and no RPC response,
    // was needed for the title preview to be correct.
  });

  it('a server-provided title from a normal fetch/first-load is displayed correctly and untouched by merely mounting the form', async () => {
    // ADR-077 point 6 / regression check: a server-provided title on a
    // normal read must never be ignored or clobbered.
    (sharedNodeStore.updateNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.updatePersonNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.getNode as unknown as ReturnType<typeof vi.fn>).mockRestore();

    const seeded = personNode({
      title: 'Server Computed Title',
      firstName: 'Server', lastName: 'Computed'
    });
    sharedNodeStore.setNode(seeded, { type: 'database', reason: 'test-seed' }, true);

    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    expect(sharedNodeStore.getNode('person-1')?.title).toBe('Server Computed Title');
  });

  it('does not push a redundant store update when the computed title already matches', async () => {
    (sharedNodeStore.updateNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.updatePersonNode as unknown as ReturnType<typeof vi.fn>).mockRestore();
    (sharedNodeStore.getNode as unknown as ReturnType<typeof vi.fn>).mockRestore();

    const seeded = personNode({
      title: 'Jane Doe',
      firstName: 'Jane', lastName: 'Doe'
    });
    sharedNodeStore.setNode(seeded, { type: 'database', reason: 'test-seed' }, true);

    const realUpdateNodeSpy = vi.spyOn(sharedNodeStore, 'updateNode');
    render(PersonSchemaForm, { props: { nodeId: 'person-1' } });

    const firstName = screen.getByLabelText('First name') as HTMLInputElement;
    // Retyping the value the store already resolves to: the preview
    // recomputes to an identical string, so no update should fire at all.
    await fireEvent.input(firstName, { target: { value: 'Jane' } });

    expect(realUpdateNodeSpy).not.toHaveBeenCalled();
  });
});
