/**
 * Svelte Component Testing Utilities
 *
 * Reusable utilities for testing Svelte 5 components with @testing-library/svelte.
 * Provides type-safe helpers for rendering, user events, and async effects.
 *
 * Key Features:
 * - Type-safe component rendering with props inference
 * - User event helpers for keyboard/mouse interactions
 * - Async effect waiting for Svelte 5 runes ($state, $derived, $effect)
 * - Event handler mocking and verification
 * - Event propagation testing utilities
 */

import { render, type RenderResult } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import type { Component, ComponentProps } from 'svelte';
import { vi, type MockInstance } from 'vitest';
import { tick } from 'svelte';

/**
 * Render options for Svelte components
 */
export interface RenderOptions<Comp extends Component> {
  /**
   * Props to pass to the component
   */
  props?: Partial<ComponentProps<Comp>>;

  /**
   * Container element to render into
   */
  target?: HTMLElement;

  /**
   * Additional context values (if needed)
   */
  context?: Map<unknown, unknown>;
}

/**
 * Type-safe component render helper
 *
 * Wraps @testing-library/svelte's render with enhanced type safety and
 * returns both the render result and component instance.
 *
 * @param Component - Svelte component to render
 * @param options - Render options including props
 * @returns Render result with component instance
 *
 * @example
 * ```ts
 * const { getByRole, component } = renderComponent(NodeAutocomplete, {
 *   props: {
 *     visible: true,
 *     results: mockResults
 *   }
 * });
 * ```
 */
export function renderComponent<Comp extends Component>(
  Component: Comp,
  options: RenderOptions<Comp> = {}
): RenderResult<Comp> {
  // Type assertion required: @testing-library/svelte v5's render() has overly restrictive types
  // that don't properly infer component props. This is a documented limitation of the library.
  // See: https://github.com/testing-library/svelte-testing-library/issues
  // We cast through unknown to bypass the incompatible type definitions
  return render(Component as never, options as never) as unknown as RenderResult<Comp>;
}

/**
 * Create a configured user event instance
 *
 * Returns a user-event instance with appropriate timing for testing.
 * Includes both keyboard and pointer interactions.
 *
 * @returns Configured user event instance
 *
 * @example
 * ```ts
 * const user = createUserEvents();
 * await user.keyboard('{ArrowDown}');
 * await user.click(element);
 * ```
 */
export function createUserEvents() {
  return userEvent.setup({
    // Use default timing for realistic interaction simulation
    delay: null
  });
}

/**
 * Wait for Svelte effects to complete
 *
 * Waits for all pending Svelte 5 effects ($effect) and reactive updates
 * to complete. Essential for testing async component behavior.
 *
 * @param additionalMs - Additional milliseconds to wait after tick
 * @returns Promise that resolves when effects are complete
 *
 * @example
 * ```ts
 * // After triggering a state change
 * await waitForEffects();
 * // Now DOM is updated and effects have run
 * expect(element).toBeInTheDocument();
 * ```
 */
export async function waitForEffects(additionalMs: number = 0): Promise<void> {
  // Wait for Svelte's reactive updates
  await tick();

  // If additional wait time is needed (for animations, timers, etc.)
  if (additionalMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, additionalMs));
  }
}

/**
 * Event handler mock helpers
 *
 * Creates typed mock functions for component event handlers.
 * Useful for verifying component events are dispatched correctly.
 *
 * @returns Object with event handler mocks
 *
 * @example
 * ```ts
 * const handlers = mockEventHandlers<{
 *   select: NodeResult;
 *   close: void;
 * }>();
 *
 * // Attach handlers after rendering using $on method
 * const { component } = renderComponent(NodeAutocomplete, {
 *   props: {
 *     visible: true,
 *     results: mockResults
 *   }
 * });
 *
 * component.$on('select', handlers.select);
 * component.$on('close', handlers.close);
 *
 * // Later in test
 * expect(handlers.select).toHaveBeenCalledWith(
 *   expect.objectContaining({ detail: mockResults[0] })
 * );
 * ```
 */
export function mockEventHandlers<T extends Record<string, unknown>>(): {
  [K in keyof T]: MockInstance;
} {
  return new Proxy(
    {},
    {
      get(_target, _prop) {
        return vi.fn();
      }
    }
  ) as { [K in keyof T]: MockInstance };
}

/**
 * Verify event propagation was stopped
 *
 * Checks that stopPropagation() was called on an event.
 * Essential for testing keyboard event handling in modals/overlays.
 *
 * @param event - Event object to check
 * @returns True if stopPropagation was called
 *
 * @example
 * ```ts
 * // Set up spy before event
 * const stopPropSpy = vi.fn();
 * const event = new KeyboardEvent('keydown', { key: 'ArrowDown' });
 * event.stopPropagation = stopPropSpy;
 *
 * element.dispatchEvent(event);
 *
 * expect(stopPropSpy).toHaveBeenCalled();
 * expectEventStopped(event);
 * ```
 */
