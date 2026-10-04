/**
 * SettingsPane + SettingsSidebar with a contributed section.
 *
 * Uses the fixture extension, so it exercises the generic host only: where a
 * section sits in the sidebar, that it renders when selected (by click or by
 * `settingsStore.initialCategory`) with a working `navigate`, and that the pane
 * falls back to Database when the viewed category is not listed.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor, within } from '@testing-library/svelte';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

import SettingsPane from '$lib/components/settings/settings-pane.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { setAllLabsFlags } from '../../helpers/labs-flags';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags,
  testExtensionMounts,
  type TestSectionOptions
} from '../../fixtures/test-extension';

function registerFixture(section: TestSectionOptions = {}) {
  uiExtensionRegistry.register(createTestExtension({}, { section }));
}

function sidebarLabels(container: HTMLElement): string[] {
  return within(within(container).getByRole('navigation'))
    .getAllByRole('button')
    .map((b) => b.textContent?.trim() ?? '');
}

function sidebarButton(container: HTMLElement, name: string): HTMLElement {
  return within(within(container).getByRole('navigation')).getByRole('button', { name });
}

const SECTION_CONTENT = 'test-settings-section';

describe('SettingsPane with a contributed section', () => {
  beforeEach(() => {
    localStorage.clear();
    setAllLabsFlags(false);
    settingsStore.initialCategory = null;
    log.warn.mockClear();
    log.error.mockClear();
  });

  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
    localStorage.clear();
    settingsStore.initialCategory = null;
  });

  it('adds nothing to the sidebar while nothing is registered', () => {
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container)).toEqual([
      'Database',
      'Display',
      'Import Sources',
      'Integrations',
      'Labs',
      'About'
    ]);
  });

  it('lists the section after Database when `after` names it', () => {
    registerFixture({ after: 'database' });
    testExtensionFlags.section = true;
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container)).toEqual([
      'Database',
      'Test section',
      'Display',
      'Import Sources',
      'Integrations',
      'Labs',
      'About'
    ]);
  });

  it('lists the section before About when `after` is absent', () => {
    registerFixture();
    testExtensionFlags.section = true;
    const view = render(SettingsPane);

    const labels = sidebarLabels(view.container);
    expect(labels.slice(-3)).toEqual(['Labs', 'Test section', 'About']);
  });

  it('uses the section’s own label', () => {
    registerFixture({ label: 'Renamed' });
    testExtensionFlags.section = true;
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container)).toContain('Renamed');
    expect(sidebarLabels(view.container)).not.toContain('Test section');
  });

  it('hides the section while its when() is false, and shows it reactively once true', async () => {
    registerFixture();
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container)).not.toContain('Test section');

    testExtensionFlags.section = true;
    await waitFor(() => expect(sidebarLabels(view.container)).toContain('Test section'));

    testExtensionFlags.section = false;
    await waitFor(() => expect(sidebarLabels(view.container)).not.toContain('Test section'));
  });

  it('renders the section when its sidebar entry is clicked', async () => {
    registerFixture({ after: 'database' });
    testExtensionFlags.section = true;
    const view = render(SettingsPane);
    await view.findByText('Databases');

    await fireEvent.click(sidebarButton(view.container, 'Test section'));

    expect(await view.findByTestId(SECTION_CONTENT)).toBeTruthy();
    expect(view.queryByText('Databases')).toBeNull();
    expect(testExtensionMounts['settings-section']).toBe(1);
  });

  it('renders the section when it is the initial category', async () => {
    registerFixture();
    testExtensionFlags.section = true;
    settingsStore.initialCategory = 'test-section';
    const view = render(SettingsPane);

    expect(await view.findByTestId(SECTION_CONTENT)).toBeTruthy();
    expect(view.queryByText('Databases')).toBeNull();
  });

  it('marks the section’s sidebar entry active while it is showing', async () => {
    registerFixture();
    testExtensionFlags.section = true;
    settingsStore.initialCategory = 'test-section';
    const view = render(SettingsPane);
    await view.findByTestId(SECTION_CONTENT);

    expect(sidebarButton(view.container, 'Test section').className).toContain('text-primary');
    expect(sidebarButton(view.container, 'Database').className).not.toContain('text-primary');
  });

  it('gives the section a navigate() that switches the pane to another category', async () => {
    registerFixture();
    testExtensionFlags.section = true;
    settingsStore.initialCategory = 'test-section';
    const view = render(SettingsPane);
    await view.findByTestId(SECTION_CONTENT);

    await fireEvent.click(view.getByRole('button', { name: 'Go to Database' }));

    expect(await view.findByText('Databases')).toBeTruthy();
    expect(view.queryByTestId(SECTION_CONTENT)).toBeNull();
  });

  it('falls back to Database for an id nothing registered', async () => {
    registerFixture();
    testExtensionFlags.section = true;
    settingsStore.initialCategory = 'no-such-section';
    const view = render(SettingsPane);

    expect(await view.findByText('Databases')).toBeTruthy();
    expect(view.queryByTestId(SECTION_CONTENT)).toBeNull();
    expect(sidebarButton(view.container, 'Database').className).toContain('text-primary');
  });

  it('falls back to Database when the section is the initial category but its when() is false', async () => {
    registerFixture();
    settingsStore.initialCategory = 'test-section';
    const view = render(SettingsPane);

    expect(await view.findByText('Databases')).toBeTruthy();
    expect(view.queryByTestId(SECTION_CONTENT)).toBeNull();
  });

  it('falls back to Database when the open section’s when() turns false', async () => {
    registerFixture();
    testExtensionFlags.section = true;
    settingsStore.initialCategory = 'test-section';
    const view = render(SettingsPane);
    await view.findByTestId(SECTION_CONTENT);

    testExtensionFlags.section = false;

    await waitFor(() => expect(view.getByText('Databases')).toBeTruthy());
    expect(view.queryByTestId(SECTION_CONTENT)).toBeNull();
    expect(sidebarLabels(view.container)).not.toContain('Test section');
  });

  it('ignores a section whose id is a core id, logs it, and leaves the core category working', async () => {
    registerFixture({ id: 'display', label: 'Impostor' });
    testExtensionFlags.section = true;
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container).filter((l) => l === 'Display')).toHaveLength(1);
    expect(sidebarLabels(view.container)).not.toContain('Impostor');

    await fireEvent.click(sidebarButton(view.container, 'Display'));
    await waitFor(() => expect(view.queryByText('Databases')).toBeNull());
    expect(view.queryByTestId(SECTION_CONTENT)).toBeNull();
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('ignored'),
      expect.objectContaining({ key: `${TEST_EXTENSION_ID}/display` })
    );
  });

  it('lists a section after a Labs-gated category once that category is listed', () => {
    registerFixture({ after: 'ai-models' });
    testExtensionFlags.section = true;
    labsFlags.aiChatEnabled = true;
    const view = render(SettingsPane);

    expect(sidebarLabels(view.container).slice(2, 4)).toEqual(['AI Models', 'Test section']);
  });
});
