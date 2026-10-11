/**
 * TaskSchemaForm — real component regression coverage.
 *
 * TaskSchemaForm used to carry its own hardcoded STATUS_OPTIONS/PRIORITY_OPTIONS
 * enum constants and bespoke date-picker markup, duplicating what the real task
 * schema's coreValues/userValues and SchemaFieldLeaf already provide. That
 * duplication was removed: the core fields now render through the shared
 * SchemaFieldLeaf (driven by the schema TaskSchemaForm fetches from the backend),
 * and the Collapsible shell / trigger row / gated Relationships button / nested-
 * field modal now come from the shared TypedFormShell (also used by
 * GenericSchemaForm) instead of a second hand-rolled copy.
 *
 * These tests exercise the REAL rendered component (not a shadow copy of its old
 * internal logic — the previous version of this file re-implemented STATUS_OPTIONS/
 * PRIORITY_OPTIONS/formatEnumLabel/etc. as free functions and tested those, never
 * the component itself) against the actual task schema shape (mirroring
 * core_schemas.rs), because this is "the most heavily-used property form in the
 * app" and needs real scrutiny, not a test of a copy of old code that no longer
 * exists:
 *   - status/priority render options sourced from the SCHEMA the component is
 *     given, not a hardcoded fallback list (a schema with different core values
 *     than the shipped task schema is used specifically to prove this)
 *   - core-field edits still go through sharedNodeStore.updateTaskNode (the
 *     type-safe, OCC/field-sequenced write path), never the generic
 *     sharedNodeStore.updateNode path — SchemaFieldLeaf is purely presentational,
 *     so swapping its markup in must not change how a core field is persisted
 *   - the date fields round-trip through the same YYYY-MM-DD storage format as
 *     before
 *   - the Relationships button is now gated on the node's type actually having a
 *     typed relationship, matching GenericSchemaForm — previously it showed
 *     unconditionally, a real inconsistency that has now been fixed
 *   - user-defined (non-core) schema fields still render dynamically
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import TaskSchemaForm from '$lib/components/property-forms/task-schema-form.svelte';
import IssueSchemaForm from '$lib/components/property-forms/issue-schema-form.svelte';
import {
  buildRelationshipsView,
  type RawRelationshipGroup
} from '$lib/services/relationship-grouping';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

function enumField(
  name: string,
  friendlyName: string,
  coreValues: Array<{ value: string; label: string }>,
  opts: Partial<SchemaField> = {}
): SchemaField {
  return {
    name,
    friendlyName,
    type: 'enum',
    protection: 'user',
    indexed: true,
    coreValues,
    userValues: [],
    required: false,
    ...opts
  };
}

function dateField(name: string, friendlyName: string): SchemaField {
  return { name, friendlyName, type: 'date', protection: 'user', indexed: false, required: false };
}

function linkField(name: string, friendlyName: string): SchemaField {
  return { name, friendlyName, type: 'link', protection: 'user', indexed: false, required: false };
}

/** Mirrors the real `task` SchemaNode exactly as declared in core_schemas.rs. */
function realTaskSchema(): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id: 'task',
    content: 'Task',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: true,
    schemaVersion: 1,
    relationships: [],
    fields: [
      enumField(
        'status',
        'Status',
        [
          { value: 'open', label: 'Open' },
          { value: 'in_progress', label: 'In Progress' },
          { value: 'in_review', label: 'In Review' },
          { value: 'done', label: 'Done' },
          { value: 'cancelled', label: 'Cancelled' }
        ],
        { required: true, default: 'open', protection: 'core', extensible: true }
      ),
      enumField(
        'priority',
        'Priority',
        [
          { value: 'highest', label: 'Highest' },
          { value: 'high', label: 'High' },
          { value: 'medium', label: 'Medium' },
          { value: 'low', label: 'Low' },
          { value: 'lowest', label: 'Lowest' }
        ],
        { extensible: true }
      ),
      dateField('due_date', 'Due date'),
      dateField('started_at', 'Started at'),
      dateField('completed_at', 'Completed at')
    ]
  };
}

/**
 * The `issue` schema as a schema read returns it: the effective fields, so
 * the ones inherited from task, the issue's own status with its two added
 * values, and the two delivery links.
 */
