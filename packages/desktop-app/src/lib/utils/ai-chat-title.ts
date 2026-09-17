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
 *
 * Note the split of responsibility: this end only *writes* the sentinel.
 * Deciding whether a chat is still eligible for titling is the daemon's alone
 * (`is_untitled`), so there is deliberately no frontend predicate mirroring it
 * — a second implementation of the guard could drift from the one that
 * actually gates the write, which is worse than not having it.
 */
export const UNTITLED_CHAT_TITLE = 'Untitled';

/**
 * Display fallback for an ai-chat node whose `content` is empty or
 * whitespace-only.
 *
 * Distinct from {@link UNTITLED_CHAT_TITLE}: that is what gets *stored*, this
 * is only what gets *shown*. A persisted chat should never actually be blank —
 * `AiChatNodeBehavior::validate` rejects blank content, and
 * {@link resolveChatTitleCommit} never writes it — so this is defensive
 * rendering for a node that arrived malformed (a hand-edited database, a
 * future writer that skips validation), not a state the app produces. It
 * reads better in a sidebar than an empty row.
 */
export const UNTITLED_CHAT_LABEL = 'Untitled chat';

/** Display title for an ai-chat node's stored `content`. */
export function aiChatDisplayTitle(content: string | null | undefined): string {
  return content?.trim() ? content : UNTITLED_CHAT_LABEL;
}

/**
 * What to persist when a title edit commits, or `null` when nothing should be
 * written.
 *
 * `null` covers both "the user didn't change anything" and "the trimmed
 * result is identical to what's already stored" — writing an unchanged value
 * back would be a no-op mutation that still bumps `modifiedAt` and re-runs
 * the backend's mention extraction for nothing.
 *
 * A cleared draft resolves to {@link UNTITLED_CHAT_TITLE}, not `''`. Two
 * reasons, and they agree:
 *
 * - An ai-chat node must carry a title (`AiChatNodeBehavior::validate`
 *   rejects blank content), so `''` is not a value the backend will accept.
 *   Sending it fails the write, and because the commit also updates the
 *   sidebar optimistically with no rollback, both surfaces would go on
 *   showing a title the database never took.
 * - Clearing a title means "I have no title for this", which is precisely
 *   what the sentinel encodes — so this also re-arms automatic titling,
 *   rather than leaving the chat stuck with no title and no way to get one.
 *
 * Note this is asymmetric with {@link aiChatDisplayTitle}, which still
 * tolerates blank content on the read side. That is deliberate: the write
 * side is a contract with the backend, the read side is defensive rendering.
 */
export function resolveChatTitleCommit(currentContent: string, draft: string): string | null {
  const trimmed = draft.trim() || UNTITLED_CHAT_TITLE;
  return trimmed === currentContent ? null : trimmed;
}
