/**
 * Arrow-key navigation across soft-wrapped lines (Browser Mode)
 *
 * A textarea's value only records hard line breaks, so first/last-line
 * detection that looked for `\n` treated every row of a wrapped paragraph
 * as both the first and the last line: ArrowUp/ArrowDown from any row left
 * the node, and entering a node by arrow key landed wherever the caret's
 * offset fell along the whole unwrapped line. These run in a real browser
 * because wrapping needs real layout.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  TextareaController,
  type TextareaControllerEvents
} from '$lib/design/components/textarea-controller';
import { TextareaCaretMirror, isSameRow } from '$lib/design/components/textarea-caret-geometry';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';

const WORDS =
  'Arrow keys should move between the visual rows of a wrapped paragraph before they ' +
  'ever leave the node, and entering a node should land on its edge row near the same column.';

/** Four-plus rows at this width; the second hard line wraps too. */
const TEXT = `${WORDS}\n${WORDS}`;

type NavigateCall = { nodeId: string; direction: 'up' | 'down'; pixelOffset: number };

function createEvents(navigateCalls: NavigateCall[]): TextareaControllerEvents {
  const noop = () => {};
  return {
    contentChanged: noop,
    focus: noop,
    blur: noop,
    createNewNode: noop,
    indentNode: noop,
    outdentNode: noop,
    navigateArrow: (data: NavigateCall) => navigateCalls.push(data),
    combineWithPrevious: noop,
    deleteNode: noop,
    triggerDetected: noop,
    triggerHidden: noop,
    nodeReferenceSelected: noop,
    slashCommandDetected: noop,
    slashCommandHidden: noop,
    slashCommandSelected: noop,
    nodeTypeConversionDetected: noop,
    directSlashCommand: noop
  } as unknown as TextareaControllerEvents;
}

describe('Arrow navigation across soft-wrapped lines (Browser Mode)', () => {
  let element: HTMLTextAreaElement;
  let controller: TextareaController;
  let navigateCalls: NavigateCall[];

  /** Visual row index of each position, measured independently. */
  function rowTops(): number[] {
    const mirror = TextareaCaretMirror.create(element);
    if (!mirror) throw new Error('textarea has no layout');
    const tops: number[] = [];
    for (let p = 0; p <= element.value.length; p++) {
      tops.push(mirror.pointAt(p)?.top ?? NaN);
    }
    mirror.dispose();
    return tops;
  }

  function setCaret(position: number): void {
    element.focus();
    element.setSelectionRange(position, position);
  }

  function pressKey(key: 'ArrowUp' | 'ArrowDown'): Promise<void> {
    element.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
    // The controller's keydown handler is async; let it settle.
    return new Promise((resolve) => setTimeout(resolve, 0));
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    element = document.createElement('textarea');
    Object.assign(element.style, {
      width: '320px',
      padding: '4px 8px',
      border: '1px solid',
      font: '16px/24px sans-serif',
      resize: 'none'
    });
    element.rows = 12;
    document.body.appendChild(element);
    navigateCalls = [];
    controller = new TextareaController(
      element,
      'wrapped-node',
      'text',
      DEFAULT_PANE_ID,
      createEvents(navigateCalls),
      () => ({ allowMultiline: true })
    );
    controller.initialize(TEXT, false);
  });

  afterEach(() => {
    controller.destroy();
    document.body.innerHTML = '';
  });

  it('the fixture really wraps: each hard line spans several rows', () => {
    const tops = rowTops();
    const firstLineEnd = TEXT.indexOf('\n');
    expect(isSameRow({ top: tops[0], left: 0 }, { top: tops[firstLineEnd], left: 0 })).toBe(false);
    expect(new Set(tops).size).toBeGreaterThanOrEqual(4);
  });

  it('isAtFirstLine is true only on the first visual row', () => {
    const tops = rowTops();
    for (let p = 0; p <= TEXT.length; p += 7) {
      setCaret(p);
      expect(controller.isAtFirstLine(), `position ${p}`).toBe(tops[p] === tops[0]);
    }
  });

  it('isAtLastLine is true only on the last visual row', () => {
    const tops = rowTops();
    const lastTop = tops[TEXT.length];
    for (let p = 0; p <= TEXT.length; p += 7) {
      setCaret(p);
      expect(controller.isAtLastLine(), `position ${p}`).toBe(tops[p] === lastTop);
    }
  });

  it('ArrowUp leaves the node only from the first visual row', async () => {
    const tops = rowTops();
    const secondRowStart = tops.findIndex((top) => top !== tops[0]);

    setCaret(secondRowStart + 3);
    await pressKey('ArrowUp');
    expect(navigateCalls).toHaveLength(0);

    setCaret(3);
    await pressKey('ArrowUp');
    expect(navigateCalls).toHaveLength(1);
    expect(navigateCalls[0].direction).toBe('up');
  });

  it('ArrowDown leaves the node only from the last visual row', async () => {
    const tops = rowTops();
    const lastTop = tops[TEXT.length];
    const lastRowStart = tops.findIndex((top) => top === lastTop);

    // Last hard line, but a row above its last wrapped row.
    setCaret(lastRowStart - 3);
    await pressKey('ArrowDown');
    expect(navigateCalls).toHaveLength(0);

    setCaret(TEXT.length - 2);
    await pressKey('ArrowDown');
    expect(navigateCalls).toHaveLength(1);
    expect(navigateCalls[0].direction).toBe('down');
  });

  it("reports the caret's offset within its wrapped row", () => {
    const tops = rowTops();
    const secondRowStart = tops.findIndex((top) => top !== tops[0]);
    const left = element.getBoundingClientRect().left + window.scrollX;

    setCaret(secondRowStart);
    // At the start of a wrapped row the caret is at the row's left edge, not
    // a full row's width to the right.
    expect(controller.getCurrentPixelOffset() - left).toBeLessThan(20);
  });

  it('entering from below (ArrowUp) lands on the last visual row, near the column', () => {
    const tops = rowTops();
    const lastTop = tops[TEXT.length];
    const left = element.getBoundingClientRect().left + window.scrollX;

    controller.enterFromArrowNavigation('up', left + 60);

    const position = element.selectionStart;
    expect(tops[position]).toBe(lastTop);
    const mirror = TextareaCaretMirror.create(element);
    const x = mirror?.pointAt(position)?.left ?? -1;
    mirror?.dispose();
    expect(Math.abs(x - 60)).toBeLessThan(12);
  });

  it('entering from above (ArrowDown) lands on the first visual row, near the column', () => {
    const tops = rowTops();
    const left = element.getBoundingClientRect().left + window.scrollX;

    controller.enterFromArrowNavigation('down', left + 200);

    const position = element.selectionStart;
    expect(tops[position]).toBe(tops[0]);
    const mirror = TextareaCaretMirror.create(element);
    const x = mirror?.pointAt(position)?.left ?? -1;
    mirror?.dispose();
    expect(Math.abs(x - 200)).toBeLessThan(12);
  });

  it('a single-line node (task-like) also moves between its wrapped rows', async () => {
    controller.destroy();
    controller = new TextareaController(
      element,
      'wrapped-task',
      'task',
      DEFAULT_PANE_ID,
      createEvents(navigateCalls),
      () => ({ allowMultiline: false })
    );
    controller.initialize(WORDS, false);
    const tops = rowTops();

    setCaret(WORDS.length);
    await pressKey('ArrowUp');
    expect(navigateCalls).toHaveLength(0);

    setCaret(0);
    await pressKey('ArrowDown');
    expect(navigateCalls).toHaveLength(0);
    expect(tops[0]).not.toBe(tops[WORDS.length]);
  });
});
