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

/**
 * Zero-width characters that read as blank but survive `String.prototype.trim`.
 *
 * `trim` strips whitespace only, so a title pasted as U+200B is truthy in JS
 * while the backend's `is_empty_or_whitespace` (`packages/core/src/behaviors/mod.rs`)
 * counts it as blank. Kept in step with that function's list, so the two ends
 * agree on what "no title" means — a disagreement here is exactly what lets a
 * write be sent that the backend then refuses.
 */
const ZERO_WIDTH_CHARS = /[\u200B\u200C\u200D\uFEFF]/g;

/**
 * Whether `content` is blank in the sense the backend uses: empty, whitespace,
 * or made only of zero-width characters.
 */
function isBlankTitle(content: string): boolean {
  return content.replace(ZERO_WIDTH_CHARS, '').trim() === '';
}

/** Display title for an ai-chat node's stored `content`. */
export function aiChatDisplayTitle(content: string | null | undefined): string {
  return content && !isBlankTitle(content) ? content : UNTITLED_CHAT_LABEL;
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
 * "Cleared" is judged with {@link isBlankTitle}, not `trim()` alone: `trim`
 * leaves zero-width characters standing, so a draft of only those would look
 * non-empty here, be sent as-is, and then be refused by the backend — the same
 * failed-write divergence this function exists to prevent, just for a narrower
 * input. The two ends have to agree on what counts as blank.
 */
export function resolveChatTitleCommit(currentContent: string, draft: string): string | null {
  const resolved = isBlankTitle(draft) ? UNTITLED_CHAT_TITLE : draft.trim();
  return resolved === currentContent ? null : resolved;
}
