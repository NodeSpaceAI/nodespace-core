/**
 * Tests for Mermaid split-panel edit mode logic (issue #925)
 *
 * Tests the debounce behavior and language-gating that controls the
 * live preview in code-block-node.svelte. The component logic is tested
 * directly (not via component mount) since Svelte $effect runes require
 * a browser environment. Tests focus on the observable contract.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Mock the mermaid-render service
vi.mock('$lib/services/mermaid-render', () => ({
  renderMermaid: vi.fn()
}));

import { renderMermaid } from '$lib/services/mermaid-render';

const mockRenderMermaid = vi.mocked(renderMermaid);

// Helper: simulate the debounced preview logic extracted from the component
function createPreviewController() {
  let previewSvg: string | null = null;
  let previewError: string | null = null;
  let previewRenderSeq = 0;
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  function triggerPreview(
    isEditing: boolean,
    language: string,
    code: string,
    nodeId: string,
    isDark: boolean
  ): Promise<void> {
    if (!isEditing || language !== 'mermaid') {
      previewSvg = null;
      previewError = null;
      return Promise.resolve();
    }

    if (debounceTimer) clearTimeout(debounceTimer);

    return new Promise((resolve) => {
      debounceTimer = setTimeout(async () => {
        const seq = ++previewRenderSeq;
        const svg = await renderMermaid(code, `${nodeId}-preview`, isDark);
        if (seq !== previewRenderSeq) return resolve(); // stale
        if (svg !== null) {
          previewSvg = svg;
          previewError = null;
        } else {
          previewSvg = null;
          previewError = 'Syntax error — check your Mermaid definition.';
        }
        resolve();
      }, 300);
    });
  }

  return {
    get previewSvg() {
      return previewSvg;
    },
    get previewError() {
      return previewError;
    },
    triggerPreview
  };
}

describe('Mermaid split-panel debounce logic', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockRenderMermaid.mockResolvedValue('<svg><rect/></svg>');
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('does not call renderMermaid immediately on trigger', async () => {
    const ctrl = createPreviewController();
    ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', false);

    // No call before debounce fires
    expect(mockRenderMermaid).not.toHaveBeenCalled();
  });

  it('calls renderMermaid after 300ms debounce', async () => {
    const ctrl = createPreviewController();
    const promise = ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', false);

    vi.advanceTimersByTime(300);
    await promise;

    expect(mockRenderMermaid).toHaveBeenCalledOnce();
    expect(mockRenderMermaid).toHaveBeenCalledWith('graph LR\nA-->B', 'node-1-preview', false);
  });

  it('sets previewSvg on successful render', async () => {
    mockRenderMermaid.mockResolvedValue('<svg><text>diagram</text></svg>');
    const ctrl = createPreviewController();
    const promise = ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', false);

    vi.advanceTimersByTime(300);
    await promise;

    expect(ctrl.previewSvg).toBe('<svg><text>diagram</text></svg>');
    expect(ctrl.previewError).toBeNull();
  });

  it('sets previewError when renderMermaid returns null', async () => {
    mockRenderMermaid.mockResolvedValue(null);
    const ctrl = createPreviewController();
    const promise = ctrl.triggerPreview(true, 'mermaid', 'invalid syntax !!', 'node-1', false);

    vi.advanceTimersByTime(300);
    await promise;

    expect(ctrl.previewSvg).toBeNull();
    expect(ctrl.previewError).toBe('Syntax error — check your Mermaid definition.');
  });

  it('debounces rapid updates — only fires once for last value', async () => {
    const ctrl = createPreviewController();

    // Fire three rapid updates
    ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', false);
    vi.advanceTimersByTime(100);
    ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B-->C', 'node-1', false);
    vi.advanceTimersByTime(100);
    const lastPromise = ctrl.triggerPreview(
      true,
      'mermaid',
      'graph LR\nA-->B-->C-->D',
      'node-1',
      false
    );
    vi.advanceTimersByTime(300);
    await lastPromise;

    // Only one render — for the last content
    expect(mockRenderMermaid).toHaveBeenCalledOnce();
    expect(mockRenderMermaid).toHaveBeenCalledWith(
      'graph LR\nA-->B-->C-->D',
      'node-1-preview',
      false
    );
  });

  it('clears preview state when not editing', () => {
    const ctrl = createPreviewController();
    // Seed some state
    ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', false);

    // Now call with isEditing=false (blur)
    ctrl.triggerPreview(false, 'mermaid', 'graph LR\nA-->B', 'node-1', false);

    expect(ctrl.previewSvg).toBeNull();
    expect(ctrl.previewError).toBeNull();
    // renderMermaid should not have been called (timers haven't advanced)
    expect(mockRenderMermaid).not.toHaveBeenCalled();
  });

  it('clears preview state when language is not mermaid', () => {
    const ctrl = createPreviewController();

    ctrl.triggerPreview(true, 'typescript', 'const x = 1;', 'node-1', false);

    expect(ctrl.previewSvg).toBeNull();
    expect(ctrl.previewError).toBeNull();
    expect(mockRenderMermaid).not.toHaveBeenCalled();
  });

  it('passes isDark flag through to renderMermaid', async () => {
    const ctrl = createPreviewController();
    const promise = ctrl.triggerPreview(true, 'mermaid', 'graph LR\nA-->B', 'node-1', true);

    vi.advanceTimersByTime(300);
    await promise;

    expect(mockRenderMermaid).toHaveBeenCalledWith('graph LR\nA-->B', 'node-1-preview', true);
  });
});

describe('Mermaid split-panel language gating', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  const nonMermaidLanguages = ['typescript', 'javascript', 'python', 'rust', 'bash', 'plaintext'];

  nonMermaidLanguages.forEach((lang) => {
    it(`does not trigger preview for language: ${lang}`, () => {
      const ctrl = createPreviewController();
      ctrl.triggerPreview(true, lang, 'some code', 'node-1', false);

      vi.advanceTimersByTime(500); // past any debounce

      expect(mockRenderMermaid).not.toHaveBeenCalled();
      expect(ctrl.previewSvg).toBeNull();
      expect(ctrl.previewError).toBeNull();
    });
  });
});
