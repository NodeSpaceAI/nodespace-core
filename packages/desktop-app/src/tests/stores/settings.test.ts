import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

const readDatabaseSettings = vi.fn();
const updateDatabaseSettings = vi.fn();
vi.mock('$lib/services/database-settings', () => ({
  readDatabaseSettings: (...args: unknown[]) => readDatabaseSettings(...args),
  updateDatabaseSettings: (...args: unknown[]) => updateDatabaseSettings(...args)
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import {
  settingsStore,
  loadSettings,
  updateDisplaySetting,
  getOpenAiConfigs,
} from '$lib/stores/settings.svelte';
import type { AppSettings, ProviderConfig } from '$lib/stores/settings.svelte';

describe('Settings Store', () => {
  const mockSettings: AppSettings = {
    activeDatabasePath: '/tmp/test.db',
    display: {
      renderMarkdown: true,
      theme: 'light'
    },
    defaultModelSelection: null,
  };

  function enableTauri(): void {
    (globalThis.window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  }

  function disableTauri(): void {
    delete (globalThis.window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  }

  beforeEach(() => {
    vi.clearAllMocks();
    settingsStore.appSettings = null;
    settingsStore.openAiConfigs = [];
    readDatabaseSettings.mockRejectedValue(new Error('no database settings'));
    localStorage.clear();
    disableTauri();
  });

  afterEach(() => {
    // singleFork test execution shares one `window` across every test file
    // in the run — an enabled flag left set here would leak into unrelated
    // suites (e.g. tauri-commands.test.ts's "outside Tauri" fallback tests).
    disableTauri();
    localStorage.clear();
  });

  describe('appSettings store', () => {
    it('should start as null', () => {
      expect(settingsStore.appSettings).toBeNull();
    });
  });

  describe('loadSettings', () => {
    it('should call invoke and set store', async () => {
      // Backend only returns the persisted (non-localStorage) subset.
      const backendSettings = {
        activeDatabasePath: mockSettings.activeDatabasePath,
        display: mockSettings.display,
      };
      mockInvoke.mockResolvedValueOnce(backendSettings);

      await loadSettings();

      expect(mockInvoke).toHaveBeenCalledWith('get_settings');
      // Store merges backend fields with localStorage defaults.
      expect(settingsStore.appSettings).toEqual(mockSettings);
    });

    it('should handle errors gracefully', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('invoke failed'));

      await loadSettings();

      expect(settingsStore.appSettings).toBeNull();
    });
  });

  describe('updateDisplaySetting', () => {
    it('should update renderMarkdown setting', async () => {
      settingsStore.appSettings = mockSettings;
      mockInvoke.mockResolvedValueOnce(undefined);

      await updateDisplaySetting('renderMarkdown', false);

      expect(mockInvoke).toHaveBeenCalledWith('update_display_settings', {
        render_markdown: false
      });
      expect(settingsStore.appSettings?.display.renderMarkdown).toBe(false);
    });

    it('should update theme setting', async () => {
      settingsStore.appSettings = mockSettings;
      mockInvoke.mockResolvedValueOnce(undefined);

      await updateDisplaySetting('theme', 'dark');

      expect(mockInvoke).toHaveBeenCalledWith('update_display_settings', {
        theme: 'dark'
      });
      expect(settingsStore.appSettings?.display.theme).toBe('dark');
    });

    it('should handle errors gracefully', async () => {
      settingsStore.appSettings = mockSettings;
      mockInvoke.mockRejectedValueOnce(new Error('update failed'));

      await updateDisplaySetting('renderMarkdown', false);

      // Optimistic update is after the await, so it's skipped when invoke rejects
      expect(settingsStore.appSettings?.display.renderMarkdown).toBe(true);
    });

    it('should return null when store is null', async () => {
      mockInvoke.mockResolvedValueOnce(undefined);

      await updateDisplaySetting('theme', 'dark');

      // Optimistic update on null store should keep it null
      expect(settingsStore.appSettings).toBeNull();
    });
  });

  describe("providers in the database's settings node", () => {
    const provider: ProviderConfig = {
      id: '0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d',
      name: 'My Endpoint',
      base_url: 'https://api.example.com/v1',
      api_key: 'sk-test',
      model: 'gpt-4o',
      routing_ok: {}
    };

    it('loadSettings reads the providers from the settings node', async () => {
      enableTauri();
      const backendSettings = {
        activeDatabasePath: mockSettings.activeDatabasePath,
        display: mockSettings.display,
      };
      mockInvoke.mockImplementation((cmd: string) =>
        Promise.resolve(cmd === 'get_settings' ? backendSettings : undefined)
      );
      readDatabaseSettings.mockResolvedValue({ providers: [provider] });

      await loadSettings();

      expect(readDatabaseSettings).toHaveBeenCalled();
      expect(settingsStore.openAiConfigs).toEqual([provider]);
      expect(getOpenAiConfigs()).toEqual([provider]);
      expect(settingsStore.appSettings).toEqual(mockSettings);
    });

    it('loadSettings still loads the app settings when the settings node cannot be read', async () => {
      enableTauri();
      mockInvoke.mockImplementation((cmd: string) =>
        Promise.resolve(
          cmd === 'get_settings'
            ? { activeDatabasePath: '/tmp/test.db', display: mockSettings.display }
            : undefined
        )
      );
      readDatabaseSettings.mockRejectedValue(new Error('daemon unreachable'));

      await loadSettings();

      expect(settingsStore.appSettings).toEqual(mockSettings);
      expect(settingsStore.openAiConfigs).toEqual([]);
    });

    it('saveProviders writes through the typed update and holds what the node holds', async () => {
      const stored = { ...provider, routing_ok: { 'gpt-4o': true } };
      updateDatabaseSettings.mockResolvedValue({ providers: [stored] });

      await settingsStore.saveProviders([provider]);

      expect(updateDatabaseSettings).toHaveBeenCalledWith({ providers: [provider] });
      expect(settingsStore.openAiConfigs).toEqual([stored]);
    });

    it('never copies providers, or their keys, into localStorage', async () => {
      updateDatabaseSettings.mockResolvedValue({ providers: [provider] });
      readDatabaseSettings.mockResolvedValue({ providers: [provider] });

      await settingsStore.saveProviders([provider]);
      await settingsStore.loadProviders();

      expect(JSON.stringify(localStorage.getItem('nodespace-settings'))).not.toContain('sk-test');
    });

    it('keeps the previous providers when the write fails', async () => {
      settingsStore.openAiConfigs = [provider];
      updateDatabaseSettings.mockRejectedValue(new Error('conflict'));

      await settingsStore.saveProviders([]);

      expect(settingsStore.openAiConfigs).toEqual([provider]);
    });
  });
});
