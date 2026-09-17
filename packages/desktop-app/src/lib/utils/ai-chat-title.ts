/**
 * Ai-chat title display and rename-commit logic.
 *
 * An ai-chat node's title lives in its `content` field (the schema declares no
 * `title`/`name` field of its own). Two surfaces show that same value — the
 * viewer header (`ai-chat-node-viewer.svelte`) and the sidebar's chat list
 * (`navigation-sidebar.svelte`) — and both must agree on what "no title yet"
 * reads as, so the fallback lives here once rather than as two copies that
 * could drift.
 *
 * "No title yet" is the literal string `UNTITLED_CHAT_TITLE`, persisted at
 * creation. It is a real stored value, not a display-only placeholder: the
 * daemon's background titler uses it as the sentinel that says "this title is
 * still up for grabs" and refuses to write over anything else, so a user's own
 * title is never clobbered. See `is_untitled` in
 * `packages/daemon/src/services/ai_chat_title.rs`, which must agree with this
 * constant exactly.
 */

/**
 * The title a new ai-chat node is created with, and the only value the
 * background titler will overwrite.
 *
 * Kept byte-identical to `UNTITLED_CHAT_TITLE` in
 * `packages/daemon/src/services/ai_chat_title.rs` — the frontend writes it and
 * the daemon tests it, so a drift between the two would silently disable
 * automatic titling (every chat would look user-titled).
 */
export const UNTITLED_CHAT_TITLE = 'Untitled';

/**
 * Display fallback for an ai-chat node whose `content` is empty or
 * whitespace-only.
 *
 * Distinct from {@link UNTITLED_CHAT_TITLE}: that is what gets *stored*, this
 * is only what gets *shown* when nothing is stored at all. Both exist because
 * `content` can still be empty — a chat created before titling landed, or one
 * the user renamed to blank — and "Untitled chat" reads better in a sidebar
 * than an empty row.
 */
export const UNTITLED_CHAT_LABEL = 'Untitled chat';

/** Display title for an ai-chat node's stored `content`. */
export function aiChatDisplayTitle(content: string | null | undefined): string {
  return content?.trim() ? content : UNTITLED_CHAT_LABEL;
}

/**
 * Whether `content` is a title the background titler may replace.
 *
 * True for the stored `"Untitled"` sentinel and for genuinely absent content;
 * false for anything the user typed. Mirrors the daemon-side guard so the two
 * ends agree on which chats are still eligible.
 */
export function isUntitledChat(content: string | null | undefined): boolean {
  const trimmed = content?.trim() ?? '';
  return trimmed === '' || trimmed === UNTITLED_CHAT_TITLE;
}

/**
 * What to persist when a title edit commits, or `null` when nothing should be
 * written.
 *
 * `null` covers both "the user didn't change anything" and "the trimmed
 * result is identical to what's already stored" — writing an unchanged value
 * back would be a no-op mutation that still bumps `modifiedAt` and re-runs
 * the backend's mention extraction for nothing.
 */
export function resolveChatTitleCommit(currentContent: string, draft: string): string | null {
  const trimmed = draft.trim();
  return trimmed === currentContent ? null : trimmed;
}
