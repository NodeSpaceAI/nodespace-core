/**
 * Developer Inspector for Component System
 *
 * Provides debugging tools and inspection capabilities for component hydration
 * in development environments. Makes debugging component issues much easier.
 */

// ComponentDecoration import removed - not used
// componentHydrationSystem import removed - not used
import { performanceTracker } from './performance-tracker';

export interface ComponentInspectionResult {
  placeholder: HTMLElement;
  status: 'pending' | 'completed' | 'failed';
  componentType: string;
  nodeType: string;
  props: Record<string, unknown>;
  metadata: Record<string, unknown>;
  errors: string[];
  performance?: {
    hydrationTime: number;
    propsSize: number;
  };
}

export interface HydrationDebugInfo {
  totalPlaceholders: number;
  completedComponents: number;
  failedComponents: number;
  pendingComponents: number;
  componentBreakdown: Record<string, number>;
  performanceIssues: string[];
  components: ComponentInspectionResult[];
}

export class DeveloperInspector {
  private debugMode: boolean = false;

  constructor() {
    // Only enable in development
    if (import.meta.env?.DEV && typeof window !== 'undefined') {
      this.enableDebugMode();
    }
  }

  /**
   * Enable debug mode with global window object
   */
  private enableDebugMode(): void {
    this.debugMode = true;

    // Attach to window for console access
    (window as unknown as { NodeSpaceInspector: unknown }).NodeSpaceInspector = {
      inspectContainer: (container: HTMLElement) => this.inspectContainer(container),
      inspectAllComponents: () => this.inspectAllComponents(),
      getPerformanceReport: () => performanceTracker.generateReport(),
      getRealtimeStats: () => performanceTracker.getRealtimeStats(),
      debugComponent: (componentId: string) => this.debugSpecificComponent(componentId),
      exportMetrics: () => performanceTracker.exportMetrics(),

      // Utility functions
      help: () => this.showHelp(),
      version: '1.0.0'
    };

    console.log('🔍 NodeSpace Inspector enabled! Use NodeSpaceInspector in console for debugging.');
    console.log('💡 Try: NodeSpaceInspector.help() for available commands');
  }

  /**
   * Inspect all components in a container
   */
  public inspectContainer(container: HTMLElement): HydrationDebugInfo {
    const placeholders = container.querySelectorAll('.ns-component-placeholder');
    const components: ComponentInspectionResult[] = [];

    let completed = 0;
    let failed = 0;
    let pending = 0;
    const componentBreakdown: Record<string, number> = {};
    const performanceIssues: string[] = [];

    for (const placeholder of placeholders) {
      const element = placeholder as HTMLElement;
      const inspection = this.inspectPlaceholder(element);
      components.push(inspection);

      // Count by status
      if (inspection.status === 'completed') completed++;
      else if (inspection.status === 'failed') failed++;
      else pending++;

      // Component type breakdown
      componentBreakdown[inspection.componentType] =
        (componentBreakdown[inspection.componentType] || 0) + 1;

      // Check for performance issues
      if (inspection.performance) {
        if (inspection.performance.hydrationTime > 50) {
          performanceIssues.push(
            `${inspection.componentType}: Slow hydration (${inspection.performance.hydrationTime}ms)`
          );
        }
        if (inspection.performance.propsSize > 1024) {
          performanceIssues.push(
            `${inspection.componentType}: Large props (${inspection.performance.propsSize} bytes)`
          );
        }
      }

      if (inspection.errors.length > 0) {
        performanceIssues.push(`${inspection.componentType}: ${inspection.errors.length} error(s)`);
      }
    }

    return {
      totalPlaceholders: placeholders.length,
      completedComponents: completed,
      failedComponents: failed,
      pendingComponents: pending,
      componentBreakdown,
      performanceIssues,
      components
    };
  }

  /**
   * Inspect all components across the document
   */
  public inspectAllComponents(): HydrationDebugInfo {
    return this.inspectContainer(document.body);
  }

