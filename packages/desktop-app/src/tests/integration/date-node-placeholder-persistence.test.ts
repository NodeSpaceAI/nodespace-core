/**
 * Integration tests for date node placeholder persistence
 *
 * Tests the full flow of creating text nodes under date node parents:
 * 1. Frontend creates placeholder (empty text node) when loading empty date
 * 2. Placeholder does NOT persist immediately (stays in-memory only)
 * 3. User types content → debouncer triggers persistence
 * 4. Backend auto-creates date node parent (if doesn't exist)
 * 5. Backend creates text node successfully
 *
 * This prevents the bug where placeholders were marked as database-sourced
 * and tried to UPDATE instead of CREATE, causing NodeNotFound errors.
 *
 * Related to Issue #208 Wave 2 manual testing
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import {
  cleanDatabase,
  waitForDatabaseWrites
} from '../utils/test-database';
import {
  initializeDatabaseIfNeeded,
  cleanupDatabaseIfNeeded,
  shouldUseDatabase
} from '../utils/should-use-database';
import { checkServerHealth } from '../utils/test-node-helpers';
import { TestNodeBuilder } from '../utils/test-node-builder';
import { getBackendAdapter, HttpAdapter } from '$lib/services/backend-adapter';
import type { BackendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store';

describe.sequential('Date Node Placeholder Persistence', () => {
  let dbPath: string | null;
  let backend: BackendAdapter;

  beforeAll(async () => {
    if (shouldUseDatabase()) {
      await checkServerHealth(new HttpAdapter('http://localhost:3001'));
    }
    backend = getBackendAdapter();
  });

  afterAll(async () => {
    await cleanupDatabaseIfNeeded(dbPath);
  });

  beforeEach(async () => {
    // Initialize database if needed
    dbPath = await initializeDatabaseIfNeeded('date-node-placeholder-persistence');

    // Clean database and stores before each test
    await cleanDatabase(backend);
    sharedNodeStore.__resetForTesting();
  });

  it('should auto-create date node when creating text node with non-existent date parent', async () => {
    // Create a text node with date node parent that doesn't exist
    const textNode = TestNodeBuilder.text('Hello from today')
      .withParent('2025-10-13')
      .withContainer('2025-10-13')
      .build();

    // Should succeed - backend should auto-create date node
    const textNodeId = await backend.createNode(textNode);
    expect(textNodeId).toBeTruthy();

    // Wait for writes to complete
    await waitForDatabaseWrites();

    // Verify text node was created
    const retrievedText = await backend.getNode(textNodeId);
    expect(retrievedText).toBeTruthy();
    expect(retrievedText?.nodeType).toBe('text');
    expect(retrievedText?.content).toBe('Hello from today');
    expect(retrievedText?.parentId).toBe('2025-10-13');

    // Verify date node was auto-created by backend
    const retrievedDate = await backend.getNode('2025-10-13');
    expect(retrievedDate).toBeTruthy();
    expect(retrievedDate?.nodeType).toBe('date');
    expect(retrievedDate?.id).toBe('2025-10-13');
    expect(retrievedDate?.parentId).toBeNull();
    expect(retrievedDate?.content).toBe(''); // Date nodes have empty content
  });

  it('should not persist placeholder nodes (empty text nodes) immediately', async () => {
    // Simulate what happens when user opens an empty date view
    const dateId = '2025-10-13';

    // Create placeholder (empty text node) as viewer source
    const placeholderId = globalThis.crypto.randomUUID();
    const placeholder = TestNodeBuilder.text('')
      .withId(placeholderId)
      .withParent(dateId)
      .withContainer(dateId)
      .build();

    // Add to store with viewer source (simulates BaseNodeViewer behavior)
    const viewerSource = {
      type: 'viewer' as const,
      viewerId: 'test-viewer',
      reason: 'placeholder-creation'
    };
    const fullPlaceholder = {
      ...placeholder,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };
    sharedNodeStore.setNode(fullPlaceholder, viewerSource);

    // Wait a bit to ensure any persistence would have happened
    await new Promise((resolve) => setTimeout(resolve, 100));

    // Verify placeholder is NOT in database
    const retrievedPlaceholder = await backend.getNode(placeholderId);
    expect(retrievedPlaceholder).toBeNull();

    // Verify placeholder IS in memory
    const inMemory = sharedNodeStore.getNode(placeholderId);
    expect(inMemory).toBeTruthy();
    expect(inMemory?.content).toBe('');

    // Verify it's not marked as persisted
    expect(sharedNodeStore.isNodePersisted(placeholderId)).toBe(false);
  });

  it('should persist node with content when date parent auto-created', async () => {
    // Simulates the full flow: create text node under non-existent date
    const dateId = '2025-10-13';
    const textNode = TestNodeBuilder.text('Hello from today')
      .withParent(dateId)
      .withContainer(dateId)
      .build();

    // Create via backend (simulates what happens after debouncer fires)
    const textNodeId = await backend.createNode(textNode);

    await waitForDatabaseWrites();

    // Verify text node persisted
    const retrievedText = await backend.getNode(textNodeId);
    expect(retrievedText?.content).toBe('Hello from today');

    // Verify date node was auto-created
    const retrievedDate = await backend.getNode(dateId);
    expect(retrievedDate?.nodeType).toBe('date');
  });
});
