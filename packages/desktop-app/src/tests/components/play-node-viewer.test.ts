/**
 * PlayNodeViewer (ADR-090 §2): a play's state, its rules as read-only lanes,
 * the switch's typed write, the seeded-play warning, and the redraw when the
 * play changes in the store from somewhere else.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { render, cleanup, fireEvent, within } from '@testing-library/svelte';
import { tick } from 'svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import PlayNodeViewer from '$lib/components/viewers/play-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { getNavigationService } from '$lib/services/navigation-service';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import type { Node, PlayNode } from '$lib/types';
import type { RuleDefinition } from '$lib/types/generated';

const PLAY_ID = '6d1f0c1e-2a5b-4f0e-9a51-0c7f6c1d9a01';
const loaded = { type: 'database', reason: 'test seed' } as const;
// What the sync listener applies when a `node:updated` arrives for the play.
const domainEvent = { type: 'database', reason: 'domain-event' } as const;

const closeParent: RuleDefinition = {
  name: 'close-parent',
  description: 'Marks a task done once all of its sub-tasks are done',
  class: 'reactive',
  trigger: {
    type: 'graph_event',
    on: 'property_changed',
    select: { target_type: 'task' },
    property_key: 'task.status'
  },
  conditions: [
    {
      expr: "node.child_of.has_child.all(status == 'done')",
      description: 'Every sibling sub-task is done'
    }
  ],
  actions: [
    {
      action_type: 'update_node',
      description: 'Set the parent task to done',
      params: { node_id: '{item.id}', properties: { status: 'done' } },
      for_each: 'trigger.node.child_of'
    }
  ]
};

const guard: RuleDefinition = {
  name: 'guard',
  description: 'Refuses a task with no title',
  class: 'invariant',
  trigger: { type: 'graph_event', on: 'node_created', select: { target_type: 'task' } },
  actions: [
    { action_type: 'reject', description: 'Refuse the write', params: { message: 'Needs a title' } }
  ]
};

function play(fields: Partial<PlayNode> = {}): Node {
  const node: PlayNode = {
    id: PLAY_ID,
    nodeType: 'play',
    content: 'Roll completion up',
    version: 1,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    properties: {},
    lifecycleStatus: 'active',
    rules: [closeParent],
    description: 'When every sub-task is done, mark the parent done',
    enabled: true,
    isSeeded: false,
    ...fields
  };
  return node as unknown as Node;
}

function open(fields: Partial<PlayNode> = {}) {
  sharedNodeStore.setNode(play(fields), loaded);
  return render(PlayNodeViewer, { props: { nodeId: PLAY_ID } });
}

describe('PlayNodeViewer', () => {
  let updatePlayNode: MockInstance<typeof backendAdapter.updatePlayNode>;

  beforeEach(() => {
    sharedNodeStore.clearAll();
    // The backend's reply: the play with the written fields applied.
    updatePlayNode = vi
      .spyOn(backendAdapter, 'updatePlayNode')
      .mockImplementation(
        async (_id, version, update) =>
          ({ ...play(), ...update, version: version + 1 }) as unknown as PlayNode
      );
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  describe('header', () => {
    it('shows the title, the description and the on state', () => {
      const { getByRole, getByText, container } = open();

      expect(getByRole('heading', { level: 1 }).textContent).toBe('Roll completion up');
      expect(getByText('When every sub-task is done, mark the parent done')).toBeTruthy();
      expect(container.querySelector('.play-state')?.textContent?.trim()).toBe('On');
      expect(getByRole('switch').getAttribute('aria-checked')).toBe('true');
      expect(container.querySelector('.play-suspension')).toBeNull();
    });

    it('shows the off state for a play switched off', () => {
      const { getByRole, container } = open({ enabled: false });

      expect(container.querySelector('.play-state')?.textContent?.trim()).toBe('Off');
      expect(getByRole('switch').getAttribute('aria-checked')).toBe('false');
    });

    it('shows a suspension with its message and time', () => {
      const suspendedAt = '2026-10-01T09:00:00Z';
      const { getByRole, container } = open({
        suspendedReason: 'action_failed',
        suspendedMessage: 'update_node failed: no such node',
        suspendedAt
      });

      expect(container.querySelector('.play-state')?.textContent?.trim()).toBe('Suspended');
      expect(getByRole('switch').getAttribute('aria-checked')).toBe('false');
      const suspension = getByRole('status');
      expect(suspension.textContent).toContain('update_node failed: no such node');
      expect(suspension.textContent).toContain(new Date(suspendedAt).toLocaleString());
    });
  });

  describe('switch', () => {
    it('writes enabled: false through the typed play update', async () => {
      const { getByRole } = open();

      await fireEvent.click(getByRole('switch'));

      await vi.waitFor(() =>
        expect(updatePlayNode).toHaveBeenCalledWith(PLAY_ID, 1, { enabled: false })
      );
    });

    it('sends enabled: true when a suspended play is switched on', async () => {
      const { getByRole } = open({
        suspendedReason: 'cycle_limit',
        suspendedAt: '2026-10-01T09:00:00Z'
      });

      await fireEvent.click(getByRole('switch'));

      await vi.waitFor(() =>
        expect(updatePlayNode).toHaveBeenCalledWith(PLAY_ID, 1, { enabled: true })
      );
    });

    it('switches off a seeded play with no invariant rule without a warning', async () => {
      const { getByRole, queryByRole } = open({ isSeeded: true });

      await fireEvent.click(getByRole('switch'));

      expect(queryByRole('dialog')).toBeNull();
      await vi.waitFor(() =>
        expect(updatePlayNode).toHaveBeenCalledWith(PLAY_ID, 1, { enabled: false })
      );
    });
  });

  describe('seeded-play warning', () => {
    const seeded = { isSeeded: true, rules: [guard, closeParent] };

    it("names the invariant rule's effect before switching off, and writes on confirm", async () => {
      const { getByRole } = open(seeded);

      await fireEvent.click(getByRole('switch'));

      const dialog = getByRole('dialog');
      expect(dialog.textContent).toContain('Refuses a task with no title');
      // The rule's effect is in the dialog's accessible description.
      const description = document.getElementById(dialog.getAttribute('aria-describedby') ?? '');
      expect(description?.textContent).toContain('Refuses a task with no title');
      expect(dialog.textContent).not.toContain(closeParent.description);
      expect(updatePlayNode).not.toHaveBeenCalled();

      await fireEvent.click(within(dialog).getByRole('button', { name: 'Turn off' }));

      await vi.waitFor(() =>
        expect(updatePlayNode).toHaveBeenCalledWith(PLAY_ID, 1, { enabled: false })
      );
    });

    it('writes nothing on Cancel and leaves the play on', async () => {
      const { getByRole, queryByRole, container } = open(seeded);

      await fireEvent.click(getByRole('switch'));
      await fireEvent.click(within(getByRole('dialog')).getByRole('button', { name: 'Cancel' }));

      expect(queryByRole('dialog')).toBeNull();
      expect(updatePlayNode).not.toHaveBeenCalled();
      expect(getByRole('switch').getAttribute('aria-checked')).toBe('true');
      expect(container.querySelector('.play-state')?.textContent?.trim()).toBe('On');
    });

    it('does not warn when a seeded play is switched on', async () => {
      const { getByRole, queryByRole } = open({ ...seeded, enabled: false });

      await fireEvent.click(getByRole('switch'));

      expect(queryByRole('dialog')).toBeNull();
      await vi.waitFor(() =>
        expect(updatePlayNode).toHaveBeenCalledWith(PLAY_ID, 1, { enabled: true })
      );
    });
  });

  describe('rules', () => {
    it('shows one lane per rule, in order, with an invariant marker on invariant rules', () => {
      const { container } = open({ rules: [guard, closeParent] });

      const lanes = [...container.querySelectorAll('.rule-lane')];
      expect(lanes.map((lane) => lane.querySelector('h2')?.textContent)).toEqual([
        'guard',
        'close-parent'
      ]);
      expect(lanes[0].querySelector('.invariant-marker')).not.toBeNull();
      expect(lanes[1].querySelector('.invariant-marker')).toBeNull();
    });

    it('still shows a play whose rules repeat a name', () => {
      // Validation rejects this on write; a play stored around it must render.
      const { container } = open({ rules: [guard, { ...guard, description: 'A second guard' }] });

      expect(container.querySelectorAll('.rule-lane')).toHaveLength(2);
    });

    it('describes the trigger, each condition and each action, naming the for_each collection', () => {
      const { container } = open();

      const steps = [...container.querySelectorAll('.step')];
      expect(steps.map((step) => step.getAttribute('data-step'))).toEqual([
        'trigger',
        'condition',
        'action'
      ]);
      const [trigger, condition, action] = steps;
      expect(trigger.querySelector('.step-description')?.textContent).toBe(
        'When status changes on a task'
      );
      expect(condition.querySelector('.step-description')?.textContent).toBe(
        'Every sibling sub-task is done'
      );
      expect(action.querySelector('.step-description')?.textContent).toBe(
        'Set the parent task to done'
      );
      expect(action.querySelector('.step-for-each')?.textContent).toContain(
        'trigger.node.child_of'
      );
    });

    it('keeps the raw CEL and params behind a closed disclosure', () => {
      const { container } = open();

      const raw = [...container.querySelectorAll('details.step-raw')];
      expect(raw).toHaveLength(3);
      expect(raw.every((details) => !details.hasAttribute('open'))).toBe(true);
      expect(raw[1].querySelector('pre')?.textContent).toBe(
        "node.child_of.has_child.all(status == 'done')"
      );
      expect(raw[2].querySelector('pre')?.textContent).toContain('"node_id": "{item.id}"');
    });

    it('offers nothing to edit in place: its controls are the switch and Edit', () => {
      const { container } = open({ rules: [guard, closeParent] });

      expect(
        container.querySelectorAll('input, textarea, select, [contenteditable="true"]')
      ).toHaveLength(0);
      const buttons = Array.from(container.querySelectorAll('button'));
      expect(buttons).toHaveLength(2);
      expect(buttons[0].getAttribute('role')).toBe('switch');
      expect(buttons[1].textContent?.trim()).toBe('Edit');
    });
  });

  describe('Edit', () => {
    const chat = {
      id: 'chat-1',
      nodeType: 'ai-chat-native',
      content: 'Edit Roll completion up',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {}
    } as unknown as Node;

    it('creates a chat bound to the play and opens it beside the play', async () => {
      const createPlayEditChat = vi.spyOn(aiChatsData, 'createPlayEditChat').mockResolvedValue(chat);
      const navigateToNodeInOtherPane = vi
        .spyOn(getNavigationService(), 'navigateToNodeInOtherPane')
        .mockResolvedValue();
      const { getByRole, container } = open();

      await fireEvent.click(getByRole('button', { name: 'Edit' }));
      await tick();

      expect(createPlayEditChat).toHaveBeenCalledWith(PLAY_ID);
      expect(navigateToNodeInOtherPane).toHaveBeenCalledWith('chat-1');
      expect(container.querySelector('.edit-error')).toBeNull();
      // Opening the chat writes nothing to the play.
      expect(updatePlayNode).not.toHaveBeenCalled();
    });

    it('makes a new chat on every click', async () => {
      const createPlayEditChat = vi.spyOn(aiChatsData, 'createPlayEditChat').mockResolvedValue(chat);
      vi.spyOn(getNavigationService(), 'navigateToNodeInOtherPane').mockResolvedValue();
      const { getByRole } = open();

      await fireEvent.click(getByRole('button', { name: 'Edit' }));
      await tick();
      await fireEvent.click(getByRole('button', { name: 'Edit' }));
      await tick();

      expect(createPlayEditChat).toHaveBeenCalledTimes(2);
    });

    it('says why when the chat could not be created, and opens nothing', async () => {
      vi.spyOn(aiChatsData, 'createPlayEditChat').mockImplementation(async () => {
        aiChatsData.createError = 'the daemon is not running';
        return null;
      });
      const navigateToNodeInOtherPane = vi
        .spyOn(getNavigationService(), 'navigateToNodeInOtherPane')
        .mockResolvedValue();
      const { getByRole } = open();

      await fireEvent.click(getByRole('button', { name: 'Edit' }));
      await tick();

      expect(getByRole('alert').textContent).toContain('the daemon is not running');
      expect(navigateToNodeInOtherPane).not.toHaveBeenCalled();
      aiChatsData.reset();
    });
  });

  describe('live redraw', () => {
    it('follows a write from elsewhere with no reload', async () => {
      const { getByRole, container } = open();

      // A chat or the CLI rewrote the rules and the engine then suspended the
      // play: the listener applies the re-fetched node to the store.
      sharedNodeStore.setNode(
        play({
          version: 2,
          description: 'Refuses untitled tasks',
          rules: [guard],
          suspendedReason: 'validation_failed',
          suspendedMessage: 'rule guard: unknown field',
          suspendedAt: '2026-10-02T10:00:00Z'
        }),
        domainEvent
      );
      await tick();

      expect(container.querySelector('.play-description')?.textContent).toBe(
        'Refuses untitled tasks'
      );
      expect(
        [...container.querySelectorAll('.rule-lane h2')].map((heading) => heading.textContent)
      ).toEqual(['guard']);
      expect(container.querySelector('.play-state')?.textContent?.trim()).toBe('Suspended');
      expect(getByRole('status').textContent).toContain('rule guard: unknown field');
    });
  });
});