  /**
   * Inspect a specific component placeholder
   */
  private inspectPlaceholder(element: HTMLElement): ComponentInspectionResult {
    const status = (element.dataset.hydrate as 'pending' | 'completed' | 'failed') || 'pending';
    const componentType = element.dataset.component || 'unknown';
    const nodeType = element.dataset.nodeType || 'unknown';

    let props = {};
    let metadata = {};
    const errors: string[] = [];

    // Parse props and metadata
    try {
      if (element.dataset.props) {
        props = JSON.parse(element.dataset.props.replace(/&quot;/g, '"'));
      }
    } catch (error) {
      errors.push(`Props parsing failed: ${error}`);
    }

    try {
      if (element.dataset.metadata) {
        metadata = JSON.parse(element.dataset.metadata.replace(/&quot;/g, '"'));
      }
    } catch (error) {
      errors.push(`Metadata parsing failed: ${error}`);
    }

    // Check for common issues
    if (!componentType || componentType === 'unknown') {
      errors.push('Missing or invalid component type');
    }

    if (Object.keys(props).length === 0) {
      errors.push('No props found - may indicate parsing issue');
    }

    // Get performance data if available
    let performance;
    const propsSize = element.dataset.props?.length || 0;
    if (status === 'completed' && propsSize > 0) {
      performance = {
        hydrationTime: 0, // Would need to be tracked during hydration
        propsSize
      };
    }

    return {
      placeholder: element,
      status,
      componentType,
      nodeType,
      props,
      metadata,
      errors,
      performance
    };
  }

  /**
   * Debug a specific component by its node ID or element
   */
  public debugSpecificComponent(
    identifier: string | HTMLElement
  ): ComponentInspectionResult | null {
    let element: HTMLElement | null = null;

    if (typeof identifier === 'string') {
      // Find by node ID
      element = document.querySelector(`[data-node-id="${identifier}"]`);
      if (!element) {
        // Try as CSS selector
        element = document.querySelector(identifier);
      }
    } else {
      element = identifier;
    }

    if (!element) {
      console.error(`🔍 Component not found: ${identifier}`);
      return null;
    }

    const inspection = this.inspectPlaceholder(element);

    // Enhanced debug output for console
    console.group(`🔍 Component Debug: ${inspection.componentType}`);
    console.log('Status:', inspection.status);
    console.log('Node Type:', inspection.nodeType);
    console.log('Props:', inspection.props);
    console.log('Metadata:', inspection.metadata);
    if (inspection.errors.length > 0) {
      console.warn('Errors:', inspection.errors);
    }
    if (inspection.performance) {
      console.log('Performance:', inspection.performance);
    }
    console.log('Element:', element);
    console.groupEnd();

    return inspection;
  }

  /**
   * Show help information
   */
  private showHelp(): void {
    console.log(`
🔍 NodeSpace Inspector Commands:

📊 Inspection:
  • inspectContainer(element) - Inspect components in container
  • inspectAllComponents() - Inspect all components on page
  • debugComponent(nodeId) - Debug specific component

📈 Performance:
  • getPerformanceReport() - Full performance analysis
  • getRealtimeStats() - Current performance metrics
  • exportMetrics() - Export raw performance data

🛠️ Utilities:
  • help() - Show this help
  • version - Inspector version

Example Usage:
  NodeSpaceInspector.inspectAllComponents()
  NodeSpaceInspector.debugComponent('task-123')
  NodeSpaceInspector.getPerformanceReport()
    `);
  }

  /**
   * Validate component props against expected interface
   */
  public validateComponentProps(
    componentType: string,
    props: Record<string, unknown>
  ): {
    isValid: boolean;
    errors: string[];
    warnings: string[];
  } {
    const errors: string[] = [];
    const warnings: string[] = [];

    // Basic validation for all components
    if (!props.nodeId) {
      errors.push('Missing required prop: nodeId');
    }
    if (!props.content) {
      warnings.push('Missing content prop');
    }
    if (!props.href) {
      errors.push('Missing required prop: href');
    }

    // For now, all components use BaseNodeReference with basic validation
    // Type-specific validation will be added when node types are properly specified
    switch (componentType) {
      case 'BaseNodeReference':
      default:
        // Basic validation for all node types using BaseNodeReference
        break;
    }

    return {
      isValid: errors.length === 0,
      errors,
      warnings
    };
  }
}

/**
 * Singleton instance for development debugging
 */
export const developerInspector = new DeveloperInspector();

export default DeveloperInspector;
