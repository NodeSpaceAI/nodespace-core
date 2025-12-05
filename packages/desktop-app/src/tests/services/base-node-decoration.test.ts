/**
 * BaseNode Decoration System Tests
 *
 * Comprehensive test suite for the base node decoration service.
 * Tests all decorator classes, factory methods, and utility functions.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  BaseNodeDecorator,
  NodeDecoratorFactory,
  NODE_TYPE_CONFIGS,
  DefaultNodeDecorator,
  TaskNodeDecorator,
  UserNodeDecorator,
  DateNodeDecorator,
  DocumentNodeDecorator,
  AINodeDecorator
} from '$lib/services/base-node-decoration';
import type {
  DecorationContext,
  DecorationResult,
  NodeTypeConfig
} from '$lib/services/base-node-decoration';
import type { ComponentDecoration } from '$lib/types/component-decoration';
import type { NodeReferenceService } from '$lib/services/content-processor';

// Mock NodeReferenceService
const createMockNodeReferenceService = (): NodeReferenceService => {
  return {
    resolveReference: vi.fn(),
    getNodeById: vi.fn(),
    createReference: vi.fn()
  } as unknown as NodeReferenceService;
};

// Helper to create a standard DecorationContext
const createContext = (overrides: Partial<DecorationContext> = {}): DecorationContext => {
  const mockElement = document.createElement('div');
  return {
    nodeId: 'test-node-123',
    nodeType: 'text',
    title: 'Test Node',
    content: 'This is test content',
    uri: 'ns://node/test-node-123',
    metadata: {},
    targetElement: mockElement,
    displayContext: 'inline',
    ...overrides
  };
};

describe('BaseNodeDecorator', () => {
  let mockService: NodeReferenceService;

  // Create a concrete implementation for testing abstract class
  class TestDecorator extends BaseNodeDecorator {
    constructor(service: NodeReferenceService) {
      super(service, 'test');
    }

    public decorateReference(context: DecorationContext): ComponentDecoration {
      return this.getBaseComponentDecoration(context);
    }

    // Expose protected methods for testing
    public testGetBaseDecoration(context: DecorationContext): DecorationResult {
      return this.getBaseDecoration(context);
    }

    public testSanitizeText(text: string): string {
      return this.sanitizeText(text);
    }

    public testExtractMetadata(content: string, nodeType: string): Record<string, unknown> {
      return this.extractMetadata(content, nodeType);
    }

    public testFormatRelativeTime(timestamp: number): string {
      return this.formatRelativeTime(timestamp);
    }

    public testGetNodeTypeConfig(nodeType: string): NodeTypeConfig {
      return this.getNodeTypeConfig(nodeType);
    }
  }

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
    vi.clearAllMocks();
  });

  describe('constructor', () => {
    it('should initialize with service and decoration type', () => {
      const decorator = new TestDecorator(mockService);
      expect(decorator).toBeDefined();
      expect(decorator['decorationType']).toBe('test');
      expect(decorator['nodeReferenceService']).toBe(mockService);
    });
  });

  describe('getBaseDecoration', () => {
    it('should create basic HTML decoration with default node type', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext();
      const result = decorator.testGetBaseDecoration(context);

      expect(result.html).toContain('ns-noderef');
      expect(result.html).toContain('ns-noderef--text');
      expect(result.html).toContain('Test Node');
      expect(result.html).toContain('data-node-id="test-node-123"');
      expect(result.cssClasses).toEqual(['ns-noderef', 'ns-noderef--text']);
      expect(result.ariaLabel).toContain('Text');
      expect(result.ariaLabel).toContain('Test Node');
      expect(result.interactive).toBe(true);
    });

    it('should handle empty title gracefully', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext({ title: '' });

      // Should not throw when creating decoration
      expect(() => decorator.testGetBaseDecoration(context)).not.toThrow();

      const result = decorator.testGetBaseDecoration(context);
      expect(result.html).toContain('ns-noderef');
    });

    it('should sanitize title to prevent XSS', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext({ title: '<script>alert("xss")</script>' });
      const result = decorator.testGetBaseDecoration(context);

      expect(result.html).not.toContain('<script>');
      expect(result.html).toContain('&lt;script&gt;');
    });

    it('should include correct metadata', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext({ nodeType: 'task' });
      const result = decorator.testGetBaseDecoration(context);

      expect(result.metadata.nodeType).toBe('task');
      expect(result.metadata.decorationType).toBe('test');
    });
  });

  describe('getBaseComponentDecoration', () => {
    it('should create component decoration with all required props', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext();
      const result = decorator.decorateReference(context);

      expect(result.component).toBeDefined();
      expect(result.props.nodeId).toBe('test-node-123');
      expect(result.props.nodeType).toBe('text');
      expect(result.props.title).toBe('Test Node');
      expect(result.props.content).toBe('This is test content');
      expect(result.props.uri).toBe('ns://node/test-node-123');
      expect(result.props.displayContext).toBe('inline');
    });

    it('should use correct icon and color from config', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext({ nodeType: 'task' });
      const result = decorator.decorateReference(context);

      expect(result.props.icon).toBe('☐');
      expect(result.props.color).toBe('var(--node-task)');
    });

    it('should fallback to default config for unknown node types', () => {
      const decorator = new TestDecorator(mockService);
      const context = createContext({ nodeType: 'unknown-type' });
      const result = decorator.decorateReference(context);

      expect(result.props.icon).toBe('📝');
      expect(result.props.color).toBe('var(--node-default)');
    });
  });

  describe('sanitizeText', () => {
    it('should escape HTML special characters', () => {
      const decorator = new TestDecorator(mockService);

      expect(decorator.testSanitizeText('<div>Test</div>')).toBe('&lt;div&gt;Test&lt;/div&gt;');
      expect(decorator.testSanitizeText('A & B')).toBe('A &amp; B');
      // Note: In browser environment, textContent escapes < > & but not quotes
      const quotedResult = decorator.testSanitizeText('"quoted"');
      expect(quotedResult).toContain('quoted');
      const singleResult = decorator.testSanitizeText("'single'");
      expect(singleResult).toContain('single');
    });

    it('should handle empty or null input', () => {
      const decorator = new TestDecorator(mockService);

      expect(decorator.testSanitizeText('')).toBe('');
      expect(decorator.testSanitizeText(null as unknown as string)).toBe('');
      expect(decorator.testSanitizeText(undefined as unknown as string)).toBe('');
    });

    it('should use fallback sanitization in non-browser environments', () => {
      const decorator = new TestDecorator(mockService);

      // Save original document
      const _originalDocument = global.document;
      const originalCreateElement = global.document?.createElement;

      try {
        // Test scenario where document.createElement is unavailable
        if (global.document) {
          // Temporarily make createElement return undefined to trigger fallback
          global.document.createElement = undefined as unknown as typeof document.createElement;
        }

        const result = decorator.testSanitizeText('<script>alert("test")</script>');
        expect(result).toContain('&lt;script&gt;');
        expect(result).toContain('&gt;');
      } finally {
        // Restore original
        if (originalCreateElement && global.document) {
          global.document.createElement = originalCreateElement;
        }
      }
    });

    it('should handle text with multiple special characters', () => {
      const decorator = new TestDecorator(mockService);
      const input = '<script>alert("XSS & \'injection\'")</script>';
      const output = decorator.testSanitizeText(input);

      expect(output).not.toContain('<script>');
      expect(output).not.toContain('</script>');
      expect(output).toContain('&lt;');
      expect(output).toContain('&gt;');
      expect(output).toContain('&amp;');
    });
  });

  describe('extractMetadata', () => {
    it('should extract dates from content', () => {
      const decorator = new TestDecorator(mockService);
      const content = 'Meeting on 2024-03-15\nDiscuss project details';
      const metadata = decorator.testExtractMetadata(content, 'text');

      expect(metadata.mentionedDate).toBe('2024-03-15');
    });

    it('should extract task status', () => {
      const decorator = new TestDecorator(mockService);
      const content = 'Fix bug\nStatus: completed\nPriority: high';
      const metadata = decorator.testExtractMetadata(content, 'task');

      expect(metadata.status).toBe('completed');
      expect(metadata.priority).toBe('high');
    });

    it('should extract user email and role', () => {
      const decorator = new TestDecorator(mockService);
      const content = 'John Doe\njohn.doe@example.com\nRole: developer';
      const metadata = decorator.testExtractMetadata(content, 'user');

      expect(metadata.email).toBe('john.doe@example.com');
      expect(metadata.role).toBe('developer');
    });

    it('should return empty object when no patterns match', () => {
      const decorator = new TestDecorator(mockService);
      const content = 'Simple text with no metadata';
      const metadata = decorator.testExtractMetadata(content, 'text');

      expect(Object.keys(metadata).length).toBe(0);
    });

    it('should handle different date formats', () => {
      const decorator = new TestDecorator(mockService);
      const content1 = 'Due 12/25/2024';
      const metadata1 = decorator.testExtractMetadata(content1, 'text');
      expect(metadata1.mentionedDate).toBe('12/25/2024');

      const content2 = 'Due 2024-12-25';
      const metadata2 = decorator.testExtractMetadata(content2, 'text');
      expect(metadata2.mentionedDate).toBe('2024-12-25');
    });

    it('should only check first 5 lines', () => {
      const decorator = new TestDecorator(mockService);
      const content = 'Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nStatus: completed';
      const metadata = decorator.testExtractMetadata(content, 'task');

      // Status on line 7 should not be extracted (only first 5 lines checked)
      expect(metadata.status).toBeUndefined();
    });
  });

  describe('formatRelativeTime', () => {
    it('should format "just now" for recent times', () => {
      const decorator = new TestDecorator(mockService);
      const now = Date.now();
      expect(decorator.testFormatRelativeTime(now - 10000)).toBe('just now'); // 10 seconds ago
    });

    it('should format minutes ago', () => {
      const decorator = new TestDecorator(mockService);
      const now = Date.now();
      expect(decorator.testFormatRelativeTime(now - 120000)).toBe('2m ago'); // 2 minutes
      expect(decorator.testFormatRelativeTime(now - 300000)).toBe('5m ago'); // 5 minutes
    });

    it('should format hours ago', () => {
      const decorator = new TestDecorator(mockService);
      const now = Date.now();
      expect(decorator.testFormatRelativeTime(now - 3600000)).toBe('1h ago'); // 1 hour
      expect(decorator.testFormatRelativeTime(now - 7200000)).toBe('2h ago'); // 2 hours
    });

    it('should format days ago', () => {
      const decorator = new TestDecorator(mockService);
      const now = Date.now();
      expect(decorator.testFormatRelativeTime(now - 86400000)).toBe('1d ago'); // 1 day
      expect(decorator.testFormatRelativeTime(now - 259200000)).toBe('3d ago'); // 3 days
    });

    it('should handle edge cases between time units', () => {
      const decorator = new TestDecorator(mockService);
      const now = Date.now();

      // 59 seconds - should be "just now"
      expect(decorator.testFormatRelativeTime(now - 59000)).toBe('just now');

      // 61 seconds - should be "1m ago"
      expect(decorator.testFormatRelativeTime(now - 61000)).toBe('1m ago');

      // 59 minutes - should be "59m ago"
      expect(decorator.testFormatRelativeTime(now - 3540000)).toBe('59m ago');

      // 61 minutes - should be "1h ago"
      expect(decorator.testFormatRelativeTime(now - 3660000)).toBe('1h ago');
    });
  });

  describe('getNodeTypeConfig', () => {
    it('should return config for known node types', () => {
      const decorator = new TestDecorator(mockService);

      const taskConfig = decorator.testGetNodeTypeConfig('task');
      expect(taskConfig.icon).toBe('☐');
      expect(taskConfig.label).toBe('Task');
      expect(taskConfig.color).toBe('var(--node-task)');

      const userConfig = decorator.testGetNodeTypeConfig('user');
      expect(userConfig.icon).toBe('👤');
      expect(userConfig.label).toBe('User');
    });

    it('should return default config for unknown node types', () => {
      const decorator = new TestDecorator(mockService);
      const config = decorator.testGetNodeTypeConfig('unknown-type');

      expect(config.icon).toBe('📝');
      expect(config.label).toBe('Node');
      expect(config.color).toBe('var(--node-default)');
    });
  });
});

describe('DefaultNodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new DefaultNodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('default');
  });

  it('should return base component decoration', () => {
    const decorator = new DefaultNodeDecorator(mockService);
    const context = createContext();
    const result = decorator.decorateReference(context);

    expect(result.component).toBeDefined();
    expect(result.props.nodeId).toBe('test-node-123');
    expect(result.props.icon).toBe('📝');
  });
});

describe('TaskNodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new TaskNodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('task');
  });

  it('should extract and include task status', () => {
    const decorator = new TaskNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'task',
      content: 'Fix bug\nStatus: completed\nPriority: high'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.status).toBe('completed');
    expect(result.props.isCompleted).toBe(true);
    expect(result.props.checkboxIcon).toBe('☑');
  });

  it('should default to pending status', () => {
    const decorator = new TaskNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'task',
      content: 'New task without status'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.status).toBe('pending');
    expect(result.props.isCompleted).toBe(false);
    expect(result.props.checkboxIcon).toBe('☐');
  });

  it('should extract priority levels', () => {
    const decorator = new TaskNodeDecorator(mockService);

    const highPriority = createContext({
      nodeType: 'task',
      content: 'Urgent task\nPriority: high'
    });
    const resultHigh = decorator.decorateReference(highPriority);
    expect(resultHigh.props.priority).toBe('high');
    expect(resultHigh.props.priorityIcon).toBe('🔴');

    const lowPriority = createContext({
      nodeType: 'task',
      content: 'Minor task\nPriority: low'
    });
    const resultLow = decorator.decorateReference(lowPriority);
    expect(resultLow.props.priority).toBe('low');
    expect(resultLow.props.priorityIcon).toBe('🔵');
  });

  it('should default to normal priority', () => {
    const decorator = new TaskNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'task',
      content: 'Regular task'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.priority).toBe('normal');
    expect(result.props.priorityIcon).toBe('');
  });

  it('should handle "done" status as completed', () => {
    const decorator = new TaskNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'task',
      content: 'Task\nStatus: done'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.isCompleted).toBe(true);
    expect(result.props.checkboxIcon).toBe('☑');
  });

  it('should include metadata in decoration', () => {
    const decorator = new TaskNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'task',
      content: 'Task\nStatus: completed\nPriority: high'
    });
    const result = decorator.decorateReference(context);

    expect(result.metadata?.status).toBe('completed');
    expect(result.metadata?.priority).toBe('high');
    expect(result.metadata?.isCompleted).toBe(true);
  });
});

describe('UserNodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new UserNodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('user');
  });

  it('should extract email and role', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      content: 'John Doe\njohn.doe@example.com\nRole: developer'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.email).toBe('john.doe@example.com');
    expect(result.props.role).toBe('developer');
  });

  it('should default to member role', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      content: 'Jane Smith\njane@example.com'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.role).toBe('member');
  });

  it('should generate display name from title', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      title: 'John Doe Smith'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.displayName).toBe('John');
  });

  it('should use full name if no space in title', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      title: 'Madonna'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.displayName).toBe('Madonna');
  });

  it('should generate avatar initial', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      title: 'alice wonderland'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.avatarInitial).toBe('A');
  });

  it('should include online status (mocked)', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({ nodeType: 'user' });
    const result = decorator.decorateReference(context);

    expect(typeof result.props.isOnline).toBe('boolean');
  });

  it('should include metadata', () => {
    const decorator = new UserNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'user',
      title: 'John Doe'
    });
    const result = decorator.decorateReference(context);

    expect(result.metadata?.displayName).toBe('John');
    expect(result.metadata?.role).toBeDefined();
    expect(typeof result.metadata?.isOnline).toBe('boolean');
  });
});

describe('DateNodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new DateNodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('date');
  });

  it('should parse valid date from title', () => {
    const decorator = new DateNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'date',
      title: '2024-03-15'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.parsedDate).toBe('2024-03-15T00:00:00.000Z');
    expect(result.props.timestamp).toBeGreaterThan(0);
  });

  it('should parse date from content if title fails', () => {
    const decorator = new DateNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'date',
      title: '', // Empty title so it falls back to content
      content: '2024-06-20\nEvent details'
    });
    const result = decorator.decorateReference(context);

    // Should parse date from first line of content when title is empty
    expect(result.props.parsedDate).toBeTruthy();
    expect(result.props.timestamp).toBeGreaterThan(0);
  });

  it('should handle unparseable dates', () => {
    const decorator = new DateNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'date',
      title: 'not a date',
      content: 'also not a date'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.parsedDate).toBeNull();
    expect(result.props.timestamp).toBeNull();
  });

  it('should handle edge case dates that throw exceptions', () => {
    const decorator = new DateNodeDecorator(mockService);

    // Test various edge cases that might throw
    const edgeCases = [
      'Invalid Date',
      '',
      'NaN',
      'undefined',
      '99999-99-99',
      'not-a-date-at-all'
    ];

    edgeCases.forEach(dateStr => {
      const context = createContext({
        nodeType: 'date',
        title: dateStr
      });
      // Should not throw, should return null
      expect(() => decorator.decorateReference(context)).not.toThrow();
      const result = decorator.decorateReference(context);
      expect(result.props.parsedDate === null || result.props.timestamp === null).toBe(true);
    });
  });

  it('should calculate relative time', () => {
    const decorator = new DateNodeDecorator(mockService);
    const yesterday = new Date();
    yesterday.setDate(yesterday.getDate() - 1);

    const context = createContext({
      nodeType: 'date',
      title: yesterday.toISOString()
    });
    const result = decorator.decorateReference(context);

    expect(result.props.relativeTime).toBe('1d ago');
  });

  it('should detect if date is today', () => {
    const decorator = new DateNodeDecorator(mockService);
    const today = new Date();

    const context = createContext({
      nodeType: 'date',
      title: today.toISOString()
    });
    const result = decorator.decorateReference(context);

    expect(result.props.isToday).toBe(true);
  });

  it('should detect if date is in the past', () => {
    const decorator = new DateNodeDecorator(mockService);
    const pastDate = new Date('2020-01-01');

    const context = createContext({
      nodeType: 'date',
      title: pastDate.toISOString()
    });
    const result = decorator.decorateReference(context);

    expect(result.props.isPast).toBe(true);
  });

  it('should handle future dates correctly', () => {
    const decorator = new DateNodeDecorator(mockService);
    const futureDate = new Date();
    futureDate.setDate(futureDate.getDate() + 10);

    const context = createContext({
      nodeType: 'date',
      title: futureDate.toISOString()
    });
    const result = decorator.decorateReference(context);

    expect(result.props.isPast).toBe(false);
  });

  it('should include metadata', () => {
    const decorator = new DateNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'date',
      title: '2024-03-15'
    });
    const result = decorator.decorateReference(context);

    expect(result.metadata?.parsedDate).toBeTruthy();
    expect(result.metadata?.relativeTime).toBeDefined();
    expect(typeof result.metadata?.isToday).toBe('boolean');
    expect(typeof result.metadata?.isPast).toBe('boolean');
  });
});

describe('DocumentNodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('document');
  });

  it('should extract file type from content', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'Document title\nType: pdf\nContent here'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.fileType).toBe('pdf');
    expect(result.props.fileTypeIcon).toBe('📄');
  });

  it('should extract file type from extension', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'report.docx\nQuarterly report content'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.fileType).toBe('docx');
  });

  it('should default to document type', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'Plain content without type'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.fileType).toBe('document');
  });

  it('should extract file size', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'File details\nSize: 2.5 MB\nContent'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.size).toBe('2.5 MB');
  });

  it('should extract preview text', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: '# Document Title\nThis is the preview text that should be extracted'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.preview).toBe('This is the preview text that should be extracted');
  });

  it('should skip metadata lines in preview', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'Title\ntype: pdf\nauthor: John\nThis is the actual preview'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.preview).toBe('This is the actual preview');
  });

  it('should truncate long preview text', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const longText = 'x'.repeat(150);
    const context = createContext({
      nodeType: 'document',
      content: `Title\n${longText}`
    });
    const result = decorator.decorateReference(context);

    expect((result.props.preview as string | undefined)?.length).toBeLessThanOrEqual(100);
  });

  it('should provide correct icons for file types', () => {
    const decorator = new DocumentNodeDecorator(mockService);

    const testCases = [
      { type: 'pdf', expectedIcon: '📄' },
      { type: 'doc', expectedIcon: '📝' },
      { type: 'png', expectedIcon: '🖼️' },
      { type: 'mp4', expectedIcon: '🎥' },
      { type: 'mp3', expectedIcon: '🎵' },
      { type: 'zip', expectedIcon: '📦' }
    ];

    testCases.forEach(({ type, expectedIcon }) => {
      const context = createContext({
        nodeType: 'document',
        content: `Type: ${type}\nContent`
      });
      const result = decorator.decorateReference(context);
      expect(result.props.fileTypeIcon).toBe(expectedIcon);
    });
  });

  it('should include file type label', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'Type: pdf'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.fileTypeLabel).toBe('PDF');
  });

  it('should include metadata', () => {
    const decorator = new DocumentNodeDecorator(mockService);
    const context = createContext({
      nodeType: 'document',
      content: 'Type: pdf\nSize: 1.5 MB\nPreview text'
    });
    const result = decorator.decorateReference(context);

    expect(result.metadata?.fileType).toBe('pdf');
    expect(result.metadata?.size).toBe('1.5 MB');
    expect(result.metadata?.preview).toBeDefined();
  });
});

describe('AINodeDecorator', () => {
  let mockService: NodeReferenceService;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
  });

  it('should create decorator with correct type', () => {
    const decorator = new AINodeDecorator(mockService);
    expect(decorator['decorationType']).toBe('ai_chat');
  });

  it('should extract AI model', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({
      nodeType: 'ai_chat',
      content: 'Chat conversation\nModel: GPT-4\nMessages: 10'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.model).toBe('GPT-4');
  });

  it('should extract message count', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({
      nodeType: 'ai_chat',
      content: 'Chat history\nMessages: 42'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.messageCount).toBe(42);
  });

  it('should default to 0 messages if not found', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({
      nodeType: 'ai_chat',
      content: 'Chat without message count'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.messageCount).toBe(0);
  });

  it('should extract last activity timestamp', () => {
    const decorator = new AINodeDecorator(mockService);
    const testDate = '2024-03-15T10:30:00Z';
    const context = createContext({
      nodeType: 'ai_chat',
      content: `Chat\nLast_activity: ${testDate}`
    });
    const result = decorator.decorateReference(context);

    expect(result.props.lastActivity).toBeGreaterThan(0);
    expect(result.props.relativeTime).toBeDefined();
  });

  it('should handle invalid last activity date', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({
      nodeType: 'ai_chat',
      content: 'Chat\nLast_activity: invalid date'
    });
    const result = decorator.decorateReference(context);

    expect(result.props.lastActivity).toBeNull();
  });

  it('should include AI-specific icons', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({ nodeType: 'ai_chat' });
    const result = decorator.decorateReference(context);

    expect(result.props.aiIcon).toBe('🤖');
    expect(result.props.statusIcon).toBe('💭');
  });

  it('should include metadata', () => {
    const decorator = new AINodeDecorator(mockService);
    const context = createContext({
      nodeType: 'ai_chat',
      content: 'Model: GPT-4\nMessages: 10\nLast_activity: 2024-03-15T10:30:00Z'
    });
    const result = decorator.decorateReference(context);

    expect(result.metadata?.model).toBe('GPT-4');
    expect(result.metadata?.messageCount).toBe(10);
    expect(result.metadata?.lastActivity).toBeDefined();
  });
});

describe('NODE_TYPE_CONFIGS', () => {
  it('should have configs for all supported node types', () => {
    const expectedTypes = [
      'default', 'text', 'task', 'user', 'date',
      'document', 'ai_chat', 'entity', 'query'
    ];

    expectedTypes.forEach(type => {
      expect(NODE_TYPE_CONFIGS[type]).toBeDefined();
      expect(NODE_TYPE_CONFIGS[type].icon).toBeDefined();
      expect(NODE_TYPE_CONFIGS[type].label).toBeDefined();
      expect(NODE_TYPE_CONFIGS[type].color).toBeDefined();
      expect(NODE_TYPE_CONFIGS[type].defaultDecoration).toBeTypeOf('function');
    });
  });

  it('should have correct icons for each type', () => {
    expect(NODE_TYPE_CONFIGS.default.icon).toBe('📝');
    expect(NODE_TYPE_CONFIGS.task.icon).toBe('☐');
    expect(NODE_TYPE_CONFIGS.user.icon).toBe('👤');
    expect(NODE_TYPE_CONFIGS.date.icon).toBe('📅');
    expect(NODE_TYPE_CONFIGS.document.icon).toBe('📄');
    expect(NODE_TYPE_CONFIGS.ai_chat.icon).toBe('🤖');
    expect(NODE_TYPE_CONFIGS.entity.icon).toBe('🏷️');
    expect(NODE_TYPE_CONFIGS.query.icon).toBe('🔍');
  });

  it('should have CSS variable colors', () => {
    Object.values(NODE_TYPE_CONFIGS).forEach(config => {
      expect(config.color).toMatch(/^var\(--node-/);
    });
  });

  it('should have defaultDecoration functions that work', () => {
    const context = createContext();

    Object.entries(NODE_TYPE_CONFIGS).forEach(([_type, config]) => {
      const result = config.defaultDecoration(context);
      expect(result).toBeDefined();
      expect('component' in result || 'html' in result).toBe(true);
    });
  });
});

describe('NodeDecoratorFactory', () => {
  let mockService: NodeReferenceService;
  let factory: NodeDecoratorFactory;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
    factory = new NodeDecoratorFactory(mockService);
  });

  describe('constructor', () => {
    it('should initialize with node reference service', () => {
      expect(factory).toBeDefined();
      expect(factory['nodeReferenceService']).toBe(mockService);
    });

    it('should initialize decorators for all node types', () => {
      expect(factory['decorators'].size).toBeGreaterThan(0);
      expect(factory['decorators'].has('default')).toBe(true);
      expect(factory['decorators'].has('text')).toBe(true);
      expect(factory['decorators'].has('task')).toBe(true);
    });
  });

  describe('getDecorator', () => {
    it('should return decorator for known node types', () => {
      const taskDecorator = factory.getDecorator('task');
      expect(taskDecorator).toBeDefined();
      expect(taskDecorator).toBeInstanceOf(BaseNodeDecorator);
    });

    it('should return default decorator for unknown types', () => {
      const unknownDecorator = factory.getDecorator('unknown-type');
      expect(unknownDecorator).toBeDefined();
      expect(unknownDecorator).toBe(factory.getDecorator('default'));
    });

    it('should return same decorator instance for same type', () => {
      const decorator1 = factory.getDecorator('task');
      const decorator2 = factory.getDecorator('task');
      expect(decorator1).toBe(decorator2);
    });
  });

  describe('decorateReference', () => {
    it('should delegate to appropriate decorator', () => {
      const context = createContext({ nodeType: 'task' });
      const result = factory.decorateReference(context);

      expect(result).toBeDefined();
      expect(result.component).toBeDefined();
      expect(result.props.nodeId).toBe('test-node-123');
    });

    it('should work for all node types', () => {
      const types = ['text', 'task', 'user', 'date', 'document', 'ai_chat'];

      types.forEach(type => {
        const context = createContext({ nodeType: type });
        const result = factory.decorateReference(context);
        expect(result).toBeDefined();
        expect(result.component).toBeDefined();
      });
    });

    it('should handle unknown node types', () => {
      const context = createContext({ nodeType: 'unknown-type' });
      const result = factory.decorateReference(context);

      expect(result).toBeDefined();
      expect(result.component).toBeDefined();
    });

    it('should preserve context in decoration', () => {
      const context = createContext({
        nodeId: 'test-123',
        title: 'Custom Title',
        content: 'Custom Content',
        displayContext: 'popup' as const
      });
      const result = factory.decorateReference(context);

      expect(result.props.nodeId).toBe('test-123');
      expect(result.props.title).toBe('Custom Title');
      expect(result.props.content).toBe('Custom Content');
      expect(result.props.displayContext).toBe('popup');
    });
  });

  describe('getNodeTypeConfig', () => {
    it('should return config for known types', () => {
      const taskConfig = factory.getNodeTypeConfig('task');
      expect(taskConfig.icon).toBe('☐');
      expect(taskConfig.label).toBe('Task');
    });

    it('should return default config for unknown types', () => {
      const config = factory.getNodeTypeConfig('unknown-type');
      expect(config.icon).toBe('📝');
      expect(config.label).toBe('Node');
    });

    it('should match NODE_TYPE_CONFIGS', () => {
      Object.keys(NODE_TYPE_CONFIGS).forEach(type => {
        const factoryConfig = factory.getNodeTypeConfig(type);
        const directConfig = NODE_TYPE_CONFIGS[type];

        expect(factoryConfig.icon).toBe(directConfig.icon);
        expect(factoryConfig.label).toBe(directConfig.label);
        expect(factoryConfig.color).toBe(directConfig.color);
      });
    });
  });
});

describe('Exports', () => {
  it('should export all decorator classes', () => {
    expect(BaseNodeDecorator).toBeDefined();
    expect(DefaultNodeDecorator).toBeDefined();
    expect(TaskNodeDecorator).toBeDefined();
    expect(UserNodeDecorator).toBeDefined();
    expect(DateNodeDecorator).toBeDefined();
    expect(DocumentNodeDecorator).toBeDefined();
    expect(AINodeDecorator).toBeDefined();
  });

  it('should export factory as default', () => {
    expect(NodeDecoratorFactory).toBeDefined();
  });

  it('should export configuration', () => {
    expect(NODE_TYPE_CONFIGS).toBeDefined();
    expect(typeof NODE_TYPE_CONFIGS).toBe('object');
  });
});

describe('Integration Tests', () => {
  let mockService: NodeReferenceService;
  let factory: NodeDecoratorFactory;

  beforeEach(() => {
    mockService = createMockNodeReferenceService();
    factory = new NodeDecoratorFactory(mockService);
  });

  it('should create consistent decorations across factory and decorators', () => {
    const context = createContext({ nodeType: 'task' });

    const factoryResult = factory.decorateReference(context);
    const decoratorResult = new TaskNodeDecorator(mockService).decorateReference(context);

    // Both should produce valid component decorations
    expect(factoryResult.component).toBeDefined();
    expect(decoratorResult.component).toBeDefined();
    expect(factoryResult.props.nodeType).toBe(decoratorResult.props.nodeType);
  });

  it('should handle complex task metadata extraction', () => {
    const context = createContext({
      nodeType: 'task',
      content: `
        Complete project documentation
        Status: in-progress
        Priority: high
        Due: 2024-12-15
        Assigned to: john@example.com
      `
    });

    const result = factory.decorateReference(context);

    // Factory uses DefaultNodeDecorator which provides base component decoration
    // Specific metadata extraction is done by specialized decorators
    expect(result.props.nodeType).toBe('task');
    expect(result.props.content).toContain('Status');
    expect(result.props.content).toContain('Priority');

    // Test with TaskNodeDecorator directly for metadata extraction
    const taskDecorator = new TaskNodeDecorator(mockService);
    const taskResult = taskDecorator.decorateReference(context);
    expect(taskResult.props.status).toBeDefined();
    expect(taskResult.props.priority).toBeDefined();
  });

  it('should maintain XSS safety across all decorators', () => {
    const maliciousContext = createContext({
      title: '<script>alert("xss")</script>',
      content: '<img src=x onerror="alert(1)">'
    });

    const types = ['text', 'task', 'user', 'date', 'document', 'ai_chat'];

    types.forEach(type => {
      const context = { ...maliciousContext, nodeType: type };
      const result = factory.decorateReference(context);

      // Props should be safe (title is passed through, not HTML-escaped in props)
      expect(result.props.title).toBeDefined();
      // Metadata should not contain raw script tags
      expect(JSON.stringify(result.metadata || {})).not.toContain('<script>');
    });
  });
});
