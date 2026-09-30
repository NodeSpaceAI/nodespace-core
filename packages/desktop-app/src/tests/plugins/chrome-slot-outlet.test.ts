/**
 * ChromeSlotOutlet: renders a chrome slot's active contributions, each in its
 * own outlet keyed by contribution key, in priority order.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import ChromeSlotOutlet from '$lib/plugins/chrome-slot-outlet.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags,
  testExtensionMounts
} from '../fixtures/test-extension';

/** The fixture markers in the container, in document order. */
function markers(container: HTMLElement): string[] {
  return [...container.querySelectorAll('[data-testid]')].map(
    (el) => el.getAttribute('data-testid') ?? ''
  );
}

describe('ChromeSlotOutlet', () => {
  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('renders a contribution only while its flag is on', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-overlay' }
    });
    expect(markers(container)).toEqual([]);

    testExtensionFlags.overlay = true;
    await findByTestId('test-chrome');
    expect(markers(container)).toEqual(['test-chrome']);

    testExtensionFlags.overlay = false;
    await waitFor(() => expect(markers(container)).toEqual([]));
  });

  it('renders only the contributions of its own slot', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.overlay = true;
    testExtensionFlags.modalSecondary = true;

    const overlay = render(ChromeSlotOutlet, { props: { name: 'app-shell-overlay' } });
    await overlay.findByTestId('test-chrome');
    expect(markers(overlay.container)).toEqual(['test-chrome']);

    const modal = render(ChromeSlotOutlet, { props: { name: 'app-shell-modal' } });
    await modal.findByTestId('test-chrome-secondary');
    expect(markers(modal.container)).toEqual(['test-chrome-secondary']);
  });

  it('renders in priority order', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.modal = true;
    testExtensionFlags.modalSecondary = true;

    const { container } = render(ChromeSlotOutlet, { props: { name: 'app-shell-modal' } });

    // `modal-secondary` has the higher priority, so it comes first even though
    // `modal` is declared before it.
    await waitFor(() => expect(markers(container)).toEqual(['test-chrome-secondary', 'test-chrome']));
  });

  it('renders nothing for an empty slot', async () => {
    uiExtensionRegistry.register(createTestExtension());
    // Warm the fixture chunk, so a wrongly started load would land within the wait below.
    await import('../fixtures/test-extension/test-chrome.svelte');
    const { container } = render(ChromeSlotOutlet, { props: { name: 'app-shell-modal' } });

    // Give any (wrongly) started load a chance to land.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(markers(container)).toEqual([]);
    expect(container.textContent).toBe('');
  });

  it('turning one contribution off unmounts only its outlet', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.modal = true;
    testExtensionFlags.modalSecondary = true;

    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-modal' }
    });
    const survivor = await findByTestId('test-chrome');
    await findByTestId('test-chrome-secondary');
    expect(testExtensionMounts).toEqual({ chrome: 1, 'chrome-secondary': 1 });

    testExtensionFlags.modalSecondary = false;
    await waitFor(() => expect(markers(container)).toEqual(['test-chrome']));

    // The sibling is the same element, never remounted.
    expect(container.querySelector('[data-testid="test-chrome"]')).toBe(survivor);
    expect(testExtensionMounts.chrome).toBe(1);
  });

  it('a contribution whose when() throws is left out while its siblings keep rendering', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.modal = true;
    testExtensionFlags.throwingWhen = true;

    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-modal' }
    });

    await findByTestId('test-chrome');
    expect(markers(container)).toEqual(['test-chrome']);

    // The predicate recovers: it is false again, so still nothing extra, and no error surfaced.
    testExtensionFlags.throwingWhen = false;
    testExtensionFlags.modalSecondary = true;
    await findByTestId('test-chrome-secondary');
    expect(markers(container)).toEqual(['test-chrome-secondary', 'test-chrome']);
  });

  it('shows a contribution added while others are already mounted without remounting them', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.modal = true;

    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-modal' }
    });
    const first = await findByTestId('test-chrome');

    testExtensionFlags.modalSecondary = true;
    await findByTestId('test-chrome-secondary');

    expect(markers(container)).toEqual(['test-chrome-secondary', 'test-chrome']);
    expect(container.querySelector('[data-testid="test-chrome"]')).toBe(first);
    expect(testExtensionMounts.chrome).toBe(1);
  });
});
