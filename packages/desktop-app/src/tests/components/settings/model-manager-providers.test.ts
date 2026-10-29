/**
 * ModelManager — provider save failures.
 *
 * A provider the database refused to store must not appear saved: the list
 * rolls back to what the store holds, the editor stays open with the user's
 * input, and an error is shown.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor, screen } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const readDatabaseSettings = vi.fn();
const updateDatabaseSettings = vi.fn();
vi.mock('$lib/services/database-settings', () => ({
  readDatabaseSettings: (...args: unknown[]) => readDatabaseSettings(...args),
  updateDatabaseSettings: (...args: unknown[]) => updateDatabaseSettings(...args)
}));

vi.mock('$lib/services/tauri-commands', () => ({
  chatModelList: vi.fn().mockResolvedValue([])
}));

vi.mock('$lib/stores/model-store.svelte', () => ({
  modelStore: {
    models: [],
    downloadProgress: {},
    isLoading: false,
    systemRamGb: 0,
    refreshModels: vi.fn()
  },
  formatBytes: (n: number) => String(n)
}));

import ModelManager from '$lib/components/settings/model-manager.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';
import type { ProviderConfig } from '$lib/types';

const existing: ProviderConfig = {
  id: 'p-existing',
  name: 'Existing',
  base_url: 'https://existing.test/v1',
  api_key: 'sk-existing',
  model: 'gpt-4o',
  routing_ok: {}
};

async function fillAndSave(name: string): Promise<void> {
  await fireEvent.click(await screen.findByText('Add'));
  await fireEvent.input(screen.getByPlaceholderText('e.g. My OpenAI Key'), {
    target: { value: name }
  });
  await fireEvent.input(screen.getByPlaceholderText('https://api.openai.com/v1'), {
    target: { value: 'https://new.test/v1' }
  });
  await fireEvent.input(screen.getByPlaceholderText('e.g. gpt-4o'), {
    target: { value: 'gpt-4o' }
  });
  await fireEvent.click(screen.getByText('Save'));
}

describe('ModelManager provider persistence', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsStore.openAiConfigs = [existing];
    readDatabaseSettings.mockResolvedValue({ providers: [existing] });
  });

  afterEach(() => {
    cleanup();
  });

  it('does not list a new provider whose save failed, and keeps the editor open with an error', async () => {
    updateDatabaseSettings.mockRejectedValue(new Error('daemon down'));
    render(ModelManager);
    await screen.findByText('Existing');

    await fillAndSave('Brand New');

    await waitFor(() => expect(updateDatabaseSettings).toHaveBeenCalled());
    await screen.findByRole('alert');
    expect(screen.queryByText('Brand New')).toBeNull();
    expect(screen.getByText('Existing')).toBeTruthy();
    expect((screen.getByPlaceholderText('e.g. My OpenAI Key') as HTMLInputElement).value).toBe(
      'Brand New'
    );
  });

  it('keeps a provider whose removal failed, and shows an error', async () => {
    updateDatabaseSettings.mockRejectedValue(new Error('daemon down'));
    render(ModelManager);
    await screen.findByText('Existing');

    await fireEvent.click(screen.getByText('Remove'));

    await screen.findByRole('alert');
    expect(screen.getByText('Existing')).toBeTruthy();
  });

  it('lists a new provider once the save succeeds, with no error', async () => {
    updateDatabaseSettings.mockImplementation(
      async ({ providers }: { providers: ProviderConfig[] }) => ({
        providers
      })
    );
    render(ModelManager);
    await screen.findByText('Existing');

    await fillAndSave('Brand New');

    await screen.findByText('Brand New');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('sends one write for a double-clicked Save of a new provider', async () => {
    let resolveWrite: (v: { providers: ProviderConfig[] }) => void = () => {};
    updateDatabaseSettings.mockImplementation(
      () => new Promise((resolve) => (resolveWrite = resolve))
    );
    render(ModelManager);
    await screen.findByText('Existing');

    await fillAndSave('Brand New');
    await fireEvent.click(screen.getByText('Saving…'));
    await fireEvent.click(screen.getByText('Saving…'));

    expect(updateDatabaseSettings).toHaveBeenCalledTimes(1);
    const sent = updateDatabaseSettings.mock.calls[0][0].providers as ProviderConfig[];
    expect(sent.filter((p) => p.name === 'Brand New')).toHaveLength(1);

    resolveWrite({ providers: sent });
    await screen.findByText('Brand New');
    expect(screen.getAllByText('Brand New')).toHaveLength(1);
  });

  it('ignores Remove while a write is in flight, so a failed write cannot roll back over it', async () => {
    let rejectWrite: (e: Error) => void = () => {};
    updateDatabaseSettings.mockImplementation(
      () => new Promise((_, reject) => (rejectWrite = reject))
    );
    render(ModelManager);
    await screen.findByText('Existing');

    await fillAndSave('Brand New');
    await fireEvent.click(screen.getAllByText('Remove')[0]);

    expect(updateDatabaseSettings).toHaveBeenCalledTimes(1);
    rejectWrite(new Error('daemon down'));
    await screen.findByRole('alert');
    expect(screen.getByText('Existing')).toBeTruthy();
  });

  it('clears the provider error when a new edit starts', async () => {
    updateDatabaseSettings.mockRejectedValue(new Error('daemon down'));
    render(ModelManager);
    await screen.findByText('Existing');
    await fireEvent.click(screen.getByText('Remove'));
    await screen.findByRole('alert');

    await fireEvent.click(screen.getByText('Edit'));
    expect(screen.queryByRole('alert')).toBeNull();

    await fireEvent.click(screen.getByText('Remove'));
    await screen.findByRole('alert');
    await fireEvent.click(screen.getByText('Add'));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('keeps Add and Cancel disabled and the form intact while a save is in flight', async () => {
    let resolveWrite: (v: { providers: ProviderConfig[] }) => void = () => {};
    updateDatabaseSettings.mockImplementation(
      () => new Promise((resolve) => (resolveWrite = resolve))
    );
    render(ModelManager);
    await screen.findByText('Existing');

    await fillAndSave('Brand New');

    expect((screen.getByText('Add') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText('Cancel') as HTMLButtonElement).disabled).toBe(true);
    const save = screen.getByText('Saving…') as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(save.getAttribute('aria-busy')).toBe('true');
    expect((screen.getByPlaceholderText('e.g. My OpenAI Key') as HTMLInputElement).value).toBe(
      'Brand New'
    );

    const sent = updateDatabaseSettings.mock.calls[0][0].providers as ProviderConfig[];
    resolveWrite({ providers: sent });
    await screen.findByText('Brand New');
  });
});
