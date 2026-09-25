/**
 * Terminal text written in place of PTY output the daemon dropped because the
 * stream fell behind a burst. Starts on a fresh line so it does not splice
 * into a partially rendered line, and is coloured like the session-closed
 * notice so it reads as a notice rather than program output. Keep the wording
 * in step with the CLI's `dropped_notice` (`packages/cli/src/commands/session.rs`).
 */
export function formatDroppedNotice(droppedChunks: number): string {
  const unit = droppedChunks === 1 ? 'chunk' : 'chunks';
  return `\r\n\x1b[33m[output truncated: ${droppedChunks} ${unit} dropped]\x1b[0m\r\n`;
}
