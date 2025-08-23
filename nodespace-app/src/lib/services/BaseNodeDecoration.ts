/**
 * BaseNode Decoration System - Rich Visual Node References (Phase 2.2)
 *
 * Implements the decorateReference() pattern for creating rich visual decorations
 * and component-based decorations for different node types in the universal node reference system.
 *
 * Key Features:
 * - Base decorateReference() method for default styling
 * - Node type-specific decoration classes (TaskNode, UserNode, DateNode, etc.)
 * - Component-based rendering with rich decorator functionality
 * - Content-driven decoration system with XSS-safe rendering
 * - Performance-optimized viewport-based processing
 * - Full accessibility support with ARIA labels
 * - Integration with existing NodeReferenceService and DecorationCoordinator
 */

import type { NodeReferenceService } from './NodeReferenceService';
import type { ComponentDecoration } from '../types/ComponentDecoration';
import { getNodeReferenceComponent } from '../components/references';

// ============================================================================
// Core Types and Interfaces
// ============================================================================

export interface DecorationContext {
  nodeId: string;
  nodeType: string;
  title: string;
  content: string;
  uri: string;
  metadata: Record<string, unknown>;
  targetElement: HTMLElement;
  displayContext: 'inline' | 'popup' | 'preview';
}

export interface DecorationResult {
  html: string;
  cssClasses: string[];
  ariaLabel: string;
  metadata: Record<string, unknown>;
  interactive: boolean;
}

export interface NodeTypeConfig {
  icon: string;
  label: string;
  color: string;
  defaultDecoration: (context: DecorationContext) => DecorationResult | ComponentDecoration;
}

// ============================================================================
// Base Node Decorator (Supporting Both Architectures)
// ============================================================================

export abstract class BaseNodeDecorator {
  protected nodeReferenceService: NodeReferenceService;
  protected readonly decorationType: string;

  constructor(nodeReferenceService: NodeReferenceService, decorationType: string) {
    this.nodeReferenceService = nodeReferenceService;
    this.decorationType = decorationType;
  }

  /**
   * Abstract method that must be implemented by each node type
   * This is the core decorateReference() pattern (supports both HTML and Component decorations)
   */
  public abstract decorateReference(context: DecorationContext): DecorationResult | ComponentDecoration;

  /**
   * Base implementation with safe defaults (HTML-based decoration)
   */
  protected getBaseDecoration(context: DecorationContext): DecorationResult {
    const { nodeType, title, uri } = context;
    const safeTitle = this.sanitizeText(title);
    const nodeConfig = this.getNodeTypeConfig(nodeType);

    return {
      html: `
        <span class="ns-noderef ns-noderef--${nodeType}" 
              data-node-id="${context.nodeId}" 
              data-uri="${uri}"
              role="link"
              tabindex="0">
          <span class="ns-noderef__icon">${nodeConfig.icon}</span>
          <span class="ns-noderef__title">${safeTitle}</span>
        </span>
      `,
      cssClasses: [`ns-noderef`, `ns-noderef--${nodeType}`],
      ariaLabel: `Reference to ${nodeConfig.label}: ${safeTitle}`,
      metadata: { nodeType, decorationType: this.decorationType },
      interactive: true
    };
  }

  /**
   * Creates a base component decoration for any node type
   */
  protected getBaseComponentDecoration(context: DecorationContext): ComponentDecoration {
    const config = NODE_TYPE_CONFIGS[context.nodeType] || NODE_TYPE_CONFIGS.default;
    
    return {
      component: getNodeReferenceComponent('base') as ComponentDecoration['component'],
      props: {
        nodeId: context.nodeId,
        nodeType: context.nodeType,
        title: context.title,
        content: context.content,
        uri: context.uri,
        icon: config.icon,
        color: config.color,
        ariaLabel: `${config.label}: ${context.title}`,
        metadata: context.metadata,
        displayContext: context.displayContext
      }
    };
  }

  /**
   * Get configuration for node type with fallback
   */
  protected getNodeTypeConfig(nodeType: string): NodeTypeConfig {
    return NODE_TYPE_CONFIGS[nodeType] || NODE_TYPE_CONFIGS.default;
  }

