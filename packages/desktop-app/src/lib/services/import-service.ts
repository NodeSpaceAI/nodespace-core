/**
 * Import Service
 *
 * Frontend service for importing markdown files/directories into NodeSpace.
 * Uses Tauri commands for direct Rust-based import (bypassing MCP for performance).
 */

import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('ImportService');

// ============================================================================
// Types
// ============================================================================

export interface ImportOptions {
  /** Collection path to add imported documents to */
  collection?: string;
  /** Whether to use filename as title (default: false - uses first heading) */
  use_filename_as_title?: boolean;
  /** Enable smart collection routing based on file paths */
  auto_collection_routing?: boolean;
  /** Directory patterns to exclude (e.g., ["design-system", "node_modules"]) */
  exclude_patterns?: string[];
  /** Base directory for relative path calculation in auto-routing */
  base_directory?: string;
}

export interface FileImportResult {
  file_path: string;
  root_id: string | null;
  nodes_created: number;
  success: boolean;
  error: string | null;
  collection: string | null;
  archived: boolean;
}

export interface BatchImportResult {
  total_files: number;
  successful: number;
  failed: number;
  results: FileImportResult[];
  duration_ms: number;
}

/**
 * Progress event emitted during import
 *
 * Shows 9 distinct steps during import:
 * 1. Scanning folder
 * 2. Reading files (shows each filename)
 * 3. Parsing markdown (shows each filename)
 * 4. Resolving links
 * 5. Creating collections
 * 6. Importing nodes
 * 7. Assigning to collections
 * 8. Creating references
 * 9. Complete (shows summary)
 */
export interface ImportProgressEvent {
  /** Step number (1-9) */
  step: number;
  /** Step name (e.g., "scanning", "reading", "parsing", etc.) */
  step_name: string;
  /** User-friendly message (e.g., "Reading: overview.md") */
  message: string;
  /** Current item in step (if applicable) */
  current: number;
  /** Total items in step (if applicable) */
  total: number;
}

// ============================================================================
// Import Service
// ============================================================================

class ImportService {
  private progressListeners: Set<(event: ImportProgressEvent) => void> = new Set();
  private unlistenProgress: (() => void) | null = null;

  /**
   * Open a folder picker dialog and return the selected path
   */
  async selectFolder(): Promise<string | null> {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Select folder to import',
      });
      return selected as string | null;
    } catch (error) {
      log.error('Failed to open folder dialog', error);
      return null;
    }
  }

  /**
   * Import all markdown files from a directory
   */
  async importDirectory(
    directoryPath: string,
    options?: ImportOptions
  ): Promise<BatchImportResult> {
    log.info('Starting directory import', { path: directoryPath, options });

    // Set up progress listener
    await this.setupProgressListener();

    try {
      const result = await invoke<BatchImportResult>('import_markdown_directory', {
        directoryPath,
        options: options || {},
      });

      log.info('Directory import complete', {
        total: result.total_files,
        successful: result.successful,
        failed: result.failed,
        duration_ms: result.duration_ms,
      });

      return result;
    } finally {
      // Clean up progress listener
      this.teardownProgressListener();
    }
  }

  /**
   * Import a single markdown file
   */
  async importFile(filePath: string, options?: ImportOptions): Promise<FileImportResult> {
    log.info('Starting file import', { path: filePath, options });

    const result = await invoke<FileImportResult>('import_markdown_file', {
      filePath,
      options: options || {},
    });

    if (result.success) {
      log.info('File import complete', {
        root_id: result.root_id,
        nodes_created: result.nodes_created,
      });
    } else {
      log.error('File import failed', { error: result.error });
    }

    return result;
  }

  /**
   * Subscribe to import progress events
   */
  onProgress(callback: (event: ImportProgressEvent) => void): () => void {
    this.progressListeners.add(callback);
    return () => {
      this.progressListeners.delete(callback);
    };
  }

  private async setupProgressListener(): Promise<void> {
    if (this.unlistenProgress) return;

    const unlisten = await listen<ImportProgressEvent>('import-progress', (event) => {
      log.debug('Import progress', event.payload);
      this.progressListeners.forEach((callback) => callback(event.payload));
    });

    this.unlistenProgress = unlisten;
  }

  private teardownProgressListener(): void {
    if (this.unlistenProgress) {
      this.unlistenProgress();
      this.unlistenProgress = null;
    }
  }
}

// Singleton instance
export const importService = new ImportService();
