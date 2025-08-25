/* eslint-env browser */
/* global Document */

/**
 * BaseNodeDecoration Tests
 *
 * Comprehensive tests for the BaseNode decoration system including all node types,
 * decoration rendering, caching, and accessibility features.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  TaskNodeDecorator,
  UserNodeDecorator,
  DateNodeDecorator,
  DocumentNodeDecorator,
  AINodeDecorator,
  NodeDecoratorFactory,
  NODE_TYPE_CONFIGS,
  type DecorationContext
} from '$lib/services/BaseNodeDecoration';
import { NodeReferenceService } from '$lib/services/NodeReferenceService';
import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
import { HierarchyService } from '$lib/services/HierarchyService';
import { NodeOperationsService } from '$lib/services/NodeOperationsService';
import { MockDatabaseService } from '$lib/services/MockDatabaseService';

// ============================================================================
// Test Setup
// ============================================================================

describe('BaseNode Decoration System', () => {
  let nodeReferenceService: NodeReferenceService;
  let databaseService: MockDatabaseService;
  let nodeManager: EnhancedNodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;

  beforeEach(async () => {
    // Initialize services
    databaseService = new MockDatabaseService();
    nodeManager = new EnhancedNodeManager(databaseService);
    hierarchyService = new HierarchyService(nodeManager, databaseService);
    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      databaseService
    );
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService
    );

    // Mock DOM environment
    const createMockElement = (tagName: string) => {
      const element = {
        tagName: tagName.toUpperCase(),
        textContent: '',
        innerHTML: '',
        dataset: {}
      };

      // Mock textContent setter that updates innerHTML
      Object.defineProperty(element, 'textContent', {
        get() {
          return this._textContent || '';
        },
        set(value) {
          this._textContent = value;
          this.innerHTML = value ? String(value).replace(/</g, '&lt;').replace(/>/g, '&gt;') : '';
        }
      });

      return element;
    };

    (globalThis as unknown as { document: Partial<Document> }).document = {
      createElement: vi.fn(createMockElement)
    } as Partial<Document>;
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  // ========================================================================
  // Node Type Configuration Tests
  // ========================================================================

  describe('Node Type Configurations', () => {
    it('should have configurations for all expected node types', () => {
      const expectedTypes = [
        'text',
        'task',
        'user',
        'date',
        'document',
        'ai_chat',
        'entity',
        'query',
        'default'
      ];

      for (const type of expectedTypes) {
        expect(NODE_TYPE_CONFIGS[type]).toBeDefined();
        expect(NODE_TYPE_CONFIGS[type].icon).toBeTruthy();
        expect(NODE_TYPE_CONFIGS[type].label).toBeTruthy();
        expect(NODE_TYPE_CONFIGS[type].color).toBeTruthy();
        expect(NODE_TYPE_CONFIGS[type].defaultDecoration).toBeTypeOf('function');
      }
    });

    it('should provide fallback to default configuration', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);
      const decorator = factory.getDecorator('unknown-type');
      expect(decorator).toBeDefined();
    });
  });

  // ========================================================================
  // Task Node Decoration Tests
  // ========================================================================

  describe('TaskNodeDecorator', () => {
    let taskDecorator: TaskNodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      taskDecorator = new TaskNodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should create pending task decoration correctly', () => {
      const context: DecorationContext = {
        nodeId: 'task-1',
        nodeType: 'task',
        title: 'Complete project documentation',
        content:
          '# Complete project documentation\nstatus: pending\npriority: high\n\nDetailed task description',
        uri: 'nodespace://node/task-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = taskDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--task');
      expect(result.html).toContain('ns-noderef--task-pending');
      expect(result.html).toContain('☐'); // Pending checkbox
      expect(result.html).toContain('Complete project documentation');
      expect(result.cssClasses).toContain('ns-noderef--task-pending');
      expect(result.cssClasses).toContain('ns-noderef--priority-high');
      expect(result.ariaLabel).toContain(
        'Task: Complete project documentation (pending, high priority)'
      );
      expect(result.interactive).toBe(true);
      expect(result.metadata.status).toBe('pending');
      expect(result.metadata.priority).toBe('high');
    });

    it('should create completed task decoration correctly', () => {
      const context: DecorationContext = {
        nodeId: 'task-2',
        nodeType: 'task',
        title: 'Setup development environment',
        content:
          '# Setup development environment\nstatus: completed\npriority: normal\n\nTask is done',
        uri: 'nodespace://node/task-2',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = taskDecorator.decorateReference(context);

      expect(result.html).toContain('☑'); // Completed checkbox
      expect(result.html).toContain('ns-noderef--task-completed');
      expect(result.html).toContain('ns-noderef__title--completed');
      expect(result.cssClasses).toContain('ns-noderef--task-completed');
      expect(result.metadata.isCompleted).toBe(true);
    });

    it('should extract metadata from task content', () => {
      const context: DecorationContext = {
        nodeId: 'task-3',
        nodeType: 'task',
        title: 'Review pull request',
        content:
          'status: pending\npriority: low\ndue_date: 2024-12-31\n\nNeed to review the changes',
        uri: 'nodespace://node/task-3',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = taskDecorator.decorateReference(context);

      expect(result.metadata.status).toBe('pending');
      expect(result.metadata.priority).toBe('low');
      expect(result.html).toContain('2024-12-31');
    });
  });

  // ========================================================================
  // User Node Decoration Tests
  // ========================================================================

  describe('UserNodeDecorator', () => {
    let userDecorator: UserNodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      userDecorator = new UserNodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;

      // Mock Math.random for consistent online status
      vi.spyOn(Math, 'random').mockReturnValue(0.7); // > 0.5, so online
    });

    it('should create user decoration with avatar and status', () => {
      const context: DecorationContext = {
        nodeId: 'user-1',
        nodeType: 'user',
        title: 'Alice Johnson',
        content: '# Alice Johnson\nemail: alice@example.com\nrole: admin\n\nLead developer',
        uri: 'nodespace://node/user-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = userDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--user');
      expect(result.html).toContain('ns-noderef--user-online');
      expect(result.html).toContain('ns-noderef__avatar');
      expect(result.html).toContain('A'); // First letter of Alice
      expect(result.html).toContain('ns-noderef__status--online');
      expect(result.html).toContain('Alice'); // Display name
      expect(result.html).toContain('admin'); // Role
      expect(result.cssClasses).toContain('ns-noderef--user-online');
      expect(result.cssClasses).toContain('ns-noderef--role-admin');
      expect(result.ariaLabel).toContain('User: Alice Johnson (online, admin)');
      expect(result.metadata.isOnline).toBe(true);
      expect(result.metadata.role).toBe('admin');
      expect(result.metadata.displayName).toBe('Alice');
    });

    it('should handle users without roles', () => {
      const context: DecorationContext = {
        nodeId: 'user-2',
        nodeType: 'user',
        title: 'Bob Smith',
        content: '# Bob Smith\nemail: bob@example.com\n\nRegular team member',
        uri: 'nodespace://node/user-2',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = userDecorator.decorateReference(context);

      expect(result.html).toContain('Bob');
      expect(result.html).not.toMatch(/admin|moderator/);
      expect(result.metadata.role).toBe('member');
    });
  });

  // ========================================================================
  // Date Node Decoration Tests
  // ========================================================================

  describe('DateNodeDecorator', () => {
    let dateDecorator: DateNodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      dateDecorator = new DateNodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should create date decoration with calendar icon', () => {
      const context: DecorationContext = {
        nodeId: 'date-1',
        nodeType: 'date',
        title: '2024-12-31',
        content: '# Project Deadline\n2024-12-31\n\nFinal delivery date',
        uri: 'nodespace://node/date-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = dateDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--date');
      expect(result.html).toContain('📅'); // Calendar icon
      expect(result.html).toContain('2024-12-31');
      expect(result.ariaLabel).toContain('Date: 2024-12-31');
      expect(result.metadata.parsedDate).toBeDefined();
    });

    it('should identify today dates correctly', () => {
      const today = new Date().toISOString().split('T')[0]; // YYYY-MM-DD format

      const context: DecorationContext = {
        nodeId: 'date-today',
        nodeType: 'date',
        title: today,
        content: `# Today\n${today}\n\nToday's date`,
        uri: 'nodespace://node/date-today',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = dateDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--date-today');
      expect(result.html).toContain('Today');
      expect(result.cssClasses).toContain('ns-noderef--date-today');
      expect(result.metadata.isToday).toBe(true);
    });

    it('should handle invalid dates gracefully', () => {
      const context: DecorationContext = {
        nodeId: 'date-invalid',
        nodeType: 'date',
        title: 'Not a date',
        content: '# Not a date\nThis is not a valid date\n\nJust some text',
        uri: 'nodespace://node/date-invalid',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = dateDecorator.decorateReference(context);

      expect(result.html).toContain('Not a date');
      expect(result.metadata.parsedDate).toBeNull();
    });
  });

  // ========================================================================
  // Document Node Decoration Tests
  // ========================================================================

  describe('DocumentNodeDecorator', () => {
    let docDecorator: DocumentNodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      docDecorator = new DocumentNodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should create document decoration with file type info', () => {
      const context: DecorationContext = {
        nodeId: 'doc-1',
        nodeType: 'document',
        title: 'API Documentation',
        content:
          '# API Documentation\ntype: pdf\nsize: 2.4MB\n\nComprehensive API documentation for the platform.',
        uri: 'nodespace://node/doc-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = docDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--document');
      expect(result.html).toContain('ns-noderef--document-pdf');
      expect(result.html).toContain('📄'); // PDF icon
      expect(result.html).toContain('API Documentation');
      expect(result.html).toContain('PDF'); // File type
      expect(result.html).toContain('2.4MB'); // File size
      expect(result.html).toContain('Comprehensive API documentation'); // Preview
      expect(result.cssClasses).toContain('ns-noderef--document-pdf');
      expect(result.metadata.fileType).toBe('pdf');
      expect(result.metadata.size).toBe('2.4MB');
    });

    it('should handle different file types with appropriate icons', () => {
      const testCases = [
        { type: 'jpg', expectedIcon: '🖼️' },
        { type: 'mp4', expectedIcon: '🎥' },
        { type: 'mp3', expectedIcon: '🎵' },
        { type: 'zip', expectedIcon: '📦' }
      ];

      for (const testCase of testCases) {
        const context: DecorationContext = {
          nodeId: `doc-${testCase.type}`,
          nodeType: 'document',
          title: `Test ${testCase.type.toUpperCase()} File`,
          content: `# Test File\ntype: ${testCase.type}\n\nTest content`,
          uri: `nodespace://node/doc-${testCase.type}`,
          metadata: {},
          targetElement: mockElement,
          displayContext: 'inline'
        };

        const result = docDecorator.decorateReference(context);
        expect(result.html).toContain(testCase.expectedIcon);
        expect(result.metadata.fileType).toBe(testCase.type);
      }
    });
  });

  // ========================================================================
  // AI Chat Node Decoration Tests
  // ========================================================================

  describe('AINodeDecorator', () => {
    let aiDecorator: AINodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      aiDecorator = new AINodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should create AI chat decoration with model info', () => {
      const context: DecorationContext = {
        nodeId: 'ai-1',
        nodeType: 'ai_chat',
        title: 'Architecture Discussion',
        content:
          '# Architecture Discussion\nmodel: Claude 3.5 Sonnet\nmessages: 47\nlast_activity: 2024-08-21T14:30:00Z\n\nDiscussion about system design',
        uri: 'nodespace://node/ai-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = aiDecorator.decorateReference(context);

      expect(result.html).toContain('ns-noderef--ai-chat');
      expect(result.html).toContain('🤖'); // AI icon
      expect(result.html).toContain('Architecture Discussion');
      expect(result.html).toContain('Claude 3.5 Sonnet'); // Model
      expect(result.html).toContain('47 messages'); // Message count
      expect(result.html).toContain('💭'); // Status icon
      expect(result.metadata.model).toBe('Claude 3.5 Sonnet');
      expect(result.metadata.messageCount).toBe(47);
    });

    it('should handle AI chats without full metadata', () => {
      const context: DecorationContext = {
        nodeId: 'ai-2',
        nodeType: 'ai_chat',
        title: 'Quick Chat',
        content: '# Quick Chat\n\nJust a simple conversation',
        uri: 'nodespace://node/ai-2',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = aiDecorator.decorateReference(context);

      expect(result.html).toContain('Quick Chat');
      expect(result.metadata.model).toBe('');
      expect(result.metadata.messageCount).toBe(0);
    });
  });

  // ========================================================================
  // NodeDecoratorFactory Tests
  // ========================================================================

  describe('NodeDecoratorFactory', () => {
    it('should create appropriate decorators for each node type', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);

      expect(factory.getDecorator('task')).toBeInstanceOf(TaskNodeDecorator);
      expect(factory.getDecorator('user')).toBeInstanceOf(UserNodeDecorator);
      expect(factory.getDecorator('date')).toBeInstanceOf(DateNodeDecorator);
      expect(factory.getDecorator('document')).toBeInstanceOf(DocumentNodeDecorator);
      expect(factory.getDecorator('ai_chat')).toBeInstanceOf(AINodeDecorator);
    });

    it('should fall back to default decorator for unknown types', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);
      const decorator = factory.getDecorator('unknown-type');
      const defaultDecorator = factory.getDecorator('default');

      expect(decorator).toBeDefined();
      expect(decorator).toBe(defaultDecorator); // Should return the same default instance
    });

    it('should decorate references using the factory method', () => {
      const factory = new NodeDecoratorFactory(nodeReferenceService);
      const mockElement = { dataset: {} } as HTMLElement;

      const context: DecorationContext = {
        nodeId: 'test-1',
        nodeType: 'task',
        title: 'Test Task',
        content: '# Test Task\nstatus: pending\n\nTest content',
        uri: 'nodespace://node/test-1',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = factory.decorateReference(context);

      expect(result).toBeDefined();
      expect(result.html).toContain('ns-noderef--task');
      expect(result.cssClasses).toContain('ns-noderef--task');
      expect(result.interactive).toBe(true);
    });
  });

  // ========================================================================
  // XSS Protection Tests
  // ========================================================================

  describe('XSS Protection', () => {
    let baseDecorator: TaskNodeDecorator;
    let mockElement: HTMLElement;

    beforeEach(() => {
      baseDecorator = new TaskNodeDecorator(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should sanitize malicious content in titles', () => {
      const maliciousTitle = '<script>alert("xss")</script>Legitimate Title';

      const context: DecorationContext = {
        nodeId: 'xss-test',
        nodeType: 'task',
        title: maliciousTitle,
        content: '# Legitimate content\nstatus: pending',
        uri: 'nodespace://node/xss-test',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = baseDecorator.decorateReference(context);

      expect(result.html).not.toContain('<script>');
      expect(result.html).toContain('&lt;script&gt;'); // Should be escaped
      expect(result.html).toContain('Legitimate Title');
    });

    it('should sanitize content with HTML entities', () => {
      const maliciousContent = '# Task & <img src=x onerror=alert(1)> Content';

      const context: DecorationContext = {
        nodeId: 'xss-test-2',
        nodeType: 'task',
        title: 'Clean Title',
        content: maliciousContent,
        uri: 'nodespace://node/xss-test-2',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = baseDecorator.decorateReference(context);

      expect(result.html).not.toContain('<img');
      expect(result.html).not.toContain('onerror');
      expect(result.html).toContain('Clean Title');
    });
  });

  // ========================================================================
  // Accessibility Tests
  // ========================================================================

  describe('Accessibility Features', () => {
    let factory: NodeDecoratorFactory;
    let mockElement: HTMLElement;

    beforeEach(() => {
      factory = new NodeDecoratorFactory(nodeReferenceService);
      mockElement = { dataset: {} } as HTMLElement;
    });

    it('should provide meaningful ARIA labels for all node types', () => {
      const testCases = [
        { type: 'task', title: 'Test Task', expectedLabel: 'Task: Test Task' },
        { type: 'user', title: 'John Doe', expectedLabel: 'User: John Doe' },
        { type: 'date', title: '2024-12-31', expectedLabel: 'Date: 2024-12-31' },
        { type: 'document', title: 'Test Doc', expectedLabel: 'Document: Test Doc' },
        { type: 'ai_chat', title: 'AI Chat', expectedLabel: 'AI Chat: AI Chat' }
      ];

      for (const testCase of testCases) {
        const context: DecorationContext = {
          nodeId: `test-${testCase.type}`,
          nodeType: testCase.type,
          title: testCase.title,
          content: `# ${testCase.title}\n\nTest content`,
          uri: `nodespace://node/test-${testCase.type}`,
          metadata: {},
          targetElement: mockElement,
          displayContext: 'inline'
        };

        const result = factory.decorateReference(context);
        expect(result.ariaLabel).toContain(testCase.expectedLabel);
      }
    });

    it('should mark interactive decorations appropriately', () => {
      const context: DecorationContext = {
        nodeId: 'interactive-test',
        nodeType: 'task',
        title: 'Interactive Task',
        content: '# Interactive Task\nstatus: pending',
        uri: 'nodespace://node/interactive-test',
        metadata: {},
        targetElement: mockElement,
        displayContext: 'inline'
      };

      const result = factory.decorateReference(context);

      expect(result.interactive).toBe(true);
      expect(result.html).toContain('role="button"');
      expect(result.html).toContain('tabindex="0"');
    });
  });
});