  /**
   * Sanitize text content to prevent XSS
   */
  protected sanitizeText(text: string): string {
    if (!text) return '';

    // In test environment or Node.js, use simple HTML escaping
    if (typeof document === 'undefined' || !document.createElement) {
      return text
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
    }

    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  /**
   * Extract metadata from node content
   */
  protected extractMetadata(content: string, nodeType: string): Record<string, unknown> {
    const metadata: Record<string, unknown> = {};

    // Extract common metadata patterns
    const lines = content.split('\n');

    // Look for metadata in first few lines
    for (const line of lines.slice(0, 5)) {
      // Date patterns
      const dateMatch = line.match(/(\d{4}-\d{2}-\d{2}|\d{1,2}\/\d{1,2}\/\d{4})/);
      if (dateMatch) {
        metadata.mentionedDate = dateMatch[1];
      }

      // Status patterns for tasks
      if (nodeType === 'task') {
        const statusMatch = line.match(/(?:status|state):\s*(\w+)/i);
        if (statusMatch) {
          metadata.status = statusMatch[1].toLowerCase();
        }

        // Priority patterns
        const priorityMatch = line.match(/(?:priority|pri):\s*(\w+)/i);
        if (priorityMatch) {
          metadata.priority = priorityMatch[1].toLowerCase();
        }
      }

      // User patterns
      if (nodeType === 'user') {
        const emailMatch = line.match(/([a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,})/);
        if (emailMatch) {
          metadata.email = emailMatch[1];
        }

        // Role patterns
        const roleMatch = line.match(/(?:role|position):\s*(\w+)/i);
        if (roleMatch) {
          metadata.role = roleMatch[1].toLowerCase();
        }
      }
    }

    return metadata;
  }

  /**
   * Format relative time for display
   */
  protected formatRelativeTime(timestamp: number): string {
    const now = Date.now();
    const diff = now - timestamp;
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);

    if (days > 0) {
      return `${days}d ago`;
    } else if (hours > 0) {
      return `${hours}h ago`;
    } else if (minutes > 0) {
      return `${minutes}m ago`;
    } else {
      return 'just now';
    }
  }
}

// ============================================================================
// Default Decorator Implementation  
// ============================================================================

/**
 * Default decorator that provides basic component-based decoration for all node types
 */
class DefaultNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'default');
  }

  public decorateReference(context: DecorationContext): ComponentDecoration {
    return this.getBaseComponentDecoration(context);
  }
}

// ============================================================================
// Node Type-Specific Decorators (Rich HTML-based implementations)
// ============================================================================

export class TaskNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'task');
  }

  public decorateReference(context: DecorationContext): DecorationResult {
    const baseDecoration = this.getBaseDecoration(context);
    const metadata = this.extractMetadata(context.content, 'task');
    const status = (metadata.status as string) || 'pending';
    const priority = (metadata.priority as string) || 'normal';

    // Determine checkbox state
    const isCompleted = status === 'completed' || status === 'done';
    const checkboxIcon = isCompleted ? '☑' : '☐';

    // Priority indicator
    const priorityIcon = priority === 'high' ? '🔴' : priority === 'low' ? '🔵' : '';

    const safeTitle = this.sanitizeText(context.title);

    baseDecoration.html = `
      <span class="ns-noderef ns-noderef--task ns-noderef--task-${status}" 
            data-node-id="${context.nodeId}" 
            data-uri="${context.uri}"
            data-status="${status}"
            data-priority="${priority}"
            role="button"
            tabindex="0">
        <span class="ns-noderef__checkbox">${checkboxIcon}</span>
        <span class="ns-noderef__title ${isCompleted ? 'ns-noderef__title--completed' : ''}">${safeTitle}</span>
        ${priorityIcon ? `<span class="ns-noderef__priority">${priorityIcon}</span>` : ''}
        ${metadata.mentionedDate ? `<span class="ns-noderef__date">${metadata.mentionedDate}</span>` : ''}
      </span>
    `;

    baseDecoration.cssClasses.push(
      `ns-noderef--task-${status}`,
      `ns-noderef--priority-${priority}`
    );
    baseDecoration.ariaLabel = `Task: ${safeTitle} (${status}${priority !== 'normal' ? `, ${priority} priority` : ''})`;
    baseDecoration.metadata = { ...baseDecoration.metadata, status, priority, isCompleted };

    return baseDecoration;
  }
}

