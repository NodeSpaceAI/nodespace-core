/**
 * TestNodeBuilder - Fluent API for creating test nodes
 *
 * Provides a convenient builder pattern for creating nodes in tests,
 * with sensible defaults and easy customization.
 *
 * # Usage
 *
 * ```typescript
 * import { TestNodeBuilder } from './test-node-builder';
 *
 * const node = new TestNodeBuilder()
 *   .withId('test-1')
 *   .withType('text')
 *   .withContent('Hello World')
 *   .withParent('parent-1')
 *   .build();
 * ```
 */

import { v4 as uuidv4 } from 'uuid';
import type { Node } from '$lib/types';

export class TestNodeBuilder {
  private node: Partial<Node> = {
    nodeType: 'text',
    content: '',
    parentId: null,
    containerNodeId: null,
    beforeSiblingId: null,
    properties: {},
    embeddingVector: null,
    mentions: []
  };

  /**
   * Set node ID (defaults to random UUID if not set)
   */
  withId(id: string): this {
    this.node.id = id;
    return this;
  }

  /**
   * Set node type
   */
  withType(nodeType: 'text' | 'task' | 'date'): this {
    this.node.nodeType = nodeType;
    return this;
  }

  /**
   * Set node content
   */
  withContent(content: string): this {
    this.node.content = content;
    return this;
  }

  /**
   * Set parent ID
   */
  withParent(parentId: string | null): this {
    this.node.parentId = parentId;
    return this;
  }

  /**
   * Set container node ID
   */
  withContainer(containerNodeId: string | null): this {
    this.node.containerNodeId = containerNodeId;
    return this;
  }

  /**
   * Set before sibling ID
   */
  withBeforeSibling(beforeSiblingId: string | null): this {
    this.node.beforeSiblingId = beforeSiblingId;
    return this;
  }

  /**
   * Set properties
   */
  withProperties(properties: Record<string, unknown>): this {
    this.node.properties = properties;
    return this;
  }

  /**
   * Add a single property
   */
  withProperty(key: string, value: unknown): this {
    if (!this.node.properties) {
      this.node.properties = {};
    }
    (this.node.properties as Record<string, unknown>)[key] = value;
    return this;
  }

  /**
   * Set embedding vector
   */
  withEmbedding(embedding: number[] | null): this {
    this.node.embeddingVector = embedding;
    return this;
  }

  /**
   * Set mentions
   */
  withMentions(mentions: string[]): this {
    this.node.mentions = mentions;
    return this;
  }

  /**
   * Build a text node with defaults
   */
  static text(content: string): TestNodeBuilder {
    return new TestNodeBuilder().withType('text').withContent(content);
  }

  /**
   * Build a task node with defaults
   */
  static task(content: string): TestNodeBuilder {
    return new TestNodeBuilder().withType('task').withContent(content);
  }

  /**
   * Build a date node with defaults
   */
  static date(dateStr: string): TestNodeBuilder {
    return new TestNodeBuilder().withType('date').withId(dateStr).withContent(`Date: ${dateStr}`);
  }

  /**
   * Build a container node (root node with containerNodeId = null)
   */
  static container(content: string): TestNodeBuilder {
    return new TestNodeBuilder()
      .withType('text')
      .withContent(content)
      .withParent(null)
      .withContainer(null)
      .withBeforeSibling(null);
  }

  /**
   * Build the node
   *
   * Note: This returns a node WITHOUT timestamps (createdAt, modifiedAt).
   * The backend will add timestamps automatically when creating the node.
   */
  build(): Omit<Node, 'createdAt' | 'modifiedAt'> {
    // Generate ID if not set
    if (!this.node.id) {
      this.node.id = uuidv4();
    }

    // Ensure all required fields are present
    if (!this.node.nodeType) {
      throw new Error('Node type is required');
    }

    if (this.node.content === undefined) {
      throw new Error('Node content is required');
    }

    return {
      id: this.node.id,
      nodeType: this.node.nodeType,
      content: this.node.content,
      parentId: this.node.parentId ?? null,
      containerNodeId: this.node.containerNodeId ?? null,
      beforeSiblingId: this.node.beforeSiblingId ?? null,
      properties: this.node.properties ?? {},
      embeddingVector: this.node.embeddingVector ?? null,
      mentions: this.node.mentions ?? []
    };
  }

  /**
   * Build and return the node with temporary timestamps (for testing in-memory scenarios)
   *
   * Note: When creating nodes via backend, use build() instead as the backend
   * will generate timestamps automatically.
   */
  buildWithTimestamps(): Node {
    const nodeWithoutTimestamps = this.build();
    const now = new Date().toISOString();

    return {
      ...nodeWithoutTimestamps,
      createdAt: now,
      modifiedAt: now
    };
  }
}
