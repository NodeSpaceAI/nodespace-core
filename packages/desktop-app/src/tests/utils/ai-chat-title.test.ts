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

  it('falls back to the placeholder for zero-width-only content', () => {
    // Renders as an empty row otherwise — `trim()` alone would treat these as
    // a real title.
    expect(aiChatDisplayTitle('​')).toBe(UNTITLED_CHAT_LABEL);
    expect(aiChatDisplayTitle('﻿‍')).toBe(UNTITLED_CHAT_LABEL);
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

  it('resolves a cleared title to the sentinel, never to an empty string', () => {
    // Clearing is a real, intentional change — but `''` is not a value the
    // backend accepts (AiChatNodeBehavior::validate rejects blank content),
    // and the commit updates the sidebar optimistically with no rollback, so
    // writing it would leave two surfaces showing a title the database never
    // took. "I have no title for this" is what the sentinel already means —
    // so this also hands the chat back to background titling, rather than
    // stranding it with no title and no way to acquire one.
    expect(resolveChatTitleCommit('Old title', '')).toBe(UNTITLED_CHAT_TITLE);
    expect(resolveChatTitleCommit('Old title', '   ')).toBe(UNTITLED_CHAT_TITLE);
  });

  it('returns null when clearing a chat that is already at the sentinel', () => {
    // No-op: already untitled, so there is nothing to write.
    expect(resolveChatTitleCommit(UNTITLED_CHAT_TITLE, '')).toBeNull();
    expect(resolveChatTitleCommit(UNTITLED_CHAT_TITLE, '   ')).toBeNull();
  });

  it('treats a zero-width-only draft as cleared', () => {
    // `trim()` does not strip U+200B and friends, so these are truthy in JS
    // while the backend's is_empty_or_whitespace counts them as blank. Without
    // matching that definition the draft would be sent as-is and refused,
    // which is the same failed write this resolution exists to prevent.
    expect(resolveChatTitleCommit('Old title', '​')).toBe(UNTITLED_CHAT_TITLE);
    expect(resolveChatTitleCommit('Old title', '​‌‍﻿')).toBe(
      UNTITLED_CHAT_TITLE
    );
    expect(resolveChatTitleCommit('Old title', ' ​ ')).toBe(UNTITLED_CHAT_TITLE);
  });

  it('keeps a real title that merely contains a zero-width character', () => {
    // Only a title made *entirely* of them is blank; one embedded in real text
    // is the user's content and must survive.
    expect(resolveChatTitleCommit('Old title', 'Bill​ing')).toBe('Bill​ing');
  });

  it('returns the sentinel when the stored content is somehow already blank', () => {
    // Defensive: validation should make a blank stored title impossible, but
    // if one is encountered, committing repairs it rather than writing `''`.
    expect(resolveChatTitleCommit('', '')).toBe(UNTITLED_CHAT_TITLE);
    expect(resolveChatTitleCommit('', '   ')).toBe(UNTITLED_CHAT_TITLE);
  });
});