function realIssueSchema(): SchemaNode {
  const task = realTaskSchema();
  const status = task.fields.find((f) => f.name === 'status')!;
  return {
    ...task,
    id: 'issue',
    content: 'Issue',
    extends: 'task',
    fields: [
      {
        ...status,
        coreValues: [
          ...(status.coreValues ?? []),
          { value: 'changes_requested', label: 'Changes Requested', mapsTo: 'in_progress' },
          { value: 'ready_to_merge', label: 'Ready to Merge', mapsTo: 'in_review' }
        ]
      },
      linkField('pull_request', 'Pull request'),
      {
        name: 'commits',
        friendlyName: 'Commits',
        type: 'array',
        itemType: 'link',
        protection: 'user',
        indexed: false,
        required: false
      },
      ...task.fields.filter((f) => f.name !== 'status')
    ]
  };
}

function issueNode(overrides: Record<string, unknown> = {}): Node {
  return {
    id: 'issue-1',
    nodeType: 'issue',
    content: 'Ship the change',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    status: 'open',
    ...overrides
  } as unknown as Node;
}

function taskNode(overrides: Record<string, unknown> = {}): Node {
  return {
    id: 'task-1',
    nodeType: 'task',
    content: 'Ship the thing',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    status: 'open',
    ...overrides
  } as unknown as Node;
}

/**
 * A task's relationships view as the daemon shapes it: task's own many-to-many
 * `blocks`, plus the inbound `one` ends `assignee` and `project` (declared
 * outbound as `tasks` on person and project).
 */
function taskRelationships(opts: { assignee?: string; blocks?: boolean } = {}) {
  const base = {
    reverseName: '',
    sourceType: 'task',
    farCardinality: 'many' as const,
    required: null,
    edgeFields: null,
    description: null,
    related: [],
    count: 0
  };
  const groups: RawRelationshipGroup[] = [
    {
      ...base,
      relationshipName: 'tasks',
      direction: 'in',
      targetType: 'person',
      sourceType: 'person',
      reverseName: 'assignee',
      cardinality: 'one',
      related: opts.assignee
        ? [{ id: 'person-1', nodeType: 'person', title: opts.assignee, contentPreview: '', edgeProperties: {} }]
        : [],
      count: opts.assignee ? 1 : 0
    },
    {
      ...base,
      relationshipName: 'tasks',
      direction: 'in',
      targetType: 'project',
      sourceType: 'project',
      reverseName: 'project',
      cardinality: 'one'
    }
  ];
  if (opts.blocks !== false) {
    groups.unshift({
      ...base,
      relationshipName: 'blocks',
      direction: 'out',
      targetType: 'task',
      reverseName: 'blocked_by',
      cardinality: 'many'
    });
  }
  return buildRelationshipsView({ nodeId: 'task-1', nodeType: 'task', groups });
}

let updateTaskNodeSpy: ReturnType<typeof vi.fn>;
let updateNodeSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateTaskNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'updateTaskNode').mockImplementation(
    updateTaskNodeSpy as unknown as typeof sharedNodeStore.updateTaskNode
  );
  updateNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );
  // Re-armed every test (not just restored) — `vi.restoreAllMocks()` below clears a
  // bare `vi.fn()`'s implementation entirely rather than restoring one, so without a
  // default here, every test after the first to leave it unset would see `undefined`
  // and throw calling `.then()` on it inside TypedFormShell's relationships-gate effect.
  loadNodeRelationshipsView.mockReset();
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
  vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(realTaskSchema() as never);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** Opens the form's Collapsible (collapsed by default) — its trigger is the first button. */
async function openForm(container: HTMLElement): Promise<void> {
  await waitFor(() => expect(container.querySelector('button')).toBeTruthy());
  await fireEvent.click(container.querySelector('button')!);
}

