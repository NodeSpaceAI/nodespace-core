/**
 * ImportOptionsModal — completion state. The modal flips to done as soon as the
 * import command resolves and tells the user semantic-search indexing continues
 * in the background.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

const { mockService, mockLoadCollections } = vi.hoisted(() => ({
  mockService: {
    selectFolder: vi.fn(),
    importDirectory: vi.fn(),
    onProgress: vi.fn(),
  },
  mockLoadCollections: vi.fn(),
}));

vi.mock('$lib/services/import-service', () => ({ importService: mockService }));
vi.mock('$lib/stores/collections.svelte', () => ({
  collectionsData: { loadCollections: mockLoadCollections },
}));

import ImportOptionsModal from '$lib/components/settings/import-options-modal.svelte';

const batch = (over: Record<string, unknown> = {}) => ({
  total_files: 338,
  successful: 338,
  failed: 0,
  results: [],
  duration_ms: 1,
  ...over,
});

async function startImport() {
  render(ImportOptionsModal, { open: true });
  await fireEvent.click(await screen.findByText('Choose Folder…'));
  await waitFor(() => expect(screen.getByText('/docs')).toBeInTheDocument());
  await fireEvent.click(screen.getByRole('button', { name: 'Import' }));
}

describe('ImportOptionsModal completion', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockService.selectFolder.mockResolvedValue('/docs');
    mockService.onProgress.mockReturnValue(() => {});
    mockLoadCollections.mockResolvedValue(undefined);
  });

  it('shows Importing until the command resolves, then the done state with background-indexing copy', async () => {
    let resolveImport!: (r: unknown) => void;
    mockService.importDirectory.mockReturnValue(
      new Promise((resolve) => {
        resolveImport = resolve;
      })
    );

    await startImport();
    expect(await screen.findByText('Importing…')).toBeInTheDocument();

    resolveImport(batch());

    expect(await screen.findByText('Import complete')).toBeInTheDocument();
    expect(screen.getByText('338 of 338 files imported.')).toBeInTheDocument();
    expect(
      screen.getByText('Indexing for semantic search is running in the background.')
    ).toBeInTheDocument();
    expect(screen.queryByText('Importing…')).not.toBeInTheDocument();
  });

  it('shows the same background-indexing note when some files failed', async () => {
    mockService.importDirectory.mockResolvedValue(batch({ successful: 337, failed: 1 }));

    await startImport();

    expect(await screen.findByText('Imported with issues')).toBeInTheDocument();
    expect(
      screen.getByText('Indexing for semantic search is running in the background.')
    ).toBeInTheDocument();
  });
});
