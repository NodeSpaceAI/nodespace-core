/**
 * Simple test utilities for NodeSpace components
 */
import { render } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import type { ComponentType } from 'svelte';

// Basic mock data types
export interface MockNodeData {
  id: string;
  type: 'text' | 'task' | 'ai-chat';
  content: string;
  createdAt: Date;
  updatedAt: Date;
}

// Simple mock data store
export class SimpleMockStore {
  private nodes = new Map<string, MockNodeData>();
  private static instance: SimpleMockStore | null = null;

  static getInstance(): SimpleMockStore {
    if (!SimpleMockStore.instance) {
      SimpleMockStore.instance = new SimpleMockStore();
    }
    return SimpleMockStore.instance;
  }

  static resetInstance(): void {
    SimpleMockStore.instance = null;
  }

  async save(node: Partial<MockNodeData>): Promise<string> {
    const id = node.id || `node-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
    const nodeData: MockNodeData = {
      ...node,
      id,
      createdAt: node.createdAt || new Date(),
      updatedAt: new Date()
    } as MockNodeData;
    
    this.nodes.set(id, nodeData);
    return id;
  }

  async load(id: string): Promise<MockNodeData | null> {
    return this.nodes.get(id) || null;
  }

  getAll(): MockNodeData[] {
    return Array.from(this.nodes.values());
  }

  clear(): void {
    this.nodes.clear();
  }
}

// Simple counter for unique test IDs
let testNodeCounter = 0;

// Test data factories
export function createTestNode(overrides: Partial<MockNodeData> = {}): MockNodeData {
  testNodeCounter++;
  return {
    id: `test-${Date.now()}-${testNodeCounter}`,
    type: 'text',
    content: 'Test content',
    createdAt: new Date(),
    updatedAt: new Date(),
    ...overrides
  };
}

// Enhanced render function with common utilities
export function renderWithContext(
  component: ComponentType,
  props: Record<string, unknown> = {}
) {
  const mockStore = SimpleMockStore.getInstance();
  mockStore.clear(); // Clean slate for each test

  const result = render(component, {
    props: { ...props, dataStore: mockStore }
  });

  return {
    ...result,
    user: userEvent.setup()
  };
}