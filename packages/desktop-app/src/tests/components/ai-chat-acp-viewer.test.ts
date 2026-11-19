/**
 * AiChatAcpNodeViewer: renders an ACP chat's stored messages (text, thought,
 * tool-call, plan and system parts) and the live session state a source
 * supplies (permission card, turn indicator, session bar), against fixtures.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import AiChatAcpNodeViewer from '$lib/components/viewers/ai-chat-acp-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import type { Node } from '$lib/types/node';
import { FixtureAcpSession, FOUR_OPTION_REQUEST } from '../helpers/acp-session-fixture.svelte';

const CHAT_ID = 'acp-chat-1';
const databaseSource = { type: 'database' as const, reason: 'test' };

function seedChat(sessionStatus: 'active' | 'ended' = 'active'): void {
  sharedNodeStore.setNode(
    {
      id: CHAT_ID,
      nodeType: 'ai-chat-acp',
      content: 'Fix the build',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      agent: 'claude-code',
      sessionStatus,
      sessionId: 'sess-1'
    } as unknown as Node,
    databaseSource
  );
}

function seedChild(
  id: string,
  content: string,
  fields: Record<string, unknown>,
  order: number
): void {
  sharedNodeStore.setNode(
    {
      id,
      nodeType: 'ai-chat-message',
      content,
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      mentions: [],
      ...fields
    } as unknown as Node,
    databaseSource
  );
  structureTree.addInMemoryRelationship(CHAT_ID, id, order);
}

function seedConversation(): void {
  seedChild('m-1', 'Fix the failing build', { role: 'user' }, 1);
  seedChild('m-2', 'Checking the logs first', { role: 'assistant', partKind: 'thought' }, 2);
  seedChild(
    'm-3',
    '',
    {
      role: 'assistant',
      partKind: 'tool_call',
      toolCallId: 'call_1',
      toolTitle: 'Read build.log',
      toolKind: 'read',
      toolStatus: 'completed'
    },
    3
  );
  seedChild('m-4', '- [x] read the log\n- [ ] fix the import', { role: 'assistant', partKind: 'plan' }, 4);
  seedChild('m-5', 'The import path was wrong.', { role: 'assistant', partKind: 'text' }, 5);
  seedChild('m-6', 'The agent stopped: it refused to continue.', { role: 'system' }, 6);
}

describe('AiChatAcpNodeViewer', () => {
  beforeEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = true;
    seedChat();
  });

  afterEach(() => {
    cleanup();
    structureTree.removeNode(CHAT_ID);
    labsFlags.aiChatEnabled = false;
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it('is gated by the AI chat labs flag', async () => {
    labsFlags.aiChatEnabled = false;
    seedConversation();
    const { findByRole, queryByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    const notice = await findByRole('status');
    expect(notice.textContent).toMatch(/AI chat is turned off/i);
    expect(queryByText('Fix the failing build')).toBeNull();
  });

  it('renders each part as its own row, in conversation order', async () => {
    seedConversation();
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('The import path was wrong.');

    const rows = [...container.querySelectorAll('[data-acp-row]')].map((el) => [
      el.getAttribute('data-message-id'),
      el.getAttribute('data-acp-row')
    ]);
    expect(rows).toEqual([
      ['m-1', 'user'],
      ['m-2', 'thought'],
      ['m-3', 'tool'],
      ['m-4', 'plan'],
      ['m-5', 'text'],
      ['m-6', 'notice']
    ]);
  });

  it('shows a tool call with its title, kind and status in words', async () => {
    seedConversation();
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('Read build.log');
    const row = container.querySelector('[data-acp-row="tool"]') as HTMLElement;
    expect(row.textContent).toContain('Read');
    expect(row.textContent).toContain('Done');
    expect(row.getAttribute('data-status')).toBe('completed');
  });

  it('renders a tool call part with no tool fields set, as the schema accepts one', async () => {
    seedChild('m-bare', '', { role: 'assistant', partKind: 'tool_call' }, 1);
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('Tool call');
    const row = container.querySelector('[data-acp-row="tool"]') as HTMLElement;
    expect(row.getAttribute('data-message-id')).toBe('m-bare');
    expect(row.getAttribute('data-status')).toBe('pending');
    expect(row.getAttribute('data-tool-call-id')).toBeNull();
    expect(row.textContent).toContain('Waiting');
  });

  it('updates a tool call in place when its status changes', async () => {
    seedConversation();
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('Read build.log');
    sharedNodeStore.updateNode('m-3', { toolStatus: 'failed' } as unknown as Partial<Node>, databaseSource);
    await waitFor(() => {
      const row = container.querySelector('[data-acp-row="tool"]') as HTMLElement;
      expect(row.textContent).toContain('Failed');
    });
    expect(container.querySelectorAll('[data-acp-row="tool"]').length).toBe(1);
  });

  it('renders the plan as markdown under a Plan heading', async () => {
    seedConversation();
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('The import path was wrong.');
    const plan = container.querySelector('[data-acp-row="plan"]') as HTMLElement;
    expect(plan.textContent).toContain('Plan');
    await waitFor(() => expect(plan.querySelectorAll('li').length).toBe(2));
    expect(plan.querySelector('details')?.open).toBe(true);
  });

  it('keeps the thought collapsed and labelled', async () => {
    seedConversation();
    const { container, findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('The import path was wrong.');
    const details = container.querySelector('[data-acp-row="thought"] details') as HTMLElement & { open: boolean };
    expect(details.open).toBe(false);
    expect(details.querySelector('summary')?.textContent).toContain('Thinking');
  });

  it('shows the session state in words, and the agent', async () => {
    seedConversation();
    const { findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID } });
    await findByText('Session active');
    expect(await findByText('claude-code')).toBeTruthy();
  });

  it('shows an ended session as ended, with no input', async () => {
    cleanup();
    seedChat('ended');
    const session = new FixtureAcpSession();
    const { findByText, queryByLabelText } = render(AiChatAcpNodeViewer, {
      props: { nodeId: CHAT_ID, session }
    });
    await findByText('Session ended');
    expect(queryByLabelText('Chat message input')).toBeNull();
  });

  it('is read-only without a session source', async () => {
    seedConversation();
    const { findByText, queryByLabelText, queryByRole } = render(AiChatAcpNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    await findByText('The import path was wrong.');
    expect(queryByLabelText('Chat message input')).toBeNull();
    expect(queryByRole('alert')).toBeNull();
  });

  describe('permission card', () => {
    it('offers only the once-only choices and a cancel, never an always choice', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole, queryByText } = render(AiChatAcpNodeViewer, {
        props: { nodeId: CHAT_ID, session }
      });
      const card = await findByRole('alert');
      const buttons = [...card.querySelectorAll('button')].map((b) => b.getAttribute('data-option-id'));
      expect(buttons).toEqual(['allow-once', 'reject-once', 'cancel']);
      expect(queryByText(/always/i)).toBeNull();
      expect(card.textContent).toContain('Run `rm -rf build`');
    });

    it('answers with the chosen option id', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const card = await findByRole('alert');
      await fireEvent.click(card.querySelector('[data-option-id="allow-once"]') as HTMLElement);
      await waitFor(() =>
        expect(session.answers).toEqual([{ requestId: 'req-1', outcome: { selected: 'allow-once' } }])
      );
    });

    it('answers cancelled when the request is cancelled', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const card = await findByRole('alert');
      await fireEvent.click(card.querySelector('[data-option-id="cancel"]') as HTMLElement);
      await waitFor(() =>
        expect(session.answers).toEqual([{ requestId: 'req-1', outcome: 'cancelled' }])
      );
    });

    it('answers once: further clicks on the same request are ignored', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const card = await findByRole('alert');
      const allow = card.querySelector('[data-option-id="allow-once"]') as HTMLElement;
      await fireEvent.click(allow);
      await fireEvent.click(allow);
      await fireEvent.click(card.querySelector('[data-option-id="reject-once"]') as HTMLElement);
      await waitFor(() => expect(session.answers.length).toBe(1));
    });

    it('with only persistent choices offered, shows the cancel alone and says why', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = {
        ...FOUR_OPTION_REQUEST,
        options: [
          { optionId: 'a', name: 'Always allow', kind: 'allow_always' },
          { optionId: 'r', name: 'Always reject', kind: 'reject_always' }
        ]
      };
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const card = await findByRole('alert');
      const ids = [...card.querySelectorAll('button')].map((b) => b.getAttribute('data-option-id'));
      expect(ids).toEqual(['cancel']);
      expect(card.textContent).toMatch(/no one-time choice/i);
    });

    it('is keyboard operable and each button names its choice in text', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const card = await findByRole('alert');
      expect(card.getAttribute('aria-labelledby')).toBeTruthy();
      const allow = card.querySelector('[data-option-id="allow-once"]') as HTMLElement;
      expect(allow.tagName).toBe('BUTTON');
      expect(allow.tabIndex).toBeGreaterThanOrEqual(0);
      expect(allow.textContent).toContain('Allow once');
      const reject = card.querySelector('[data-option-id="reject-once"]') as HTMLElement;
      expect(reject.textContent).toContain('Reject once');
    });

    it('goes away when the request is answered elsewhere', async () => {
      const session = new FixtureAcpSession();
      session.pendingPermission = FOUR_OPTION_REQUEST;
      const { findByRole, queryByRole } = render(AiChatAcpNodeViewer, {
        props: { nodeId: CHAT_ID, session }
      });
      await findByRole('alert');
      session.pendingPermission = null;
      await waitFor(() => expect(queryByRole('alert')).toBeNull());
    });
  });

  describe('turn, input and session bar', () => {
    it('sends a prompt as a user message, then asks the session to run it', async () => {
      const session = new FixtureAcpSession();
      const { findByLabelText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
      await fireEvent.input(input, { target: { value: 'run the tests' } });
      await fireEvent.keyDown(input, { key: 'Enter' });
      await waitFor(() => expect(session.prompts).toEqual(['run the tests']));
      const sent = structureTree.getChildren(CHAT_ID).map((id) => sharedNodeStore.getNode(id));
      expect(sent.some((n) => n?.content === 'run the tests')).toBe(true);
    });

    it('takes back the user message and shows an alert when the prompt fails', async () => {
      const session = new FixtureAcpSession();
      session.sendPrompt = async () => {
        throw new Error('agent not running');
      };
      const { findByLabelText, findByRole } = render(AiChatAcpNodeViewer, {
        props: { nodeId: CHAT_ID, session }
      });
      const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
      await fireEvent.input(input, { target: { value: 'hello' } });
      await fireEvent.keyDown(input, { key: 'Enter' });
      const alert = await findByRole('alert');
      expect(alert.textContent).toContain('agent not running');
      const left = structureTree.getChildren(CHAT_ID).map((id) => sharedNodeStore.getNode(id));
      expect(left.some((n) => n?.content === 'hello')).toBe(false);
    });

    it('announces a running turn and stops it on request', async () => {
      const session = new FixtureAcpSession();
      session.turnRunning = true;
      const { findByText, findByRole } = render(AiChatAcpNodeViewer, {
        props: { nodeId: CHAT_ID, session }
      });
      await findByText('Agent is working');
      await fireEvent.click(await findByRole('button', { name: 'Stop' }));
      await waitFor(() => expect(session.cancelled).toBe(1));
    });

    it('disables the input while a turn runs', async () => {
      const session = new FixtureAcpSession();
      session.turnRunning = true;
      const { findByLabelText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      expect(((await findByLabelText('Chat message input')) as HTMLTextAreaElement).disabled).toBe(true);
    });

    it('ends an active session and resumes an ended one', async () => {
      const session = new FixtureAcpSession();
      const first = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      await fireEvent.click(await first.findByRole('button', { name: 'End session' }));
      await waitFor(() => expect(session.ended).toBe(1));
      first.unmount();

      seedChat('ended');
      const second = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      await fireEvent.click(await second.findByRole('button', { name: 'Resume session' }));
      await waitFor(() => expect(session.resumed).toBe(1));
    });

    it('offers only the control that fits the session state', async () => {
      const session = new FixtureAcpSession();
      const active = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      await active.findByRole('button', { name: 'End session' });
      expect(active.queryByRole('button', { name: 'Resume session' })).toBeNull();
      expect(active.queryByRole('button', { name: 'Stop' })).toBeNull();
      active.unmount();

      seedChat('ended');
      const ended = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      await ended.findByRole('button', { name: 'Resume session' });
      expect(ended.queryByRole('button', { name: 'End session' })).toBeNull();
    });

    it('shows context usage as text', async () => {
      const session = new FixtureAcpSession();
      session.usage = { used: 12_000, size: 200_000 };
      const { findByText } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      expect(await findByText(/Context 12,000 of 200,000 tokens/)).toBeTruthy();
    });

    it('shows a session error as an alert', async () => {
      const session = new FixtureAcpSession();
      session.error = 'Sign in to Claude Code in a terminal first';
      const { findByRole } = render(AiChatAcpNodeViewer, { props: { nodeId: CHAT_ID, session } });
      expect((await findByRole('alert')).textContent).toContain('Sign in to Claude Code');
    });
  });
});