export function expectEventStopped(event: Event): boolean {
  // In testing, we spy on stopPropagation to verify it was called
  // Runtime type check is required because the function may or may not be a Vitest mock
  type StopPropagationFn = Event['stopPropagation'] & { mock?: { calls: unknown[] } };
  const stopPropagation = event.stopPropagation as StopPropagationFn;

  if (typeof stopPropagation === 'function' && stopPropagation.mock) {
    return stopPropagation.mock.calls.length > 0;
  }

  // If not a mock, we can't verify - return false
  return false;
}

/**
 * Create a keyboard event with proper configuration
 *
 * Helper for creating KeyboardEvent instances with correct bubbling
 * and event properties for testing keyboard interactions.
 *
 * @param key - Key name (e.g., 'Enter', 'ArrowDown', 'Escape')
 * @param options - Additional event options
 * @returns Configured KeyboardEvent
 *
 * @example
 * ```ts
 * const enterEvent = createKeyboardEvent('Enter', { shiftKey: true });
 * element.dispatchEvent(enterEvent);
 * ```
 */
export function createKeyboardEvent(
  key: string,
  options: globalThis.KeyboardEventInit = {}
): KeyboardEvent {
  return new KeyboardEvent('keydown', {
    key,
    bubbles: true,
    cancelable: true,
    ...options
  });
}

/**
 * Wait for element to be in the document
 *
 * Polls for an element to appear in the DOM. Useful for testing
 * conditionally rendered components.
 *
 * @param getElement - Function that returns the element (or null)
 * @param timeout - Maximum time to wait in milliseconds
 * @returns Promise that resolves with the element
 *
 * @example
 * ```ts
 * const autocomplete = await waitForElement(
 *   () => screen.queryByRole('listbox'),
 *   1000
 * );
 * expect(autocomplete).toBeInTheDocument();
 * ```
 */
export async function waitForElement<T extends HTMLElement>(
  getElement: () => T | null,
  timeout: number = 3000
): Promise<T> {
  const startTime = Date.now();

  while (Date.now() - startTime < timeout) {
    const element = getElement();
    if (element) {
      return element;
    }
    await waitForEffects(50);
  }

  throw new Error(`Element not found within ${timeout}ms`);
}

/**
 * Get ARIA attributes from element
 *
 * Helper for extracting and verifying ARIA attributes in tests.
 * Provides type-safe access to common ARIA properties.
 *
 * @param element - HTML element to inspect
 * @returns Object with ARIA attributes
 *
 * @example
 * ```ts
 * const aria = getAriaAttributes(listbox);
 * expect(aria.role).toBe('listbox');
 * expect(aria.selected).toBe('true');
 * ```
 */
export function getAriaAttributes(element: HTMLElement): {
  role: string | null;
  label: string | null;
  selected: string | null;
  expanded: string | null;
  hidden: string | null;
  disabled: string | null;
  live: string | null;
} {
  return {
    role: element.getAttribute('role'),
    label: element.getAttribute('aria-label'),
    selected: element.getAttribute('aria-selected'),
    expanded: element.getAttribute('aria-expanded'),
    hidden: element.getAttribute('aria-hidden'),
    disabled: element.getAttribute('aria-disabled'),
    live: element.getAttribute('aria-live')
  };
}

/**
 * Simulate component lifecycle
 *
 * Helper for testing component mount/unmount behavior.
 * Useful for verifying event listener cleanup.
 *
 * @param Component - Component to test
 * @param props - Props to pass to component
 * @returns Functions to mount and unmount the component
 *
 * @example
 * ```ts
 * const lifecycle = simulateComponentLifecycle(NodeAutocomplete, {
 *   visible: true,
 *   results: []
 * });
 *
 * // Component is mounted
 * lifecycle.unmount();
 * // Component is unmounted - listeners should be cleaned up
 *
 * // Re-mount if needed
 * lifecycle.mount();
 * ```
 */
export function simulateComponentLifecycle<Comp extends Component>(
  Component: Comp,
  props: Partial<ComponentProps<Comp>> = {}
): {
  mount: () => RenderResult<Comp>;
  unmount: () => void;
  getCurrentResult: () => RenderResult<Comp> | null;
} {
  let currentResult: RenderResult<Comp> | null = null;

  const mount = () => {
    if (currentResult) {
      currentResult.unmount();
    }
    currentResult = renderComponent(Component, { props });
    return currentResult;
  };

  const unmount = () => {
    if (currentResult) {
      currentResult.unmount();
      currentResult = null;
    }
  };

  const getCurrentResult = () => currentResult;

  // Auto-mount on creation
  mount();

  return { mount, unmount, getCurrentResult };
}