describe('TaskSchemaForm — core fields render from the fetched schema, not a hardcoded list', () => {
  it('shows the real task schema\'s status/priority options and date field labels', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    // "Open" appears twice once expanded: the collapsed-header status badge (always
    // rendered) and the Status field's own Select trigger.
    await waitFor(() => expect(screen.getAllByText('Open').length).toBeGreaterThanOrEqual(2));
    expect(screen.getByText('Select Priority...')).toBeTruthy();
    expect(screen.getAllByText('Pick a date')).toHaveLength(3); // due date, started at, completed at
  });

  it('renders whatever core status values the SCHEMA declares, not a hardcoded fallback list', async () => {
    // A schema whose status coreValues are completely different from the shipped
    // task schema's open/in_progress/done/cancelled. If the component still had a
    // local hardcoded fallback, this value would either fail to resolve to a label
    // or the control would ignore the schema entirely — this pins that it does not.
    const customSchema = realTaskSchema();
    const statusField = customSchema.fields.find((f) => f.name === 'status')!;
    statusField.coreValues = [
      { value: 'todo', label: 'To Do' },
      { value: 'complete', label: 'Complete' }
    ];
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(customSchema as never);
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode({ status: 'todo' }));

    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    await waitFor(() => expect(screen.getAllByText('To Do').length).toBeGreaterThanOrEqual(1));
  });

  it('the collapsed header humanizes a user-extended status value via the same schema lookup', async () => {
    const customSchema = realTaskSchema();
    const statusField = customSchema.fields.find((f) => f.name === 'status')!;
    statusField.userValues = [{ value: 'blocked', label: 'Blocked' }];
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(customSchema as never);
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode({ status: 'blocked' }));

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    // The header badge is visible even while the Collapsible is closed (bits-ui keeps
    // Collapsible.Content mounted-but-hidden, so the Select trigger's own copy of the
    // same label may also be present in the DOM — assert at least one is showing).
    await waitFor(() => expect(screen.getAllByText('Blocked').length).toBeGreaterThanOrEqual(1));
  });
});

describe('TaskSchemaForm — schema-fetch failure degrades gracefully (no blank crash)', () => {
  it('shows an "Unable to load" hint for core fields instead of silently leaving them blank forever', async () => {
    vi.spyOn(backendAdapter, 'getSchema').mockRejectedValue(new Error('daemon offline'));
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    // 5 core fields (status, priority, due date, started at, completed at) each fall back
    // to the unavailable hint.
    await waitFor(() => expect(screen.getAllByText('Unable to load')).toHaveLength(5));
  });

  it('does not show "Unable to load" while the schema fetch is merely still in flight', async () => {
    let resolveSchema!: (schema: unknown) => void;
    vi.spyOn(backendAdapter, 'getSchema').mockReturnValue(
      new Promise((resolve) => {
        resolveSchema = resolve;
      }) as never
    );
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    expect(screen.queryByText('Unable to load')).toBeNull();

    resolveSchema(realTaskSchema());
    await waitFor(() => expect(screen.getAllByText('Open').length).toBeGreaterThanOrEqual(1));
    expect(screen.queryByText('Unable to load')).toBeNull();
  });
});

describe('TaskSchemaForm — a plain task has no delivery links', () => {
  it('renders no pull request or commits field', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    await waitFor(() => expect(container.querySelector('#task-status')).toBeTruthy());
    expect(container.querySelector('#task-pull-request')).toBeNull();
    expect(container.querySelector('#task-commits')).toBeNull();
    expect(screen.queryByText('Pull Request')).toBeNull();
    expect(screen.queryByText('Commits')).toBeNull();
  });

  it('renders nothing for an issue: an issue has its own form', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(issueNode());
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'issue-1' } });

    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(container.querySelector('button')).toBeNull();
  });
});

