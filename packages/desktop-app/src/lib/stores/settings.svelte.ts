import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';
import type { AiChatProvider } from '$lib/types/ai-chat-node';
import type { ProviderConfig } from '$lib/types';
import { readDatabaseSettings, updateDatabaseSettings } from '$lib/services/database-settings';

const log = createLogger('SettingsStore');

export type { ProviderConfig };

const LOCAL_STORAGE_KEY = 'nodespace-settings';

export interface ModelSelection {
  provider: AiChatProvider;
  modelId: string;
  configId?: string;
}

export interface AppSettings {
  activeDatabasePath: string;
  display: {
    renderMarkdown: boolean;
    theme: string;
  };
  defaultModelSelection: ModelSelection | null;
}

// ---------------------------------------------------------------------------
// localStorage helpers for client-side settings
//
// Only the app's own preference lives here: the default model choice. The
// OpenAI-compatible providers are a field of the database's settings node
// (ADR-095) — the source of truth the daemon reads by UUID when loading an
// "openai-compat:<uuid>" model. They are held in memory for the database the
// window shows, never copied to localStorage, where they would outlive a
// switch to another database and carry its API keys with them.
// ---------------------------------------------------------------------------

interface LocalPersistedSettings {
  defaultModelSelection?: ModelSelection | null;
}

function readLocalSettings(): LocalPersistedSettings {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(LOCAL_STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as LocalPersistedSettings;
  } catch {
    return {};
  }
}

function writeLocalSettings(patch: LocalPersistedSettings): void {
  if (typeof localStorage === 'undefined') return;
  try {
    const existing = readLocalSettings();
    localStorage.setItem(LOCAL_STORAGE_KEY, JSON.stringify({ ...existing, ...patch }));
  } catch (err) {
    log.warn('Failed to persist settings to localStorage', err);
  }
}

class SettingsStore {
  appSettings = $state<AppSettings | null>(null);

  /** The providers of the database this window shows, as last read from it. */
  openAiConfigs = $state<ProviderConfig[]>([]);

  /** Set before opening the settings tab to pre-select a category (e.g. 'integrations'). */
  initialCategory = $state<string | null>(null);

  async loadSettings(): Promise<void> {
    try {
      const settings = await invoke<Omit<AppSettings, 'defaultModelSelection'>>('get_settings');
      const local = readLocalSettings();

      try {
        await this.loadProviders();
      } catch (err) {
        log.warn("Failed to load the database's providers", err);
      }

      this.appSettings = {
        ...settings,
        defaultModelSelection: local.defaultModelSelection ?? null,
      };
    } catch (err) {
      log.error('Failed to load settings:', err);
    }
  }

  async updateDisplaySetting(
    key: 'renderMarkdown' | 'theme',
    value: boolean | string
  ): Promise<void> {
    try {
      const params: Record<string, unknown> = {};
      if (key === 'renderMarkdown') params.render_markdown = value;
      if (key === 'theme') params.theme = value;

      await invoke('update_display_settings', params);

      // Optimistic update
      if (this.appSettings) {
        this.appSettings = {
          ...this.appSettings,
          display: { ...this.appSettings.display, [key]: value },
        };
      }
    } catch (err) {
      log.error('Failed to update display setting:', err);
    }
  }

  /** Read the database's providers (the source of truth) into the store. */
  async loadProviders(): Promise<ProviderConfig[]> {
    const settings = await readDatabaseSettings();
    this.openAiConfigs = settings.providers;
    return settings.providers;
  }

  /**
   * Replace the database's providers through the settings node's typed
   * update. The store holds what the node holds afterward: the verdicts the
   * daemon keeps or drops are its own to decide.
   */
  async saveProviders(providers: ProviderConfig[]): Promise<void> {
    try {
      const node = await updateDatabaseSettings({ providers });
      this.openAiConfigs = node.providers;
    } catch (err) {
      log.error("Failed to save the database's providers:", err);
    }
  }

  saveDefaultModelSelection(selection: ModelSelection | null): void {
    writeLocalSettings({ defaultModelSelection: selection });
    if (this.appSettings) {
      this.appSettings = { ...this.appSettings, defaultModelSelection: selection };
    }
  }
}

export const settingsStore = new SettingsStore();

// ---------------------------------------------------------------------------
// Free-function delegators / helpers (keep existing callers working unchanged)
// ---------------------------------------------------------------------------

export function getOpenAiConfigs(): ProviderConfig[] {
  return settingsStore.openAiConfigs;
}

export function getDefaultModelSelection(): ModelSelection | null {
  return readLocalSettings().defaultModelSelection ?? null;
}

export function saveDefaultModelSelection(selection: ModelSelection | null): void {
  settingsStore.saveDefaultModelSelection(selection);
}

export const loadSettings = (): Promise<void> => settingsStore.loadSettings();

export const updateDisplaySetting = (
  key: 'renderMarkdown' | 'theme',
  value: boolean | string
): Promise<void> => settingsStore.updateDisplaySetting(key, value);