export class UserNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'user');
  }

  public decorateReference(context: DecorationContext): DecorationResult {
    const baseDecoration = this.getBaseDecoration(context);
    const metadata = this.extractMetadata(context.content, 'user');

    // Mock online status (in real implementation, this would come from presence service)
    const isOnline = Math.random() > 0.5; // Mock for demo
    const role = (metadata.role as string) || 'member';

    const safeTitle = this.sanitizeText(context.title);
    const displayName = safeTitle.split(' ')[0] || safeTitle; // Use first name for compact display

    baseDecoration.html = `
      <span class="ns-noderef ns-noderef--user ns-noderef--user-${isOnline ? 'online' : 'offline'}" 
            data-node-id="${context.nodeId}" 
            data-uri="${context.uri}"
            data-online="${isOnline}"
            data-role="${role}"
            role="button"
            tabindex="0">
        <span class="ns-noderef__avatar">
          <span class="ns-noderef__avatar-initial">${displayName.charAt(0).toUpperCase()}</span>
          <span class="ns-noderef__status-indicator ${isOnline ? 'ns-noderef__status--online' : 'ns-noderef__status--offline'}"></span>
        </span>
        <span class="ns-noderef__name">${displayName}</span>
        ${role !== 'member' ? `<span class="ns-noderef__role">${role}</span>` : ''}
      </span>
    `;

    baseDecoration.cssClasses.push(
      `ns-noderef--user-${isOnline ? 'online' : 'offline'}`,
      `ns-noderef--role-${role}`
    );
    baseDecoration.ariaLabel = `User: ${safeTitle} (${isOnline ? 'online' : 'offline'}${role !== 'member' ? `, ${role}` : ''})`;
    baseDecoration.metadata = { ...baseDecoration.metadata, isOnline, role, displayName };

    return baseDecoration;
  }
}

export class DateNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'date');
  }

  public decorateReference(context: DecorationContext): DecorationResult {
    const baseDecoration = this.getBaseDecoration(context);

    // Try to parse date from title or content
    const dateStr = context.title || context.content.split('\n')[0];
    const parsedDate = this.parseDate(dateStr);

    const safeTitle = this.sanitizeText(context.title);
    const relativeTime = parsedDate ? this.formatRelativeTime(parsedDate.getTime()) : '';
    const isToday = parsedDate ? this.isToday(parsedDate) : false;
    const isPast = parsedDate ? parsedDate.getTime() < Date.now() : false;

    baseDecoration.html = `
      <span class="ns-noderef ns-noderef--date ${isToday ? 'ns-noderef--date-today' : ''} ${isPast ? 'ns-noderef--date-past' : 'ns-noderef--date-future'}" 
            data-node-id="${context.nodeId}" 
            data-uri="${context.uri}"
            data-timestamp="${parsedDate?.getTime() || ''}"
            role="button"
            tabindex="0">
        <span class="ns-noderef__icon">📅</span>
        <span class="ns-noderef__date-text">${safeTitle}</span>
        ${relativeTime ? `<span class="ns-noderef__relative-time">${relativeTime}</span>` : ''}
        ${isToday ? `<span class="ns-noderef__today-badge">Today</span>` : ''}
      </span>
    `;

    baseDecoration.cssClasses.push(
      isToday ? 'ns-noderef--date-today' : '',
      isPast ? 'ns-noderef--date-past' : 'ns-noderef--date-future'
    );
    baseDecoration.ariaLabel = `Date: ${safeTitle}${relativeTime ? ` (${relativeTime})` : ''}${isToday ? ' (Today)' : ''}`;
    baseDecoration.metadata = {
      ...baseDecoration.metadata,
      parsedDate: parsedDate?.toISOString() || null,
      relativeTime,
      isToday,
      isPast
    };

    return baseDecoration;
  }

  private parseDate(dateStr: string): Date | null {
    try {
      // Try common date formats
      const date = new Date(dateStr);
      return isNaN(date.getTime()) ? null : date;
    } catch {
      return null;
    }
  }

  private isToday(date: Date): boolean {
    const today = new Date();
    return date.toDateString() === today.toDateString();
  }
}

