/**
 * The `collaboration.entry` replaceable slot on the Labs page (ADR-082 §3.2,
 * §3.4; ADR-084 §1). Core's contact card is the slot's default and renders only
 * while no contribution is registered for the slot. A registered contribution
 * keeps it out even while hidden, when its `when()` throws, its `load()` rejects
 * or its component throws; at most one contribution renders, the visible one
 * with the highest priority, then the first registered.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

import LabsSettings from '$lib/components/settings/sections/labs-settings.svelte';
import { uiExtensionRegistry, type NodespaceExtension } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags,
  testExtensionMounts
} from '../../fixtures/test-extension';

const SECOND_EXTENSION_ID = 'second-test-extension';

const CONTACT_TEXT = 'Want team collaboration?';

/** The fixture markers in the container, in document order. */
function markers(container: HTMLElement): string[] {
  return [...container.querySelectorAll('[data-testid]')].map(
    (el) => el.getAttribute('data-testid') ?? ''
  );
}

function showsContactCard(container: HTMLElement): boolean {
  return container.textContent?.includes(CONTACT_TEXT) ?? false;
}

/** Let any (wrongly) started load land, so an absence check is not vacuous. */
async function settle(): Promise<void> {
  await import('../../fixtures/test-extension/test-collaboration-entry.svelte');
  await import('../../fixtures/test-extension/test-collaboration-entry-secondary.svelte');
  await new Promise((resolve) => setTimeout(resolve, 20));
}