describe('IssueSchemaForm — the task form with the issue\'s own fields', () => {
  let updateIssueNodeSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    updateIssueNodeSpy = vi.fn();
    vi.spyOn(sharedNodeStore, 'updateIssueNode').mockImplementation(
      updateIssueNodeSpy as unknown as typeof sharedNodeStore.updateIssueNode
    );
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'issue', groups: [] });
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(realIssueSchema() as never);
  });

  it('loads the issue schema, and shows the pull request and the commit count', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      issueNode({
        pullRequest: { title: 'Add the review status', url: 'https://example.com/pr/7' },
        commits: [
          { title: 'abc123', url: 'https://example.com/c/abc123' },
          { title: 'def456', url: 'https://example.com/c/def456' }
        ]
      })
    );
    const { container } = render(IssueSchemaForm, { props: { nodeId: 'issue-1' } });
    await openForm(container);

    await waitFor(() => expect(screen.getByText('Add the review status')).toBeTruthy());
    expect(screen.getByText('2 items')).toBeTruthy();
    expect(backendAdapter.getSchema).toHaveBeenCalledWith('issue');
  });

  it('counts the delivery links in the completion badge, out of 7 core fields', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      issueNode({
        status: 'open',
        pullRequest: { title: 'PR', url: 'https://example.com/pr/1' },
        commits: [{ title: 'abc', url: 'https://example.com/c/abc' }]
      })
    );
    render(IssueSchemaForm, { props: { nodeId: 'issue-1' } });

    await waitFor(() => expect(screen.getByText('3/7 fields')).toBeTruthy());
  });

  it('shows an issue status by the label the issue schema gives it', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      issueNode({ status: 'changes_requested' })
    );
    render(IssueSchemaForm, { props: { nodeId: 'issue-1' } });

    // In the collapsed header's badge, and in the status control once it renders.
    await waitFor(() =>
      expect(screen.getAllByText('Changes Requested').length).toBeGreaterThanOrEqual(1)
    );
    expect(screen.queryByText('changes_requested')).toBeNull();
  });

  it('writes an inherited field through updateIssueNode, never a task\'s update', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(issueNode());
    const { container } = render(IssueSchemaForm, { props: { nodeId: 'issue-1' } });
    await openForm(container);

    await waitFor(() => expect(screen.getByLabelText('Due Date')).toBeTruthy());
    await fireEvent.click(screen.getByLabelText('Due Date'));
    const day = await waitFor(() => {
      const cell = document.querySelector('[data-bits-day]:not([data-outside-month])');
      expect(cell).toBeTruthy();
      return cell as Element;
    });
    await fireEvent.click(day);

    await waitFor(() => expect(updateIssueNodeSpy).toHaveBeenCalled());
    const [nodeId, update, source] = updateIssueNodeSpy.mock.calls[0];
    expect(nodeId).toBe('issue-1');
    expect(Object.keys(update)).toEqual(['dueDate']);
    expect(source).toEqual({ type: 'viewer', viewerId: 'issue-schema-form' });
    expect(updateTaskNodeSpy).not.toHaveBeenCalled();
    expect(updateNodeSpy).not.toHaveBeenCalled();
  });

  it('renders nothing for a plain task', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());
    const { container } = render(IssueSchemaForm, { props: { nodeId: 'task-1' } });

    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(container.querySelector('button')).toBeNull();
  });
});

describe('TaskSchemaForm — core-field writes still go through updateTaskNode', () => {
  it('writes a due-date edit through sharedNodeStore.updateTaskNode, never the generic properties path', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    await waitFor(() => expect(screen.getByLabelText('Due Date')).toBeTruthy());
    await fireEvent.click(screen.getByLabelText('Due Date'));
    const dayButton = await waitFor(() => {
      const btn = document.querySelector('[data-bits-day]:not([data-outside-month])');
      expect(btn).toBeTruthy();
      return btn as Element;
    });
    await fireEvent.click(dayButton);

    await waitFor(() => expect(updateTaskNodeSpy).toHaveBeenCalledTimes(1));
    const [id, update] = updateTaskNodeSpy.mock.calls[0] as [string, { dueDate: string }];
    expect(id).toBe('task-1');
    // Same YYYY-MM-DD storage format as before the refactor.
    expect(update.dueDate).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    // SchemaFieldLeaf is presentational only — the write path is unchanged, not
    // rerouted through the generic per-field properties write.
    expect(updateNodeSpy).not.toHaveBeenCalled();
  });

  it('writes a started-at edit through updateTaskNode with the startedAt field name', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    await waitFor(() => expect(screen.getByLabelText('Started At')).toBeTruthy());
    await fireEvent.click(screen.getByLabelText('Started At'));
    const dayButton = await waitFor(() => {
      const btn = document.querySelector('[data-bits-day]:not([data-outside-month])');
      expect(btn).toBeTruthy();
      return btn as Element;
    });
    await fireEvent.click(dayButton);

    await waitFor(() => expect(updateTaskNodeSpy).toHaveBeenCalledTimes(1));
    const [, update] = updateTaskNodeSpy.mock.calls[0] as [string, { startedAt: string }];
    expect(update.startedAt).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });
});

