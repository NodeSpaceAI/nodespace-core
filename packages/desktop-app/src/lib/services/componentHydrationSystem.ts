/**
 * Component Hydration System
 *
 * Finds component placeholders in rendered HTML and replaces them with
 * mounted Svelte components. Designed for plugin architecture where
 * node types can be developed separately but bundled at compile time.
 */

import type { SvelteComponent } from 'svelte';
import type { ComponentDecoration } from '../types/componentDecoration';
import { getNodeReferenceComponent } from '../components/references';
import { pluginRegistry } from '../plugins/index';
import { performanceTracker } from './performanceTracker';

export interface HydrationContext {
  /** Target container element containing placeholders */
  container: HTMLElement;
  /** Optional callback when components are mounted */
  onComponentMounted?: (element: HTMLElement, component: SvelteComponent) => void;
  /** Optional error handler */
  onError?: (error: Error, placeholder: HTMLElement) => void;
}

export interface HydrationResult {
  /** Number of components successfully hydrated */
  hydrated: number;
  /** Number of placeholders that failed to hydrate */
  failed: number;
  /** Any errors that occurred */
  errors: Array<{ element: HTMLElement; error: Error }>;
}

/**
 * Component Hydration System
 *
 * Features:
 * - Finds ns-component-placeholder elements
 * - Parses props and metadata from data attributes
 * - Mounts appropriate Svelte components
 * - Supports core and plugin node types (bundled at compile time)
 * - Clean error handling and reporting
 */
export class ComponentHydrationSystem {
  private mountedComponents = new Map<HTMLElement, SvelteComponent>();

  /**
   * Hydrate all component placeholders in a container
   */
  public async hydrate(context: HydrationContext): Promise<HydrationResult> {
    const { container, onComponentMounted, onError } = context;

    // Find all component placeholders
    const placeholders = container.querySelectorAll(
      '.ns-component-placeholder[data-hydrate="pending"]'
    );

    const result: HydrationResult = {
      hydrated: 0,
      failed: 0,
      errors: []
    };

    for (const placeholder of placeholders) {
      const startTime = performance.now();
      let componentType = 'unknown';
      let propsSize = 0;

      try {
        const htmlElement = placeholder as HTMLElement;

        // Parse component data from placeholder
        const componentData = this.parseComponentData(htmlElement);

        if (componentData) {
          componentType = htmlElement.dataset.nodeType || componentData.component.name || 'unknown';
          try {
            propsSize = JSON.stringify(componentData.props).length;
          } catch {
            propsSize = 0; // Fallback if props can't be serialized
          }

          // Mount the component with enhanced error recovery
          const component = await this.mountComponentWithFallback(htmlElement, componentData);

          // Track mounted component
          this.mountedComponents.set(htmlElement, component);

          // Mark as hydrated
          htmlElement.dataset.hydrate = 'completed';

          // Record performance metrics (skip in test environment)
          const hydrationTime = performance.now() - startTime;
          if (!import.meta.env?.TEST) {
            performanceTracker.recordHydration(componentType, hydrationTime, 0, propsSize, true);
          }

          // Callback
          onComponentMounted?.(htmlElement, component);

          result.hydrated++;
        } else {
          // Set componentType for metrics even if parsing fails
          const htmlElement = placeholder as HTMLElement;
          componentType =
            htmlElement.dataset.nodeType || htmlElement.dataset.component || 'unknown';
          throw new Error('Failed to parse component data from placeholder');
        }
      } catch (error) {
        const err = error as Error;
        const hydrationTime = performance.now() - startTime;

        // Record failed performance metrics (skip in test environment)
        if (!import.meta.env?.TEST) {
          performanceTracker.recordHydration(
            componentType,
            hydrationTime,
            0,
            propsSize,
            false,
            err.message
          );
        }

        result.failed++;
        result.errors.push({ element: placeholder as HTMLElement, error: err });

        // Enhanced error recovery - try fallback rendering
        try {
          this.renderErrorFallback(placeholder as HTMLElement, err);
        } catch (fallbackError) {
          console.error('ComponentHydrationSystem: Fallback rendering also failed', fallbackError);
        }

        // Mark as failed
        (placeholder as HTMLElement).dataset.hydrate = 'failed';

        // Error callback
        onError?.(err, placeholder as HTMLElement);

        console.error('ComponentHydrationSystem: Failed to hydrate component', {
          error: err,
          placeholder
        });
      }
    }

    return result;
  }

  /**
   * Parse component data from placeholder element
   */
  private parseComponentData(element: HTMLElement): ComponentDecoration | null {
    try {
      const componentName = element.dataset.component;
      const nodeType = element.dataset.nodeType;
      const propsJSON = element.dataset.props;
      const metadataJSON = element.dataset.metadata;

      if (!componentName || !nodeType || !propsJSON) {
        return null;
      }

      // Parse JSON data (decode HTML entities first)
      const props = JSON.parse(propsJSON.replace(/&quot;/g, '"'));
      const metadata = metadataJSON ? JSON.parse(metadataJSON.replace(/&quot;/g, '"')) : {};

      // Get component class (supports both core and plugin components)
      const component = this.resolveComponent(componentName, nodeType);

      return {
        component,
        props,
        metadata
      };
    } catch (error) {
      console.error('ComponentHydrationSystem: Error parsing component data', { error, element });
      return null;
    }
  }

  /**
   * Resolve component class by name and node type
   * Now uses the unified plugin registry system
   */
  private resolveComponent(
    componentName: string,
    nodeType: string
  ): new (...args: unknown[]) => SvelteComponent {
    // Try getting from unified plugin registry first
    const pluginComponent = pluginRegistry.getReferenceComponent(nodeType);
    if (pluginComponent) {
      return pluginComponent as new (...args: unknown[]) => SvelteComponent;
    }

    // Fall back to legacy system (maintains compatibility during transition)
    return getNodeReferenceComponent(nodeType) as new (...args: unknown[]) => SvelteComponent;
  }