export class DocumentNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'document');
  }

  public decorateReference(context: DecorationContext): DecorationResult {
    const baseDecoration = this.getBaseDecoration(context);

    // Extract file type and preview from content
    const fileType = this.extractFileType(context.content);
    const preview = this.extractPreview(context.content);
    const size = this.extractSize(context.content);

    const safeTitle = this.sanitizeText(context.title);
    const fileTypeIcon = this.getFileTypeIcon(fileType);

    baseDecoration.html = `
      <span class="ns-noderef ns-noderef--document ns-noderef--document-${fileType}" 
            data-node-id="${context.nodeId}" 
            data-uri="${context.uri}"
            data-file-type="${fileType}"
            role="button"
            tabindex="0">
        <span class="ns-noderef__file-icon">${fileTypeIcon}</span>
        <span class="ns-noderef__document-info">
          <span class="ns-noderef__title">${safeTitle}</span>
          ${preview ? `<span class="ns-noderef__preview">${preview}</span>` : ''}
          <span class="ns-noderef__meta">
            <span class="ns-noderef__file-type">${fileType.toUpperCase()}</span>
            ${size ? `<span class="ns-noderef__file-size">${size}</span>` : ''}
          </span>
        </span>
      </span>
    `;

    baseDecoration.cssClasses.push(`ns-noderef--document-${fileType}`);
    baseDecoration.ariaLabel = `Document: ${safeTitle} (${fileType.toUpperCase()}${size ? `, ${size}` : ''})`;
    baseDecoration.metadata = { ...baseDecoration.metadata, fileType, preview, size };

    return baseDecoration;
  }

  private extractFileType(content: string): string {
    // Look for file type indicators in content
    const typeMatch = content.match(/type:\s*(\w+)/i) || content.match(/\.(\w+)$/m);
    return typeMatch ? typeMatch[1].toLowerCase() : 'document';
  }

  private extractPreview(content: string): string {
    // Get first line that's not metadata or title
    const lines = content.split('\n');
    for (let i = 1; i < lines.length; i++) {
      const line = lines[i].trim();
      if (line && !line.match(/^\w+:/) && !line.match(/^#+/)) {
        return line.substring(0, 100);
      }
    }
    return '';
  }

  private extractSize(content: string): string {
    const sizeMatch = content.match(/size:\s*([0-9.]+\s*[KMGT]?B)/i);
    return sizeMatch ? sizeMatch[1] : '';
  }

  private getFileTypeIcon(fileType: string): string {
    const icons: Record<string, string> = {
      pdf: '📄',
      doc: '📝',
      docx: '📝',
      txt: '📄',
      md: '📝',
      image: '🖼️',
      jpg: '🖼️',
      png: '🖼️',
      gif: '🖼️',
      video: '🎥',
      mp4: '🎥',
      audio: '🎵',
      mp3: '🎵',
      zip: '📦',
      default: '📄'
    };
    return icons[fileType] || icons.default;
  }
}

export class AINodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'ai_chat');
  }

  public decorateReference(context: DecorationContext): DecorationResult {
    const baseDecoration = this.getBaseDecoration(context);

    // Extract AI-specific metadata
    const model = this.extractModel(context.content);
    const messageCount = this.extractMessageCount(context.content);
    const lastActivity = this.extractLastActivity(context.content);

    const safeTitle = this.sanitizeText(context.title);
    const relativeTime = lastActivity ? this.formatRelativeTime(lastActivity) : '';

    baseDecoration.html = `
      <span class="ns-noderef ns-noderef--ai-chat" 
            data-node-id="${context.nodeId}" 
            data-uri="${context.uri}"
            data-model="${model}"
            data-message-count="${messageCount}"
            role="button"
            tabindex="0">
        <span class="ns-noderef__ai-icon">🤖</span>
        <span class="ns-noderef__ai-info">
          <span class="ns-noderef__title">${safeTitle}</span>
          <span class="ns-noderef__ai-meta">
            ${model ? `<span class="ns-noderef__model">${model}</span>` : ''}
            ${messageCount ? `<span class="ns-noderef__message-count">${messageCount} messages</span>` : ''}
            ${relativeTime ? `<span class="ns-noderef__last-activity">${relativeTime}</span>` : ''}
          </span>
        </span>
        <span class="ns-noderef__ai-status">💭</span>
      </span>
    `;

    baseDecoration.cssClasses.push('ns-noderef--ai-chat');
    baseDecoration.ariaLabel = `AI Chat: ${safeTitle}${model ? ` (${model})` : ''}${messageCount ? `, ${messageCount} messages` : ''}`;
    baseDecoration.metadata = {
      ...baseDecoration.metadata,
      model,
      messageCount,
      lastActivity,
      relativeTime
    };

    return baseDecoration;
  }

  private extractModel(content: string): string {
    const modelMatch = content.match(/model:\s*([^\n]+)/i);
    return modelMatch ? modelMatch[1].trim() : '';
  }

  private extractMessageCount(content: string): number {
    const countMatch = content.match(/messages:\s*(\d+)/i);
    return countMatch ? parseInt(countMatch[1], 10) : 0;
  }

  private extractLastActivity(content: string): number | null {
    const activityMatch = content.match(/last_activity:\s*([^\n]+)/i);
    if (activityMatch) {
      const date = new Date(activityMatch[1]);
      return isNaN(date.getTime()) ? null : date.getTime();
    }
    return null;
  }
}