describe('Labs page: the collaboration.entry replaceable slot', () => {
  beforeEach(() => {
    log.warn.mockClear();
    log.error.mockClear();
  });

  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    uiExtensionRegistry.unregister(SECOND_EXTENSION_ID);
    resetTestExtension();
  });

  it("renders core's contact card while nothing is registered for the slot", () => {
    const { container } = render(LabsSettings);

    expect(showsContactCard(container)).toBe(true);
  });

  it('renders the contact card when a registered extension contributes nothing to the slot', () => {
    uiExtensionRegistry.register(createTestExtension({ replaceableSlots: undefined }));
    const { container } = render(LabsSettings);

    expect(showsContactCard(container)).toBe(true);
  });

  it('renders a registered, visible contribution instead of the contact card', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntry = true;

    const { container, findByTestId } = render(LabsSettings);

    await findByTestId('test-collaboration-entry');
    expect(showsContactCard(container)).toBe(false);
  });

  it('keeps the contact card out while the registered contribution is hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());

    const { container } = render(LabsSettings);
    await settle();

    expect(showsContactCard(container)).toBe(false);
    expect(markers(container)).toEqual([]);
  });

  it('does not bring the contact card back when a shown contribution becomes hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntry = true;

    const { container, findByTestId } = render(LabsSettings);
    await findByTestId('test-collaboration-entry');

    testExtensionFlags.collaborationEntry = false;
    await waitFor(() => expect(markers(container)).toEqual([]));
    await settle();
    expect(showsContactCard(container)).toBe(false);
  });

  it("keeps the contact card out, and logs, when the contribution's when() throws", async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntryThrowingWhen = true;

    const { container } = render(LabsSettings);
    await settle();

    expect(showsContactCard(container)).toBe(false);
    expect(markers(container)).toEqual([]);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('when() threw'),
      expect.objectContaining({ key: `${TEST_EXTENSION_ID}/collaboration-entry-throwing-when` })
    );
  });

  it("keeps the contact card out, and logs, when the contribution's load() rejects", async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntryFailingLoad = true;

    const { container } = render(LabsSettings);

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('failed to load'),
        expect.anything()
      )
    );
    expect(showsContactCard(container)).toBe(false);
    expect(markers(container)).toEqual([]);
  });

  it("keeps the contact card out, and logs, when the contribution's component throws", async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntryThrowing = true;

    const { container } = render(LabsSettings);

    await waitFor(() =>
      expect(log.error).toHaveBeenCalledWith(
        expect.stringContaining('Extension component threw'),
        expect.anything()
      )
    );
    expect(showsContactCard(container)).toBe(false);
    expect(markers(container)).toEqual([]);
    // The rest of the Labs page still renders.
    expect(container.textContent).toContain('Playbooks');
  });

  it('renders only the visible contribution with the highest priority', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntry = true;
    testExtensionFlags.collaborationEntrySecondary = true;

    const { container, findByTestId } = render(LabsSettings);

    await findByTestId('test-collaboration-entry-secondary');
    await settle();
    // `collaboration-entry-secondary` has priority 10; `collaboration-entry` is
    // declared first but has the default 0, so it does not render at all.
    expect(markers(container)).toEqual(['test-collaboration-entry-secondary']);
    expect(testExtensionMounts['collaboration-entry']).toBeUndefined();
  });

  it('falls back to the next visible contribution when the higher-priority one is hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.collaborationEntry = true;
    testExtensionFlags.collaborationEntrySecondary = true;

    const { container, findByTestId } = render(LabsSettings);
    await findByTestId('test-collaboration-entry-secondary');

    testExtensionFlags.collaborationEntrySecondary = false;
    await findByTestId('test-collaboration-entry');
    expect(markers(container)).toEqual(['test-collaboration-entry']);
    expect(showsContactCard(container)).toBe(false);
  });

  it('keeps the rendered contribution mounted while the state its when() reads changes', async () => {
    // `when()` reads a flag (`section`) whose changes never change its result.
    const second: NodespaceExtension = {
      id: SECOND_EXTENSION_ID,
      replaceableSlots: [
        {
          id: 'entry',
          slot: 'collaboration.entry',
          when: () => {
            void testExtensionFlags.section;
            return true;
          },
          load: () => import('../../fixtures/test-extension/test-collaboration-entry.svelte')
        }
      ]
    };
    uiExtensionRegistry.register(second);

    const { container, findByTestId } = render(LabsSettings);
    const mounted = await findByTestId('test-collaboration-entry');

    testExtensionFlags.section = true;
    await settle();
    testExtensionFlags.section = false;
    await settle();

    expect(testExtensionMounts['collaboration-entry']).toBe(1);
    expect(container.querySelector('[data-testid="test-collaboration-entry"]')).toBe(mounted);
  });

  it('keeps the rendered contribution mounted while a hidden, higher-priority one re-evaluates', async () => {
    const second: NodespaceExtension = {
      id: SECOND_EXTENSION_ID,
      replaceableSlots: [
        {
          // Evaluated first (higher priority) on every pass; stays hidden.
          id: 'hidden',
          slot: 'collaboration.entry',
          priority: 5,
          when: () => {
            void testExtensionFlags.section;
            return false;
          },
          load: () =>
            import('../../fixtures/test-extension/test-collaboration-entry-secondary.svelte')
        },
        {
          id: 'shown',
          slot: 'collaboration.entry',
          load: () => import('../../fixtures/test-extension/test-collaboration-entry.svelte')
        }
      ]
    };
    uiExtensionRegistry.register(second);

    const { findByTestId } = render(LabsSettings);
    await findByTestId('test-collaboration-entry');

    testExtensionFlags.section = true;
    await settle();

    expect(testExtensionMounts['collaboration-entry']).toBe(1);
    expect(testExtensionMounts['collaboration-entry-secondary']).toBeUndefined();
  });

  it('among visible contributions of equal priority, renders only the first registered', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const second: NodespaceExtension = {
      id: SECOND_EXTENSION_ID,
      replaceableSlots: [
        {
          id: 'entry',
          slot: 'collaboration.entry',
          load: () =>
            import('../../fixtures/test-extension/test-collaboration-entry-secondary.svelte')
        }
      ]
    };
    uiExtensionRegistry.register(second);
    testExtensionFlags.collaborationEntry = true;

    const { container, findByTestId } = render(LabsSettings);

    await findByTestId('test-collaboration-entry');
    await settle();
    expect(markers(container)).toEqual(['test-collaboration-entry']);
  });
});