describe('TaskSchemaForm — Relationships button is now gated (previously unconditional)', () => {
  it('hides the Relationships entry point when the type has no typed relationships', async () => {
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(loadNodeRelationshipsView).toHaveBeenCalledWith('task-1'));
    // `hasRelationships` already defaults to false, so a fixed number of ticks here would
    // pass vacuously even if the gate's `.then()` callback never actually ran. Await the
    // EXACT promise the component received (proving its `.then()` has been scheduled),
    // then flush one more microtask for that callback's own body to execute.
    await loadNodeRelationshipsView.mock.results[0].value;
    await Promise.resolve();
    expect(screen.queryByText('Relationships')).toBeNull();
  });

  it('shows the Relationships entry point when the type has a typed relationship', async () => {
    loadNodeRelationshipsView.mockResolvedValue(taskRelationships());
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });

  it('hides the Relationships entry point when every relationship is a form field', async () => {
    loadNodeRelationshipsView.mockResolvedValue(taskRelationships({ blocks: false }));
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByText('Assignee')).toBeTruthy());
    expect(screen.queryByText('Relationships')).toBeNull();
  });
});

describe('TaskSchemaForm — single-valued relationships as fields', () => {
  it('renders Assignee and Project as fields, showing the current assignee', async () => {
    loadNodeRelationshipsView.mockResolvedValue(taskRelationships({ assignee: 'Sam Lee' }));
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByLabelText('Assignee')).toBeTruthy());
    expect(screen.getByLabelText('Assignee').textContent).toContain('Sam Lee');
    // Project has no edge yet: an empty search field.
    expect((screen.getByLabelText('Project') as HTMLInputElement).value).toBe('');
  });

  it('counts promoted relationship fields in the badge', async () => {
    loadNodeRelationshipsView.mockResolvedValue(taskRelationships({ assignee: 'Sam Lee' }));
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    // taskNode() fills status only (1/5 core); +1 filled assignee of 2 promoted.
    await waitFor(() => expect(screen.getByText('2/7 fields')).toBeTruthy());
  });

  it('fails open (shows the trigger) when the relationship check errors', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode());

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByText('Relationships')).toBeTruthy());
  });
});

describe('TaskSchemaForm — field completion badge', () => {
  it('counts filled core fields out of 5', async () => {
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      taskNode({ status: 'open', priority: 'high', dueDate: '2026-12-31' })
    );
    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByText('3/5 fields')).toBeTruthy());
  });

  it('includes user-defined schema fields in the total', async () => {
    const schema = realTaskSchema();
    schema.fields.push({
      name: 'estimate',
      friendlyName: 'Estimate',
      type: 'number',
      protection: 'user',
      indexed: false,
      required: false
    });
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(schema as never);
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode({ status: 'open' }));

    render(TaskSchemaForm, { props: { nodeId: 'task-1' } });

    await waitFor(() => expect(screen.getByText('1/6 fields')).toBeTruthy());
  });
});

describe('TaskSchemaForm — user-defined fields still render dynamically', () => {
  it('renders a non-core schema field through SchemaFieldLeaf, read and written flat', async () => {
    const schema = realTaskSchema();
    schema.fields.push({
      name: 'sprint',
      friendlyName: 'Sprint',
      type: 'text',
      protection: 'user',
      indexed: false,
      required: false
    });
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(schema as never);
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      taskNode({ properties: { sprint: 'Sprint 12' } })
    );

    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    const input = (await waitFor(() => screen.getByLabelText('Sprint'))) as HTMLInputElement;
    expect(input.value).toBe('Sprint 12');

    await fireEvent.input(input, { target: { value: 'Sprint 13' } });

    expect(updateNodeSpy).toHaveBeenCalledTimes(1);
    const [, changes] = updateNodeSpy.mock.calls[0] as [string, Partial<Node>];
    expect(changes.properties).toEqual({ sprint: 'Sprint 13' });
  });

  it('does not render a system-protected field as an editable control', async () => {
    const schema = realTaskSchema();
    schema.fields.push({
      name: 'sprint',
      friendlyName: 'Sprint',
      type: 'text',
      protection: 'user',
      indexed: false,
      required: false
    });
    schema.fields.push({
      name: 'transcript',
      friendlyName: 'Transcript',
      type: 'text',
      protection: 'system',
      indexed: false,
      required: false
    });
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(schema as never);
    loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'task', groups: [] });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(taskNode({ status: 'open' }));

    const { container } = render(TaskSchemaForm, { props: { nodeId: 'task-1' } });
    await openForm(container);

    // The user-protected sibling proves the dynamic field loop ran at all, so the
    // absence of the system field below is the filter working, not a form that
    // rendered nothing.
    await waitFor(() => expect(screen.getByLabelText('Sprint')).toBeTruthy());
    expect(screen.queryByLabelText('Transcript')).toBeNull();
  });
});