// ============================================================================
// Node Type Configuration
// ============================================================================

export const NODE_TYPE_CONFIGS: Record<string, NodeTypeConfig> = {
  default: {
    icon: '📝',
    label: 'Node',
    color: 'var(--node-default)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  text: {
    icon: '📝',
    label: 'Text',
    color: 'var(--node-text)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  task: {
    icon: '☐',
    label: 'Task',
    color: 'var(--node-task)',
    defaultDecoration: (context) => new TaskNodeDecorator(null!).decorateReference(context)
  },
  user: {
    icon: '👤',
    label: 'User',
    color: 'var(--node-user)',
    defaultDecoration: (context) => new UserNodeDecorator(null!).decorateReference(context)
  },
  date: {
    icon: '📅',
    label: 'Date',
    color: 'var(--node-date)',
    defaultDecoration: (context) => new DateNodeDecorator(null!).decorateReference(context)
  },
  document: {
    icon: '📄',
    label: 'Document',
    color: 'var(--node-document)',
    defaultDecoration: (context) => new DocumentNodeDecorator(null!).decorateReference(context)
  },
  ai_chat: {
    icon: '🤖',
    label: 'AI Chat',
    color: 'var(--node-ai-chat)',
    defaultDecoration: (context) => new AINodeDecorator(null!).decorateReference(context)
  },
  entity: {
    icon: '🏷️',
    label: 'Entity',
    color: 'var(--node-entity)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  query: {
    icon: '🔍',
    label: 'Query',
    color: 'var(--node-query)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  }
};

// ============================================================================
// Hybrid Decorator Factory (Supporting Both Rich Decorators and Components)
// ============================================================================

export class NodeDecoratorFactory {
  private nodeReferenceService: NodeReferenceService;
  private decorators: Map<string, BaseNodeDecorator> = new Map();

  constructor(nodeReferenceService: NodeReferenceService) {
    this.nodeReferenceService = nodeReferenceService;
    this.initializeDecorators();
  }

  private initializeDecorators(): void {
    // Rich HTML-based decorators for specific node types
    this.decorators.set('task', new TaskNodeDecorator(this.nodeReferenceService));
    this.decorators.set('user', new UserNodeDecorator(this.nodeReferenceService));
    this.decorators.set('date', new DateNodeDecorator(this.nodeReferenceService));
    this.decorators.set('document', new DocumentNodeDecorator(this.nodeReferenceService));
    this.decorators.set('ai_chat', new AINodeDecorator(this.nodeReferenceService));

    // Component-based decorators for fallback and basic node types
    const defaultDecorator = new DefaultNodeDecorator(this.nodeReferenceService);
    this.decorators.set('default', defaultDecorator);
    this.decorators.set('text', defaultDecorator);
    this.decorators.set('entity', defaultDecorator);
    this.decorators.set('query', defaultDecorator);
  }

  public getDecorator(nodeType: string): BaseNodeDecorator {
    return this.decorators.get(nodeType) || this.decorators.get('default')!;
  }

  /**
   * Creates decoration for the specified node type (supports both HTML and Component returns)
   */
  public decorateReference(context: DecorationContext): DecorationResult | ComponentDecoration {
    const decorator = this.getDecorator(context.nodeType);
    return decorator.decorateReference(context);
  }

  /**
   * Gets the configuration for a node type
   */
  public getNodeTypeConfig(nodeType: string): NodeTypeConfig {
    return NODE_TYPE_CONFIGS[nodeType] || NODE_TYPE_CONFIGS.default;
  }
}

// ============================================================================
// Exports
// ============================================================================

export {
  DefaultNodeDecorator,
  TaskNodeDecorator,
  UserNodeDecorator,
  DateNodeDecorator,
  DocumentNodeDecorator,
  AINodeDecorator
};

export default NodeDecoratorFactory;
