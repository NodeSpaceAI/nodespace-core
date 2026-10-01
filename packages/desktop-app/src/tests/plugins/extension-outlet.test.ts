/**
 * ExtensionOutlet: mounts one lazily-loaded contribution and isolates its
 * failures (ADR-082 §3.4). A rejected `load()` renders nothing and is logged; a
 * component that throws removes only its own outlet.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import type { Component } from 'svelte';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

import ExtensionOutletGeneric from '$lib/plugins/extension-outlet.svelte';
import ChromeSlotOutlet from '$lib/plugins/chrome-slot-outlet.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';

// The outlet is generic over the loaded component's props; `render` cannot infer that,
// so the tests see it with the loosest props: any component, any prop bag.
const ExtensionOutlet = ExtensionOutletGeneric as unknown as Component<{
  load: () => Promise<{ default: Component<never> }>;
  props?: Record<string, unknown>;
}>;

describe('ExtensionOutlet', () => {
  beforeEach(() => {
    log.error.mockClear();
  });

  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('passes props to the loaded component', async () => {
    const { findByTestId } = render(ExtensionOutlet, {
      props: {
        load: () => import('../fixtures/test-extension/test-viewer-tab.svelte'),
        props: { nodeId: 'node-42' }
      }
    });

    const el = await findByTestId('test-viewer-tab');
    expect(el.getAttribute('data-node-id')).toBe('node-42');
    expect(log.error).not.toHaveBeenCalled();
  });

  it('renders nothing, and logs, when load() rejects', async () => {
    const failure = new Error('chunk failed to load');
    const load = vi.fn(() => Promise.reject(failure));
    const { container } = render(ExtensionOutlet, { props: { load } });

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('failed to load'),
        expect.objectContaining({ error: failure })
      )
    );
    expect(container.textContent).toBe('');
    expect(container.querySelector('[data-testid]')).toBeNull();
    // Not retried in a loop: one mount, one attempt.
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('renders nothing, and logs, when load() throws synchronously', async () => {
    const failure = new Error('load threw');
    const { container } = render(ExtensionOutlet, {
      props: {
        load: () => {
          throw failure;
        }
      }
    });

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('failed to load'),
        expect.objectContaining({ error: failure })
      )
    );
    expect(container.querySelector('[data-testid]')).toBeNull();
  });

  it('renders nothing, and logs, when load() returns something that is not a promise', async () => {
    const { container } = render(ExtensionOutlet, {
      props: { load: (() => undefined) as never }
    });

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('failed to load'),
        expect.objectContaining({ error: expect.any(TypeError) })
      )
    );
    expect(container.querySelector('[data-testid]')).toBeNull();
  });

  it('renders nothing, and logs, when the loaded module has no default component', async () => {
    const { container } = render(ExtensionOutlet, {
      props: { load: (() => Promise.resolve({})) as never }
    });

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('failed to load'),
        expect.objectContaining({ error: expect.any(TypeError) })
      )
    );
    expect(container.querySelector('[data-testid]')).toBeNull();
  });

  it('renders nothing, and logs, when the loaded component throws while rendering', async () => {
    const { container } = render(ExtensionOutlet, {
      props: { load: () => import('../fixtures/test-extension/test-throwing.svelte') }
    });

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('threw'),
        expect.objectContaining({ error: expect.objectContaining({ message: expect.stringContaining('render failed') }) })
      )
    );
    expect(container.querySelector('[data-testid]')).toBeNull();
  });

  it('a throwing component removes only its own outlet; a sibling outlet still renders', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.modal = true;
    testExtensionFlags.throwingComponent = true;

    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-modal' }
    });

    await findByTestId('test-chrome');
    await waitFor(() => expect(log.error).toHaveBeenCalledWith(expect.stringContaining('threw'), expect.anything()));
    expect(container.querySelector('[data-testid="test-throwing"]')).toBeNull();
    expect(container.querySelectorAll('[data-testid]')).toHaveLength(1);
  });

  it('a rejected load() leaves a sibling outlet rendering', async () => {
    uiExtensionRegistry.register(
      createTestExtension({
        chrome: [
          {
            id: 'rejects',
            slot: 'app-shell-modal',
            priority: 5,
            load: () => Promise.reject(new Error('chunk failed to load'))
          },
          {
            id: 'fine',
            slot: 'app-shell-modal',
            load: () => import('../fixtures/test-extension/test-chrome.svelte')
          }
        ]
      })
    );

    const { container, findByTestId } = render(ChromeSlotOutlet, {
      props: { name: 'app-shell-modal' }
    });

    await findByTestId('test-chrome');
    await waitFor(() => expect(log.error).toHaveBeenCalled());
    expect(container.querySelectorAll('[data-testid]')).toHaveLength(1);
  });
});
