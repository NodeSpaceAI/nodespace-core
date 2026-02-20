import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('SettingsStore');

export interface AppSettings {
    activeDatabasePath: string;
    savedDatabasePath: string | null;
    display: {
        renderMarkdown: boolean;
        theme: string;
    };
}

export const appSettings = writable<AppSettings | null>(null);

export async function loadSettings(): Promise<void> {
    try {
        const settings = await invoke<AppSettings>('get_settings');
        appSettings.set(settings);
    } catch (err) {
        log.error('Failed to load settings:', err);
    }
}

export async function updateDisplaySetting(
    key: 'renderMarkdown' | 'theme',
    value: boolean | string
): Promise<void> {
    try {
        const params: Record<string, unknown> = {};
        if (key === 'renderMarkdown') params.render_markdown = value;
        if (key === 'theme') params.theme = value;

        await invoke('update_display_settings', params);

        // Optimistic update
        appSettings.update((s) =>
            s ? { ...s, display: { ...s.display, [key]: value } } : null
        );
    } catch (err) {
        log.error('Failed to update display setting:', err);
    }
}
