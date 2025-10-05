# Svelte Component Testing Guide

## Overview

This guide covers best practices for testing Svelte 5 components in the NodeSpace project. We use **@testing-library/svelte** with **Vitest** and **Happy-DOM** to create fast, reliable component tests that focus on user behavior rather than implementation details.

## Table of Contents

1. [Setup and Configuration](#setup-and-configuration)
2. [Testing Svelte 5 Components with Runes](#testing-svelte-5-components-with-runes)
3. [Keyboard Event Testing Patterns](#keyboard-event-testing-patterns)
4. [Event Propagation Verification](#event-propagation-verification)
5. [Component Lifecycle Testing](#component-lifecycle-testing)
6. [Accessibility Testing](#accessibility-testing)
7. [Common Pitfalls and Solutions](#common-pitfalls-and-solutions)
8. [Integration with Existing Infrastructure](#integration-with-existing-infrastructure)

## Setup and Configuration

### Test Environment

NodeSpace uses **Happy-DOM** as the test environment for its speed and Bun compatibility:

```typescript
// vitest.config.ts
export default defineConfig({
  test: {
    environment: 'happy-dom',
    globals: true,
    setupFiles: ['src/tests/setup-svelte-mocks.ts', 'src/tests/setup.ts']
  }
});
```

### Svelte 5 Rune Mocks

Svelte 5 runes (`$state`, `$derived`, `$effect`) are mocked in the test environment:

- `setup-svelte-mocks.ts` - Provides basic reactive mocks for runes
- `vitest.config.ts` - Injects rune mocks early in the build process

These mocks allow components using Svelte 5 runes to run in tests without the full Svelte runtime.

## Testing Svelte 5 Components with Runes

### Using $state in Components

When testing components that use `$state`:

```typescript
import { renderComponent, waitForEffects } from './svelte-test-utils';
import MyComponent from '$lib/components/my-component.svelte';

test('component with $state reactivity', async () => {
  const { component, getByText } = renderComponent(MyComponent, {
    props: { initialCount: 0 }
  });

  // Trigger state change
  component.$set({ initialCount: 5 });

  // Wait for reactive updates to complete
  await waitForEffects();

  expect(getByText('Count: 5')).toBeInTheDocument();
});
```

### Using $derived in Components

Components using `$derived` work automatically with the mocks:

```typescript
// Component using $derived
let count = $state(0);
let doubled = $derived(count * 2);

// Test
test('derived values update correctly', async () => {
  const { component, getByText } = renderComponent(MyComponent);

  component.$set({ count: 5 });
  await waitForEffects();

  expect(getByText('Doubled: 10')).toBeInTheDocument();
});
```

### Using $effect in Components

`$effect` is executed immediately in tests (no async waiting needed for effects):

```typescript
// Component with side effect
$effect(() => {
  console.log('Count changed:', count);
});

// Test - effect runs synchronously
test('effects execute on state changes', () => {
  const spy = vi.spyOn(console, 'log');

  renderComponent(MyComponent, { props: { count: 5 } });

  // Effect should have run immediately
  expect(spy).toHaveBeenCalledWith('Count changed:', 5);
});
```

### Best Practices for Rune Testing

1. **Always await `waitForEffects()`** after triggering state changes
2. **Use `component.$set()`** to update props programmatically
3. **Don't rely on effect cleanup** in tests - cleanup functions are ignored
4. **Test reactive behavior**, not internal state implementation

## Keyboard Event Testing Patterns

### Creating Keyboard Events

Use the `createKeyboardEvent` helper for consistent event creation:

```typescript
import { createKeyboardEvent } from './svelte-test-utils';

test('handles Enter key', async () => {
  const { getByRole } = renderComponent(MyComponent);

  const enterEvent = createKeyboardEvent('Enter');
  document.dispatchEvent(enterEvent);

  await waitForEffects();

  // Assert expected behavior
});
```

### Testing Keyboard Shortcuts

For modifier keys (Ctrl, Shift, Meta, Alt):

```typescript
test('handles Ctrl+Enter', async () => {
  const event = createKeyboardEvent('Enter', {
    ctrlKey: true
  });

  element.dispatchEvent(event);
  await waitForEffects();

  // Verify shortcut behavior
});

test('handles Shift+Tab for outdent', async () => {
  const event = createKeyboardEvent('Tab', {
    shiftKey: true
  });

  element.dispatchEvent(event);
  await waitForEffects();

  expect(onOutdent).toHaveBeenCalled();
});
```

### Testing Navigation Keys

Arrow keys, Tab, Escape are common navigation patterns:

```typescript
test('arrow keys navigate list', async () => {
  const { getAllByRole } = renderComponent(ListComponent);

  // ArrowDown to move selection
  document.dispatchEvent(createKeyboardEvent('ArrowDown'));
  await waitForEffects();

  const items = getAllByRole('option');
  expect(items[1]).toHaveAttribute('aria-selected', 'true');
});
```

### Document vs Element Event Listeners

Components may listen on `document` or specific elements:

```typescript
// For document listeners (like NodeAutocomplete)
test('document-level keyboard handling', () => {
  renderComponent(MyComponent);

  // Dispatch on document
  document.dispatchEvent(createKeyboardEvent('Escape'));
});

// For element listeners
test('element-level keyboard handling', () => {
  const { getByRole } = renderComponent(MyComponent);
  const input = getByRole('textbox');

  // Dispatch on specific element
  input.dispatchEvent(createKeyboardEvent('Enter'));
});
```

## Event Propagation Verification

### Why Event Propagation Matters

Modal overlays and autocomplete components must prevent keyboard events from affecting the underlying UI. Testing `stopPropagation()` ensures events don't leak through.

### Testing stopPropagation()

```typescript
test('prevents event propagation on arrow keys', async () => {
  renderComponent(NodeAutocomplete, {
    props: { visible: true, results: mockData }
  });

  const stopPropagationSpy = vi.fn();
  const arrowEvent = createKeyboardEvent('ArrowDown');
  arrowEvent.stopPropagation = stopPropagationSpy;

  document.dispatchEvent(arrowEvent);
  await waitForEffects();

  expect(stopPropagationSpy).toHaveBeenCalled();
});
```

### Testing preventDefault()

Similar pattern for `preventDefault()`:

```typescript
test('prevents default behavior on Enter', async () => {
  renderComponent(MyComponent);

  const preventDefaultSpy = vi.fn();
  const enterEvent = createKeyboardEvent('Enter');
  enterEvent.preventDefault = preventDefaultSpy;

  element.dispatchEvent(enterEvent);

  expect(preventDefaultSpy).toHaveBeenCalled();
});
```

### Critical Events to Test

Always verify event propagation for:

- **Arrow keys** (prevent cursor movement in background)
- **Enter** (prevent form submission/node creation in background)
- **Escape** (prevent closing parent modals)
- **Tab** (prevent focus changes when capturing keyboard)

## Component Lifecycle Testing

### Event Listener Cleanup

Components using `onMount` for event listeners **must** clean up on unmount:

```typescript
test('removes event listeners on unmount', () => {
  const addListenerSpy = vi.spyOn(document, 'addEventListener');
  const removeListenerSpy = vi.spyOn(document, 'removeEventListener');

  const lifecycle = simulateComponentLifecycle(MyComponent, {
    visible: true
  });

  // Verify listener was added
  expect(addListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));

  lifecycle.unmount();

  // Verify listener was removed
  expect(removeListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
});
```

### Testing Memory Leaks

Ensure events don't fire after unmount:

```typescript
test('does not respond to events after unmount', async () => {
  const onSelect = vi.fn();

  const lifecycle = simulateComponentLifecycle(MyComponent, {
    onselect: onSelect
  });

  lifecycle.unmount();

  // Try to trigger event after unmount
  document.dispatchEvent(createKeyboardEvent('Enter'));
  await waitForEffects();

  // Should not respond
  expect(onSelect).not.toHaveBeenCalled();
});
```

### Component Re-mounting

Test that components can be safely re-mounted:

```typescript
test('can remount component safely', () => {
  const lifecycle = simulateComponentLifecycle(MyComponent);

  lifecycle.unmount();
  const newResult = lifecycle.mount();

  expect(newResult.container).toBeInTheDocument();
});
```

## Accessibility Testing

### ARIA Attributes

Use the `getAriaAttributes` helper:

```typescript
import { getAriaAttributes } from './svelte-test-utils';

test('has correct ARIA attributes', () => {
  const { getByRole } = renderComponent(ListComponent);
  const listbox = getByRole('listbox');

  const aria = getAriaAttributes(listbox);

  expect(aria.role).toBe('listbox');
  expect(aria.label).toBe('Node selection');
  expect(aria.expanded).toBe('true');
});
```

### Testing Selection States

```typescript
test('updates aria-selected on navigation', async () => {
  const { getAllByRole } = renderComponent(ListComponent);

  document.dispatchEvent(createKeyboardEvent('ArrowDown'));
  await waitForEffects();

  const options = getAllByRole('option');
  expect(options[0]).toHaveAttribute('aria-selected', 'false');
  expect(options[1]).toHaveAttribute('aria-selected', 'true');
});
```

### Keyboard Navigation Support

Verify `tabindex` is correct:

```typescript
test('selected item is keyboard focusable', () => {
  const { getAllByRole } = renderComponent(ListComponent);
  const options = getAllByRole('option');

  // Selected item should be in tab order
  expect(options[0]).toHaveAttribute('tabindex', '0');

  // Other items should not be in tab order
  expect(options[1]).toHaveAttribute('tabindex', '-1');
});
```

### Screen Reader Announcements

Test for `aria-live` regions:

```typescript
test('announces changes to screen readers', () => {
  const { getByRole } = renderComponent(StatusComponent);
  const status = getByRole('status');

  const aria = getAriaAttributes(status);
  expect(aria.live).toBe('polite');
});
```

## Common Pitfalls and Solutions

### Pitfall 1: Not Waiting for Effects

**Problem:** Assertions fail because reactive updates haven't completed.

```typescript
// ❌ WRONG - No wait
test('updates count', () => {
  const { component, getByText } = renderComponent(Counter);
  component.$set({ count: 5 });
  expect(getByText('5')).toBeInTheDocument(); // FAILS - too fast
});

// ✅ CORRECT - Wait for effects
test('updates count', async () => {
  const { component, getByText } = renderComponent(Counter);
  component.$set({ count: 5 });
  await waitForEffects();
  expect(getByText('5')).toBeInTheDocument(); // PASSES
});
```

### Pitfall 2: Testing Implementation Details

**Problem:** Tests break when refactoring internal component structure.

```typescript
// ❌ WRONG - Testing implementation
test('has selectedIndex state', () => {
  const { component } = renderComponent(MyComponent);
  expect(component.selectedIndex).toBe(0); // Fragile
});

// ✅ CORRECT - Testing user-visible behavior
test('first item is selected by default', () => {
  const { getByRole } = renderComponent(MyComponent);
  const firstOption = getByRole('option', { selected: true });
  expect(firstOption).toBeInTheDocument();
});
```

### Pitfall 3: Incorrect Event Target

**Problem:** Events dispatched on wrong element don't trigger handlers.

```typescript
// ❌ WRONG - Component listens on document
test('handles keyboard', () => {
  const { container } = renderComponent(MyComponent);
  container.dispatchEvent(createKeyboardEvent('Enter')); // Won't work
});

// ✅ CORRECT - Dispatch on document
test('handles keyboard', () => {
  renderComponent(MyComponent);
  document.dispatchEvent(createKeyboardEvent('Enter')); // Works
});
```

### Pitfall 4: Forgetting to Clean Up Spies

**Problem:** Spies persist across tests causing false positives.

```typescript
// ❌ WRONG - Spy not cleared
let consoleSpy;
beforeEach(() => {
  consoleSpy = vi.spyOn(console, 'log');
});
// Missing: afterEach cleanup

// ✅ CORRECT - Clear spies after each test
let consoleSpy;
beforeEach(() => {
  consoleSpy = vi.spyOn(console, 'log');
});
afterEach(() => {
  vi.clearAllMocks();
});
```

### Pitfall 5: Not Mocking Event Handlers Properly

**Problem:** Component events don't connect to test handlers.

```typescript
// ❌ WRONG - Event prop name mismatch
test('calls handler', () => {
  const onSelect = vi.fn();
  renderComponent(MyComponent, {
    props: { select: onSelect } // Wrong: should be 'onselect'
  });
});

// ✅ CORRECT - Use 'on' prefix for event props
test('calls handler', () => {
  const onSelect = vi.fn();
  renderComponent(MyComponent, {
    props: { onselect: onSelect } // Correct
  });

  // Trigger event...
  expect(onSelect).toHaveBeenCalled();
});
```

## Integration with Existing Infrastructure

### Using Existing Test Setup

NodeSpace has pre-configured test infrastructure:

```typescript
// src/tests/setup.ts
// - @testing-library/jest-dom matchers
// - MutationObserver, ResizeObserver mocks
// - Enhanced Event constructor

// src/tests/setup-svelte-mocks.ts
// - Svelte 5 rune mocks ($state, $derived, $effect)
// - Plugin registry initialization

// vitest.config.ts
// - Happy-DOM environment
// - Global test setup
// - Coverage configuration
```

### Leveraging Test Utilities

The `svelte-test-utils.ts` module provides reusable helpers:

```typescript
import {
  renderComponent,
  createUserEvents,
  waitForEffects,
  createKeyboardEvent,
  getAriaAttributes,
  simulateComponentLifecycle
} from './svelte-test-utils';
```

### Running Tests

```bash
# Run all tests
bun run test

# Run specific test file
bun run test node-autocomplete.test.ts

# Run with coverage
bun run test:coverage

# Watch mode
bun run test:watch
```

### Quality Standards

All tests must:

- ✅ Pass without console errors or warnings
- ✅ Follow Testing Library best practices (query by role/text, not implementation)
- ✅ Test user-visible behavior, not internals
- ✅ Clean up resources (event listeners, spies, timers)
- ✅ Be deterministic (no flaky tests)
- ✅ Have clear, descriptive test names

## Example: Complete Component Test Suite

Here's a complete example testing a modal component:

```typescript
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import MyModal from '$lib/components/my-modal.svelte';
import {
  renderComponent,
  createKeyboardEvent,
  waitForEffects,
  simulateComponentLifecycle
} from './svelte-test-utils';

describe('MyModal', () => {
  let onClose: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onClose = vi.fn();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe('Rendering', () => {
    it('renders when open', () => {
      renderComponent(MyModal, {
        props: { open: true, title: 'Test Modal' }
      });

      expect(screen.getByRole('dialog')).toBeInTheDocument();
      expect(screen.getByText('Test Modal')).toBeInTheDocument();
    });

    it('does not render when closed', () => {
      renderComponent(MyModal, {
        props: { open: false, title: 'Test Modal' }
      });

      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });
  });

  describe('Keyboard Handling', () => {
    it('closes on Escape key', async () => {
      renderComponent(MyModal, {
        props: { open: true, onclose: onClose }
      });

      document.dispatchEvent(createKeyboardEvent('Escape'));
      await waitForEffects();

      expect(onClose).toHaveBeenCalled();
    });

    it('prevents event propagation', async () => {
      renderComponent(MyModal, {
        props: { open: true }
      });

      const stopPropSpy = vi.fn();
      const escEvent = createKeyboardEvent('Escape');
      escEvent.stopPropagation = stopPropSpy;

      document.dispatchEvent(escEvent);

      expect(stopPropSpy).toHaveBeenCalled();
    });
  });

  describe('Lifecycle', () => {
    it('cleans up event listeners', () => {
      const removeListenerSpy = vi.spyOn(document, 'removeEventListener');

      const lifecycle = simulateComponentLifecycle(MyModal, {
        open: true
      });

      lifecycle.unmount();

      expect(removeListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });
  });

  describe('Accessibility', () => {
    it('has correct ARIA role', () => {
      renderComponent(MyModal, {
        props: { open: true, title: 'Test Modal' }
      });

      const dialog = screen.getByRole('dialog');
      expect(dialog).toBeInTheDocument();
    });
  });
});
```

## Additional Resources

- [@testing-library/svelte Documentation](https://testing-library.com/docs/svelte-testing-library/intro/)
- [Vitest Documentation](https://vitest.dev/)
- [Happy-DOM GitHub](https://github.com/capricorn86/happy-dom)
- [Testing Library Guiding Principles](https://testing-library.com/docs/guiding-principles/)
- [NodeSpace Test Examples](../src/tests/components/)

## Contributing

When adding new component tests:

1. **Follow the established patterns** in existing tests
2. **Use the test utilities** - don't reinvent helpers
3. **Test behavior, not implementation** - focus on what users see
4. **Include accessibility tests** - ARIA attributes and keyboard navigation
5. **Verify event cleanup** - prevent memory leaks
6. **Document complex test cases** - explain non-obvious test logic

For questions or improvements to this guide, open an issue or discussion in the NodeSpace repository.
