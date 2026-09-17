/**
 * Unit tests for ai-chat-title — the shared display-title and rename-commit
 * logic for ai-chat nodes, used by both the viewer header and the sidebar's
 * chat list so they can't drift on what "no title yet" reads as.
 */

import { describe, it, expect } from 'vitest';
import {
  aiChatDisplayTitle,
  resolveChatTitleCommit,
  UNTITLED_CHAT_LABEL,
  UNTITLED_CHAT_TITLE
} from '$lib/utils/ai-chat-title';

describe('aiChatDisplayTitle', () => {
  it('returns the content when it has non-whitespace characters', () => {
    expect(aiChatDisplayTitle('My chat about Rust')).toBe('My chat about Rust');
  });

  it('falls back to the placeholder for empty content', () => {
    expect(aiChatDisplayTitle('')).toBe(UNTITLED_CHAT_LABEL);
  });

  it('falls back to the placeholder for whitespace-only content', () => {
    expect(aiChatDisplayTitle('   ')).toBe(UNTITLED_CHAT_LABEL);
  });

  it('falls back to the placeholder for null/undefined content', () => {
    expect(aiChatDisplayTitle(null)).toBe(UNTITLED_CHAT_LABEL);
    expect(aiChatDisplayTitle(undefined)).toBe(UNTITLED_CHAT_LABEL);
  });

  it('preserves leading/trailing whitespace in a non-empty title rather than trimming it', () => {
    // Trimming is a decision for the caller writing the value, not for display —
    // this only decides whether to show the placeholder.
    expect(aiChatDisplayTitle('  Padded  ')).toBe('  Padded  ');
  });
});

describe('UNTITLED_CHAT_TITLE', () => {
  it('is the exact sentinel the daemon tests for', () => {
    // This string crosses a language boundary: the frontend writes it at
    // creation (schema-authoring) and the daemon's `is_untitled` tests for it
    // (packages/daemon/src/services/ai_chat_title.rs). A drift between the two
    // disables background titling silently — every chat would look
    // user-titled — so both sides pin the literal.
    expect(UNTITLED_CHAT_TITLE).toBe('Untitled');
  });

  it('is distinct from the display-only placeholder', () => {
    // One is stored, the other is only rendered. Collapsing them would make a
    // chat displaying "Untitled chat" look claimable to the titler.
    expect(UNTITLED_CHAT_TITLE).not.toBe(UNTITLED_CHAT_LABEL);
  });
});

describe('resolveChatTitleCommit', () => {
  it('returns the trimmed draft when it differs from the current content', () => {
    expect(resolveChatTitleCommit('Old title', 'New title')).toBe('New title');
  });

  it('trims surrounding whitespace from the draft before comparing/returning', () => {
    expect(resolveChatTitleCommit('', '  New title  ')).toBe('New title');
  });

  it('returns null when the trimmed draft equals the current content — a no-op write', () => {
    expect(resolveChatTitleCommit('Same', 'Same')).toBeNull();
  });

  it('returns null when the draft only adds whitespace around the current content', () => {
    expect(resolveChatTitleCommit('Same', '  Same  ')).toBeNull();
  });

  it('returns an empty string (not null) when clearing a previously-set title', () => {
    // Clearing is a real, intentional change — distinct from the no-op case,
    // and must be persisted so the display falls back to the placeholder.
    expect(resolveChatTitleCommit('Old title', '')).toBe('');
    expect(resolveChatTitleCommit('Old title', '   ')).toBe('');
  });

  it('returns null when both the current content and the draft are already empty', () => {
    expect(resolveChatTitleCommit('', '')).toBeNull();
    expect(resolveChatTitleCommit('', '   ')).toBeNull();
  });
});
