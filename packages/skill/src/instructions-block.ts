// The instructions block (ADR-093 §6): for a harness with no plugin, one
// marked block in that harness's user-level instructions file. The installer
// owns the bytes from the begin marker to the end marker and nothing else in
// the file.

import { CONSENT_RULES, LIST_SKILLS_FIRST, ORIENTATION } from './shipped-text.js';

/**
 * The markers are HTML comments: a Markdown renderer shows neither, and the
 * wording is not something a user's own instructions would hold.
 */
export const BLOCK_BEGIN =
  '<!-- nodespace:instructions:begin (written by the NodeSpace skill installer; edits here are overwritten) -->';
export const BLOCK_END = '<!-- nodespace:instructions:end -->';

/**
 * A begin marker as it is found: by its name alone, whatever note follows it
 * inside the comment, so rewording the note never strands a block an earlier
 * version wrote.
 */
const BEGIN_PATTERN = /<!-- nodespace:instructions:begin\b[^>]*-->/g;

/** The block as the installer writes it, markers included, with no newline after it. */
export function renderBlock(): string {
  return [BLOCK_BEGIN, '', ORIENTATION, '', CONSENT_RULES, '', LIST_SKILLS_FIRST, '', BLOCK_END].join('\n');
}

/**
 * Where the block sits in `content`, as `[start, end)` offsets from the begin
 * marker to just past the end marker, or `undefined` when there is none.
 *
 * A begin marker with no end marker after it is not a block: the match never
 * runs across a second begin marker, so a marker left dangling by a hand edit
 * cannot make the installer claim the user's text below it.
 */
function findBlock(content: string): [number, number] | undefined {
  const begins = [...content.matchAll(BEGIN_PATTERN)];
  for (const [i, begin] of begins.entries()) {
    const end = content.indexOf(BLOCK_END, begin.index + begin[0].length);
    if (end === -1) return undefined;
    const next = begins[i + 1]?.index;
    if (next === undefined || next > end) return [begin.index, end + BLOCK_END.length];
  }
  return undefined;
}

/** Whether `content` holds the installer's block. */
export function hasBlock(content: string): boolean {
  return findBlock(content) !== undefined;
}

/**
 * `content` with the block in it: replaced where it already is, and appended
 * otherwise.
 *
 * Appending adds the block and one newline after it, and nothing before it.
 * When the file's last line has no newline the begin marker follows it on
 * that line, so removing the block gives back exactly the bytes that were
 * there.
 */
export function upsertBlock(content: string, block = renderBlock()): string {
  const found = findBlock(content);
  if (found) return content.slice(0, found[0]) + block + content.slice(found[1]);
  return `${content}${block}\n`;
}

/**
 * `content` without the block, and without the one newline the installer wrote
 * after it. Every other byte is returned as it was. `content` itself when it
 * holds no block.
 */
export function removeBlock(content: string): string {
  const found = findBlock(content);
  if (!found) return content;
  const after = content.startsWith('\n', found[1]) ? found[1] + 1 : found[1];
  return content.slice(0, found[0]) + content.slice(after);
}