  /**
   * Mount a Svelte component with fallback error recovery
   */
  private async mountComponentWithFallback(
    placeholder: HTMLElement,
    decoration: ComponentDecoration
  ): Promise<SvelteComponent> {
    try {
      return this.mountComponent(placeholder, decoration);
    } catch (error) {
      console.warn('ComponentHydrationSystem: Primary component mounting failed, trying fallback', {
        componentType: decoration.component.name,
        error
      });

      // Try mounting BaseNodeReference as fallback
      const fallbackDecoration: ComponentDecoration = {
        component: getNodeReferenceComponent('base'),
        props: {
          ...decoration.props,
          className: `${decoration.props.className || ''} ns-fallback-component`.trim()
        },
        metadata: {
          ...decoration.metadata,
          fallbackReason: (error as Error).message,
          originalComponent: decoration.component.name
        }
      };

      return this.mountComponent(placeholder, fallbackDecoration);
    }
  }

  /**
   * Mount a Svelte component in place of a placeholder
   */
  private mountComponent(
    placeholder: HTMLElement,
    decoration: ComponentDecoration
  ): SvelteComponent {
    const { component: ComponentClass, props } = decoration;

    try {
      // Check if we're in a test/Node.js environment or if the component isn't a constructor
      if (
        typeof window === 'undefined' ||
        typeof document === 'undefined' ||
        typeof ComponentClass !== 'function'
      ) {
        // In test environment, create a mock component
        const mockComponent = {
          $destroy: () => {},
          $on: () => {},
          $set: () => {},
          props
        } as unknown as SvelteComponent;

        // Update placeholder to show it was "mounted"
        placeholder.innerHTML = `<div class="ns-mock-component">${props.content || 'Component'}</div>`;

        return mockComponent;
      }

      // Create component instance in browser environment
      const component = new ComponentClass({
        target: placeholder,
        props
      });

      // Set up event forwarding if needed
      if (decoration.events) {
        for (const [eventName, handler] of Object.entries(decoration.events)) {
          (component as SvelteComponent & { $on: (event: string, handler: unknown) => void }).$on(
            eventName,
            handler
          );
        }
      }

      return component;
    } catch (error) {
      console.error('ComponentHydrationSystem: Error mounting component', {
        error,
        ComponentClass,
        props
      });
      throw error;
    }
  }

  /**
   * Cleanup mounted components
   */
  public cleanup(container?: HTMLElement): void {
    if (container) {
      // Cleanup components within specific container
      const elementsToCleanup = Array.from(this.mountedComponents.keys()).filter((element) =>
        container.contains(element)
      );

      for (const element of elementsToCleanup) {
        this.destroyComponent(element);
      }
    } else {
      // Cleanup all components
      for (const element of this.mountedComponents.keys()) {
        this.destroyComponent(element);
      }
    }
  }

  /**
   * Destroy a single component
   */
  private destroyComponent(element: HTMLElement): void {
    const component = this.mountedComponents.get(element);
    if (component) {
      try {
        component.$destroy();
      } catch (error) {
        console.warn('ComponentHydrationSystem: Error destroying component', { error, element });
      }
      this.mountedComponents.delete(element);
    }
  }

  /**
   * Render error fallback when component mounting fails completely
   */
  private renderErrorFallback(placeholder: HTMLElement, error: Error): void {
    const nodeType = placeholder.dataset.nodeType || 'unknown';
    const content = placeholder.textContent || 'Unknown content';

    // Create user-friendly error display
    placeholder.innerHTML = `
      <span class="ns-error-fallback" title="Component failed to load: ${error.message}">
        <span class="ns-error-icon" aria-hidden="true">⚠️</span>
        <span class="ns-error-content">${this.escapeHtml(content)}</span>
        <span class="ns-error-type">[${nodeType}]</span>
      </span>
    `;

    // Add error styling
    placeholder.classList.add('ns-hydration-error');

    // Development-only detailed error info
    if (import.meta.env?.DEV) {
      placeholder.title = `Hydration Error: ${error.message}\nComponent: ${placeholder.dataset.component}\nNode Type: ${nodeType}`;
    }
  }

  /**
   * Escape HTML to prevent XSS in error messages
   */
  private escapeHtml(text: string): string {
    const div = document?.createElement('div');
    if (div) {
      div.textContent = text;
      return div.innerHTML;
    }
    // Fallback for Node.js environment
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  /**
   * Get statistics about mounted components
   */
  public getStats(): {
    totalMounted: number;
    componentTypes: Record<string, number>;
    performanceReport?: Record<string, unknown>;
  } {
    const componentTypes: Record<string, number> = {};

    for (const [element] of this.mountedComponents) {
      const componentName = element.dataset.component || 'unknown';
      componentTypes[componentName] = (componentTypes[componentName] || 0) + 1;
    }

    return {
      totalMounted: this.mountedComponents.size,
      componentTypes,
      performanceReport: performanceTracker.generateReport() as unknown as Record<string, unknown>
    };
  }

  /**
   * Get real-time performance metrics
   */
  public getPerformanceMetrics() {
    return {
      realtime: performanceTracker.getRealtimeStats(),
      fullReport: performanceTracker.generateReport()
    };
  }
}

/**
 * Singleton instance for application-wide use
 */
export const componentHydrationSystem = new ComponentHydrationSystem();

export default ComponentHydrationSystem;
