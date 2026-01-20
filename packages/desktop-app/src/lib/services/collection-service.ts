/**
 * Collection Service
 *
 * Provides frontend access to collection operations via Tauri commands.
 * Collections are a flexible organizational structure for grouping nodes.
 *
 * ## Collection System Overview
 *
 * - Collections use colon-separated paths (e.g., "hr:policy:vacation")
 * - Nodes can belong to multiple collections (many-to-many)
 * - Collections are flat (globally unique names)
 * - Path syntax is for navigation convenience, not structural hierarchy
 */

import type { Node, CollectionNode } from '$lib/types';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('CollectionService');

// ============================================================================
// Types
// ============================================================================

/**
 * Collection with member count for UI display
 */
export interface CollectionInfo {
  /** The collection node data */
  id: string;
  content: string;
  nodeType: 'collection';
  createdAt: string;
  modifiedAt: string;
  version: number;
  properties: Record<string, unknown>;
  /** Number of direct members in this collection */
  memberCount: number;
}

/**
 * Collection member for sub-panel display
 */
export interface CollectionMember {
  id: string;
  name: string;
  nodeType: string;
}

// ============================================================================
// Environment Detection
// ============================================================================

function isTauriEnvironment(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

function isTestEnvironment(): boolean {
  return typeof process !== 'undefined' && process.env.NODE_ENV === 'test';
}

// ============================================================================
// Collection Service Interface
// ============================================================================

export interface CollectionServiceInterface {
  // Query operations
  getAllCollections(): Promise<CollectionInfo[]>;
  getCollectionMembers(collectionId: string): Promise<Node[]>;
  getCollectionMembersRecursive(collectionId: string): Promise<Node[]>;
  getNodeCollections(nodeId: string): Promise<string[]>;
  findCollectionByPath(path: string): Promise<CollectionNode | null>;
  getCollectionByName(name: string): Promise<CollectionNode | null>;

  // Mutation operations
  addNodeToCollection(nodeId: string, collectionId: string): Promise<void>;
  addNodeToCollectionPath(nodeId: string, path: string): Promise<string>;
  removeNodeFromCollection(nodeId: string, collectionId: string): Promise<void>;
  createCollection(name: string, description?: string): Promise<string>;
  renameCollection(collectionId: string, version: number, newName: string): Promise<CollectionNode>;
  deleteCollection(collectionId: string, version: number): Promise<void>;
}

// ============================================================================
// Tauri Collection Service Implementation
// ============================================================================

class TauriCollectionService implements CollectionServiceInterface {
  private _invoke: typeof import('@tauri-apps/api/core').invoke | null = null;

  private async getInvoke(): Promise<typeof import('@tauri-apps/api/core').invoke> {
    if (!this._invoke) {
      const tauriCore = await import('@tauri-apps/api/core');
      this._invoke = tauriCore.invoke;
    }
    return this._invoke;
  }

  async getAllCollections(): Promise<CollectionInfo[]> {
    const invoke = await this.getInvoke();
    log.debug('Fetching all collections');
    return invoke<CollectionInfo[]>('get_all_collections');
  }

  async getCollectionMembers(collectionId: string): Promise<Node[]> {
    const invoke = await this.getInvoke();
    log.debug('Fetching collection members', { collectionId });
    return invoke<Node[]>('get_collection_members', { collectionId });
  }

  async getCollectionMembersRecursive(collectionId: string): Promise<Node[]> {
    const invoke = await this.getInvoke();
    log.debug('Fetching recursive collection members', { collectionId });
    return invoke<Node[]>('get_collection_members_recursive', { collectionId });
  }

  async getNodeCollections(nodeId: string): Promise<string[]> {
    const invoke = await this.getInvoke();
    log.debug('Fetching node collections', { nodeId });
    return invoke<string[]>('get_node_collections', { nodeId });
  }

  async findCollectionByPath(path: string): Promise<CollectionNode | null> {
    const invoke = await this.getInvoke();
    log.debug('Finding collection by path', { path });
    return invoke<CollectionNode | null>('find_collection_by_path', { collectionPath: path });
  }

  async getCollectionByName(name: string): Promise<CollectionNode | null> {
    const invoke = await this.getInvoke();
    log.debug('Getting collection by name', { name });
    return invoke<CollectionNode | null>('get_collection_by_name', { name });
  }

  async addNodeToCollection(nodeId: string, collectionId: string): Promise<void> {
    const invoke = await this.getInvoke();
    log.debug('Adding node to collection', { nodeId, collectionId });
    return invoke<void>('add_node_to_collection', { nodeId, collectionId });
  }

  async addNodeToCollectionPath(nodeId: string, path: string): Promise<string> {
    const invoke = await this.getInvoke();
    log.debug('Adding node to collection path', { nodeId, path });
    return invoke<string>('add_node_to_collection_path', { nodeId, collectionPath: path });
  }

  async removeNodeFromCollection(nodeId: string, collectionId: string): Promise<void> {
    const invoke = await this.getInvoke();
    log.debug('Removing node from collection', { nodeId, collectionId });
    return invoke<void>('remove_node_from_collection', { nodeId, collectionId });
  }

  async createCollection(name: string, description?: string): Promise<string> {
    const invoke = await this.getInvoke();
    log.debug('Creating collection', { name, description });
    return invoke<string>('create_collection', { name, description });
  }

  async renameCollection(collectionId: string, version: number, newName: string): Promise<CollectionNode> {
    const invoke = await this.getInvoke();
    log.debug('Renaming collection', { collectionId, version, newName });
    return invoke<CollectionNode>('rename_collection', { collectionId, version, newName });
  }

  async deleteCollection(collectionId: string, version: number): Promise<void> {
    const invoke = await this.getInvoke();
    log.debug('Deleting collection', { collectionId, version });
    return invoke<void>('delete_collection', { collectionId, version });
  }
}

// ============================================================================
// Mock Collection Service (for tests)
// ============================================================================

class MockCollectionService implements CollectionServiceInterface {
  async getAllCollections(): Promise<CollectionInfo[]> {
    return [];
  }

  async getCollectionMembers(_collectionId: string): Promise<Node[]> {
    return [];
  }

  async getCollectionMembersRecursive(_collectionId: string): Promise<Node[]> {
    return [];
  }

  async getNodeCollections(_nodeId: string): Promise<string[]> {
    return [];
  }

  async findCollectionByPath(_path: string): Promise<CollectionNode | null> {
    return null;
  }

  async getCollectionByName(_name: string): Promise<CollectionNode | null> {
    return null;
  }

  async addNodeToCollection(_nodeId: string, _collectionId: string): Promise<void> {
    // No-op in mock
  }

  async addNodeToCollectionPath(_nodeId: string, _path: string): Promise<string> {
    return 'mock-collection-id';
  }

  async removeNodeFromCollection(_nodeId: string, _collectionId: string): Promise<void> {
    // No-op in mock
  }

  async createCollection(_name: string, _description?: string): Promise<string> {
    return 'mock-collection-id';
  }

  async renameCollection(collectionId: string, _version: number, newName: string): Promise<CollectionNode> {
    return {
      id: collectionId,
      nodeType: 'collection',
      content: newName,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  }

  async deleteCollection(_collectionId: string, _version: number): Promise<void> {
    // No-op in mock
  }
}

// ============================================================================
// Singleton Export
// ============================================================================

// ============================================================================
// HTTP Collection Service (for browser dev mode)
// ============================================================================

const DEV_PROXY_URL = 'http://localhost:3001';

class HttpCollectionService implements CollectionServiceInterface {
  async getAllCollections(): Promise<CollectionInfo[]> {
    log.debug('Fetching all collections via HTTP');
    const response = await fetch(`${DEV_PROXY_URL}/api/collections`);
    if (!response.ok) {
      throw new Error(`Failed to fetch collections: ${response.statusText}`);
    }
    return response.json();
  }

  async getCollectionMembers(collectionId: string): Promise<Node[]> {
    log.debug('Fetching collection members via HTTP', { collectionId });
    const response = await fetch(`${DEV_PROXY_URL}/api/collections/${encodeURIComponent(collectionId)}/members`);
    if (!response.ok) {
      throw new Error(`Failed to fetch collection members: ${response.statusText}`);
    }
    return response.json();
  }

  async getCollectionMembersRecursive(collectionId: string): Promise<Node[]> {
    // For now, just return direct members (recursive not implemented in dev-proxy)
    return this.getCollectionMembers(collectionId);
  }

  async getNodeCollections(_nodeId: string): Promise<string[]> {
    // Not implemented in dev-proxy yet
    return [];
  }

  async findCollectionByPath(_path: string): Promise<CollectionNode | null> {
    // Not implemented in dev-proxy yet
    return null;
  }

  async getCollectionByName(_name: string): Promise<CollectionNode | null> {
    // Not implemented in dev-proxy yet
    return null;
  }

  async addNodeToCollection(_nodeId: string, _collectionId: string): Promise<void> {
    // Not implemented in dev-proxy yet
  }

  async addNodeToCollectionPath(_nodeId: string, _path: string): Promise<string> {
    // Not implemented in dev-proxy yet
    return '';
  }

  async removeNodeFromCollection(_nodeId: string, _collectionId: string): Promise<void> {
    // Not implemented in dev-proxy yet
  }

  async createCollection(_name: string, _description?: string): Promise<string> {
    // Not implemented in dev-proxy yet
    return '';
  }

  async renameCollection(collectionId: string, _version: number, newName: string): Promise<CollectionNode> {
    // Not implemented in dev-proxy yet
    return {
      id: collectionId,
      nodeType: 'collection',
      content: newName,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  }

  async deleteCollection(_collectionId: string, _version: number): Promise<void> {
    // Not implemented in dev-proxy yet
  }
}

/**
 * Get the collection service instance based on runtime environment
 */
function getCollectionService(): CollectionServiceInterface {
  if (isTestEnvironment()) {
    return new MockCollectionService();
  }

  if (isTauriEnvironment()) {
    return new TauriCollectionService();
  }

  // For browser dev mode, use HTTP adapter
  log.info('Using HTTP collection service for browser dev mode');
  return new HttpCollectionService();
}

/** Singleton collection service instance */
export const collectionService = getCollectionService();
