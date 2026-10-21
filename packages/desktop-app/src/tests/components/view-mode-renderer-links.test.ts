/**
 * Mounts the real ViewModeRenderer: an unsafe link must never become an anchor.
 */
import { describe, it, expect } from 'vitest';
import { mount, unmount } from 'svelte';
import ViewModeRenderer from '$lib/design/components/view-mode-renderer.svelte';

function render(content: string): HTMLElement {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(ViewModeRenderer, { target, props: { content } });
  const cleanup = () => {
    unmount(comp);
    target.remove();
  };
  (target as HTMLElement & { cleanup?: () => void }).cleanup = cleanup;
  return target;
}

describe('ViewModeRenderer link hrefs', () => {
  it('renders unsafe links as label text with no anchor', () => {
    for (const u of [
      'javascript:alert(1)',
      'JaVaScRiPt:alert(1)',
      'data:text/html,x',
      'vbscript:x',
      'javascript&#58;alert(1)',
      'javascript&colon;alert(1)',
      '//evil.test'
    ]) {
      const el = render(`[click](<${u}>)`);
      expect(el.querySelector('a'), u).toBeNull();
      expect(el.textContent, u).toContain('click');
      expect(el.innerHTML, u).not.toMatch(/javascript/i);
    }
  });

  it('renders a safe link as an anchor', () => {
    const el = render('[ok](https://a.test)');
    expect(el.querySelector('a')?.getAttribute('href')).toBe('https://a.test');
  });
});
