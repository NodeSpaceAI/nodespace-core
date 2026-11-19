import { describe, it, expect } from 'vitest';
import {
  acpRowKind,
  onceOnlyOptions,
  toolKindLabel,
  toolStatusLabel,
  isToolActive,
  permissionOptionKindLabel
} from '$lib/utils/acp-chat';
import type { AcpPermissionOption } from '$lib/types/acp-session';
import type { AiChatMessageNode } from '$lib/types/ai-chat-node';

function message(fields: Partial<AiChatMessageNode>): AiChatMessageNode {
  return { id: 'm', nodeType: 'ai-chat-message', content: '', role: 'assistant', ...fields } as AiChatMessageNode;
}

describe('acpRowKind', () => {
  it('maps a user message to a user row and a system message to a notice', () => {
    expect(acpRowKind(message({ role: 'user' }))).toBe('user');
    expect(acpRowKind(message({ role: 'system', partKind: 'plan' }))).toBe('notice');
  });

  it('maps an assistant message by its part kind', () => {
    expect(acpRowKind(message({}))).toBe('text');
    expect(acpRowKind(message({ partKind: 'text' }))).toBe('text');
    expect(acpRowKind(message({ partKind: 'thought' }))).toBe('thought');
    expect(acpRowKind(message({ partKind: 'plan' }))).toBe('plan');
    expect(acpRowKind(message({ partKind: 'tool_call' }))).toBe('tool');
  });
});

describe('onceOnlyOptions', () => {
  const options: AcpPermissionOption[] = [
    { optionId: 'a', name: 'Always allow', kind: 'allow_always' },
    { optionId: 'b', name: 'Allow', kind: 'allow_once' },
    { optionId: 'c', name: 'Always reject', kind: 'reject_always' },
    { optionId: 'd', name: 'Reject', kind: 'reject_once' }
  ];

  it('keeps only the once-only choices, in the agent order', () => {
    expect(onceOnlyOptions(options).map((o) => o.optionId)).toEqual(['b', 'd']);
  });

  it('drops a kind it does not know rather than showing it', () => {
    const odd = [{ optionId: 'x', name: 'Bypass', kind: 'allow_forever' }] as unknown as AcpPermissionOption[];
    expect(onceOnlyOptions(odd)).toEqual([]);
  });

  it('leaves nothing when the agent offered only persistent choices', () => {
    expect(onceOnlyOptions([options[0], options[2]])).toEqual([]);
  });
});

describe('labels', () => {
  it('names every tool status in words', () => {
    expect(toolStatusLabel('pending')).toBe('Waiting');
    expect(toolStatusLabel('in_progress')).toBe('Running');
    expect(toolStatusLabel('completed')).toBe('Done');
    expect(toolStatusLabel('failed')).toBe('Failed');
    expect(toolStatusLabel('cancelled')).toBe('Cancelled');
    expect(toolStatusLabel(undefined)).toBe('Waiting');
  });

  it('names tool kinds, with a fallback for an unknown one', () => {
    expect(toolKindLabel('read')).toBe('Read');
    expect(toolKindLabel('switch_mode')).toBe('Switch mode');
    expect(toolKindLabel(undefined)).toBe('Tool');
  });

  it('treats only a pending or running call as active', () => {
    expect(isToolActive('pending')).toBe(true);
    expect(isToolActive('in_progress')).toBe(true);
    expect(isToolActive('completed')).toBe(false);
    expect(isToolActive('failed')).toBe(false);
    expect(isToolActive('cancelled')).toBe(false);
  });

  it('labels the permission option kinds', () => {
    expect(permissionOptionKindLabel('allow_once')).toBe('Allow once');
    expect(permissionOptionKindLabel('reject_once')).toBe('Reject once');
  });
});
