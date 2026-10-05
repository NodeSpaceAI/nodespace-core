import { describe, it, expect } from 'vitest';
import {
  BLOCK_BEGIN,
  BLOCK_END,
  hasBlock,
  removeBlock,
  renderBlock,
  upsertBlock,
} from '../instructions-block.js';
import { CONSENT_RULES, LIST_SKILLS_FIRST, ORIENTATION } from '../shipped-text.js';

describe('the instructions block', () => {
  it('carries the orientation, the confirmation rules and the instruction to list skills, between its markers', () => {
    const block = renderBlock();
    expect(block.startsWith(BLOCK_BEGIN)).toBe(true);
    expect(block.endsWith(BLOCK_END)).toBe(true);
    expect(block).toContain(ORIENTATION);
    expect(block).toContain(CONSENT_RULES);
    expect(block).toContain(LIST_SKILLS_FIRST);
    expect(block).toContain('`nodespace skill guidance`');
  });

  it('is the whole file when the file was empty, and removing it leaves nothing', () => {
    const written = upsertBlock('');
    expect(written).toBe(`${renderBlock()}\n`);
    expect(removeBlock(written)).toBe('');
  });

  // Every shape of what was there before: with a final newline, without one,
  // with blank lines at the end, with Windows line endings.
  it.each([
    ['# Mine\n\nBe terse.\n'],
    ['# Mine\n\nBe terse.'],
    ['# Mine\n\n\n'],
    ['# Mine\r\n\r\nBe terse.\r\n'],
    ['\n'],
  ])('appended after %j and removed again gives back the same bytes', before => {
    const written = upsertBlock(before);
    expect(written.startsWith(before)).toBe(true);
    expect(hasBlock(written)).toBe(true);
    expect(removeBlock(written)).toBe(before);
  });

  it('removes only itself when the user has content before and after it', () => {
    const before = '# Mine\n\nBe terse.\n';
    const after = '\n## Added later\n\nNo emoji.\n';
    const file = upsertBlock(before) + after;
    expect(removeBlock(file)).toBe(before + after);
  });

  it('is replaced in place on a reinstall, leaving the content around it as it was', () => {
    const before = '# Mine\n';
    const after = '\n## Added later\n';
    const old = [BLOCK_BEGIN, 'an older block', BLOCK_END].join('\n');
    const file = `${before}${old}\n${after}`;

    const updated = upsertBlock(file);
    expect(updated).toBe(`${before}${renderBlock()}\n${after}`);
    expect(updated).not.toContain('an older block');
    expect(updated.split(BLOCK_BEGIN)).toHaveLength(2);
    // A second run changes nothing.
    expect(upsertBlock(updated)).toBe(updated);
  });

  // The note inside the begin marker may be reworded; the block an earlier
  // version wrote is still the block.
  it('finds a block whose begin marker carries a different note', () => {
    const old = `<!-- nodespace:instructions:begin (an older note) -->\nolder text\n${BLOCK_END}`;
    const file = `# Mine\n${old}\n`;

    expect(hasBlock(file)).toBe(true);
    expect(upsertBlock(file)).toBe(`# Mine\n${renderBlock()}\n`);
    expect(removeBlock(file)).toBe('# Mine\n');
  });

  it('leaves a file with no block as it is on removal', () => {
    const file = '# Mine\n\nBe terse.\n';
    expect(hasBlock(file)).toBe(false);
    expect(removeBlock(file)).toBe(file);
  });

  // A begin marker whose end marker was deleted by hand is the user's text
  // now. Matching from it to a later block's end would take what lies between.
  it('never claims the text between a dangling begin marker and a later block', () => {
    const dangling = `${BLOCK_BEGIN}\nkept by the user\n`;
    const file = upsertBlock(dangling);
    expect(file).toContain('kept by the user');
    expect(removeBlock(file)).toBe(dangling);
    expect(upsertBlock(file)).toBe(file);
  });

  it('does not treat a begin marker with no end marker as a block', () => {
    const file = `# Mine\n${BLOCK_BEGIN}\nhalf a block\n`;
    expect(hasBlock(file)).toBe(false);
    expect(removeBlock(file)).toBe(file);
  });
});
