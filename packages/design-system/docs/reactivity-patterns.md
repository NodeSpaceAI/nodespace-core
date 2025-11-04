# Svelte 5 Reactivity Patterns for Store-Backed Components

## Problem: Infinite Loops with Two-Way Binding

### The Anti-Pattern

```typescript
// ❌ WRONG: Creates infinite loop
let selectValues = $state<Record<string, { value: string; label: string }>>({});

// Effect 1: Sync FROM store TO component state
$effect(() => {
  schema.fields.forEach(field => {
    selectValues[field.name] = { value: node.properties[field.name], label: ... };
  });
});

// Effect 2: Sync FROM component state TO store (INFINITE LOOP!)
$effect(() => {
  Object.entries(selectValues).forEach(([fieldName, selected]) => {
    if (node.properties[fieldName] !== selected.value) {
      updateProperty(fieldName, selected.value);
    }
  });
});

// Usage with bind:selected creates circular dependency
<Select.Root bind:selected={selectValues[field.name]}>
```

**Why this fails:**
1. Store update → Effect 1 fires → Updates selectValues
2. selectValues update → Effect 2 fires → Calls updateProperty()
3. updateProperty() → Store update → Effect 1 fires → Infinite loop

### The Root Cause

- `bind:selected` creates **bidirectional binding**
- `$effect` creates **reactive synchronization**
- Together, they form a **circular dependency**: A → B → A → B → ...

## Solution: Controlled Component Pattern

### Key Principles

1. **Single Source of Truth**: Store owns the data
2. **Derived State**: Component derives values from store using `$derived`
3. **One-Way Data Flow**: Store → Derived → UI, UI Events → Store
4. **Explicit Updates**: UI changes call functions, not reactive declarations

### The Correct Pattern

```typescript
// ✅ CORRECT: Derive values from store (no separate state)
const selectValues = $derived.by(() => {
  if (!schema || !node) return {};

  const values: Record<string, { value: string; label: string }> = {};

  schema.fields
    .filter(f => f.type === 'enum')
    .forEach(field => {
      const currentValue = node.properties[field.name];
      values[field.name] = {
        value: String(currentValue || ''),
        label: formatLabel(String(currentValue || ''))
      };
    });

  return values;
});

// ✅ CORRECT: Update through explicit function
function updateProperty(fieldName: string, value: unknown) {
  sharedNodeStore.updateNode(
    nodeId,
    { properties: { ...node.properties, [fieldName]: value } },
    { type: 'viewer', viewerId: 'schema-property-form' }
  );
}

// ✅ CORRECT: Controlled component (no bind:selected)
<Select.Root
  type="single"
  selected={selectValues[field.name]}
  onSelectedChange={(selected) => {
    if (selected?.value) {
      updateProperty(field.name, selected.value);
    }
  }}
>
```

**Why this works:**
1. Store update → `$derived` recalculates → selectValues updates → UI updates
2. UI change → onSelectedChange fires → updateProperty() called → Store updates
3. No circular dependency: derived doesn't trigger on writes, only reads

## Detailed Comparison

### Pattern Breakdown

| Aspect | ❌ Anti-Pattern | ✅ Correct Pattern |
|--------|----------------|-------------------|
| **State Management** | Separate `$state` for component | `$derived` from store |
| **Synchronization** | Two `$effect` blocks (bidirectional) | Zero effects (unidirectional) |
| **Binding Type** | `bind:selected` (two-way) | `selected={...}` (one-way) |
| **Update Mechanism** | Reactive `$effect` | Explicit callback |
| **Data Flow** | Circular (A ↔ B) | Linear (Store → UI → Store) |
| **Infinite Loops** | Yes, guaranteed | No, impossible |

### When to Use Each Rune

#### `$state` - Component-Local State

**Use for:**
- UI-only state (dropdown open/closed, hover states)
- Temporary form data NOT backed by store
- Computed values that don't depend on external stores

**Don't use for:**
- Data that comes from SharedNodeStore
- Values that sync with external state

```typescript
// ✅ Good: UI-only state
let isOpen = $state(false);
let searchQuery = $state('');

// ❌ Bad: Store-backed data
let nodeContent = $state(''); // Should use $derived from store
```

#### `$derived` - Computed Values from Store

**Use for:**
- Values computed from store data
- Transforming store data for UI (e.g., { value, label } format)
- Any value that should automatically update when store changes

**Don't use for:**
- Values that need to be written to independently
- Temporary user input (use $state for that)

```typescript
// ✅ Good: Derive from store
const selectValues = $derived.by(() => {
  return schema.fields.map(f => ({
    value: node.properties[f.name],
    label: formatLabel(node.properties[f.name])
  }));
});

// ✅ Good: Transform store data
const hasRequiredFields = $derived(
  schema.fields.filter(f => f.required).every(f => node.properties[f.name])
);

// ❌ Bad: Using $state instead
let selectValues = $state({}); // Should be $derived
```

#### `$effect` - Side Effects Only

**Use for:**
- Loading data from external sources (fetch, WebSocket)
- Logging/debugging reactive changes
- Imperative DOM operations
- Synchronizing with non-Svelte libraries

**Don't use for:**
- Syncing component state with store (use $derived instead)
- Updating store based on component state (use callbacks instead)
- Computing derived values (use $derived instead)

```typescript
// ✅ Good: Load external data
$effect(() => {
  if (nodeType) {
    loadSchema(nodeType);
  }
});

// ✅ Good: Logging
$effect(() => {
  console.log('Node properties changed:', node.properties);
});

// ❌ Bad: Syncing state (use $derived)
$effect(() => {
  selectValues = computeFromStore(node.properties);
});

// ❌ Bad: Updating store (use callback)
$effect(() => {
  if (localState !== storeState) {
    updateStore(localState);
  }
});
```

## Handling Dynamic Schemas

### Challenge: Schema-Driven Fields

With dynamic schemas, you don't know field names at compile time:

```typescript
interface SchemaDefinition {
  fields: Array<{
    name: string;        // Dynamic: "status", "priority", "assignee", etc.
    type: 'enum' | 'date' | 'text';
    values?: string[];
  }>;
}

interface Node {
  properties: Record<string, unknown>; // Dynamic keys
}
```

### Solution: Derive All Field Values

```typescript
// ✅ CORRECT: Derive dynamic fields
const fieldValues = $derived.by(() => {
  if (!schema || !node) return {};

  const values: Record<string, unknown> = {};

  schema.fields.forEach(field => {
    values[field.name] = node.properties[field.name];
  });

  return values;
});

// ✅ CORRECT: Type-specific derivations
const enumFields = $derived.by(() => {
  if (!schema || !node) return {};

  return schema.fields
    .filter(f => f.type === 'enum')
    .reduce((acc, field) => {
      acc[field.name] = {
        value: String(node.properties[field.name] || ''),
        label: formatEnumLabel(String(node.properties[field.name] || ''))
      };
      return acc;
    }, {} as Record<string, { value: string; label: string }>);
});

const dateFields = $derived.by(() => {
  if (!schema || !node) return {};

  return schema.fields
    .filter(f => f.type === 'date')
    .reduce((acc, field) => {
      const rawValue = node.properties[field.name];
      acc[field.name] = typeof rawValue === 'string' ? parseDate(rawValue) : undefined;
      return acc;
    }, {} as Record<string, DateValue | undefined>);
});
```

### Rendering Dynamic Fields

```svelte
{#each schema.fields as field (field.name)}
  <div class="space-y-2">
    <label>{field.description || field.name}</label>

    {#if field.type === 'enum' && enumFields[field.name]}
      <Select.Root
        type="single"
        selected={enumFields[field.name]}
        onSelectedChange={(selected) => {
          if (selected?.value) {
            updateProperty(field.name, selected.value);
          }
        }}
      >
        <Select.Trigger>{enumFields[field.name].label}</Select.Trigger>
        <Select.Content>
          {#each field.values as value}
            <Select.Item {value} label={formatEnumLabel(value)} />
          {/each}
        </Select.Content>
      </Select.Root>

    {:else if field.type === 'date'}
      <Calendar
        value={dateFields[field.name]}
        onValueChange={(date) => {
          updateProperty(field.name, formatDateISO(date));
        }}
      />

    {:else if field.type === 'text'}
      <Input
        value={(node.properties[field.name] as string) || ''}
        oninput={(e) => updateProperty(field.name, e.currentTarget.value)}
      />
    {/if}
  </div>
{/each}
```

## Working with bits-ui Select Component

### bits-ui Select API

```typescript
interface SelectRootProps {
  type: 'single' | 'multiple';
  selected?: { value: string; label: string };  // Current selection
  onSelectedChange?: (selected: { value: string; label: string } | undefined) => void;
}
```

### Correct Usage Pattern

```typescript
// ✅ CORRECT: Controlled component
<Select.Root
  type="single"
  selected={$derived(/* compute from store */)}
  onSelectedChange={(selected) => {
    // Explicit update to store
    updateProperty('fieldName', selected?.value);
  }}
>

// ❌ WRONG: Two-way binding
<Select.Root
  type="single"
  bind:selected={localState}  // Creates reactivity loop
>
```

### Why bind:selected Fails

When you use `bind:selected={localState}`:

1. **Component updates local state** when user selects
2. **Local state change** triggers `$effect` watching localState
3. **$effect updates store** via updateProperty()
4. **Store change** triggers `$effect` watching store
5. **$effect updates local state** back to same value
6. **Svelte detects state change** (even if same value) → Re-render
7. **Re-render triggers bind:selected** → Updates localState → LOOP

With controlled pattern:
1. **User selects** → onSelectedChange callback fires
2. **Callback updates store** via updateProperty()
3. **Store change** triggers `$derived` recalculation
4. **$derived returns new value** → UI updates
5. **No loop**: derived doesn't trigger on writes

## Performance Considerations

### Deriving is Cheap

Svelte 5's `$derived` is highly optimized:
- Only recalculates when dependencies change
- Uses fine-grained reactivity (tracks individual properties)
- No performance penalty vs manual state management

```typescript
// ✅ Efficient: Only recalculates when node.properties.status changes
const statusLabel = $derived(formatEnumLabel(node.properties.status));

// ✅ Efficient: Only recalculates when relevant fields change
const enumFields = $derived.by(() => {
  // Svelte tracks which properties are accessed
  // Only re-runs when those specific properties change
  return schema.fields
    .filter(f => f.type === 'enum')
    .reduce((acc, field) => {
      acc[field.name] = { value: node.properties[field.name], label: ... };
      return acc;
    }, {});
});
```

### Avoid Premature Optimization

Don't avoid `$derived` out of performance concerns:

```typescript
// ❌ Bad: Trying to "optimize" with manual state
let cachedValues = $state({});
$effect(() => {
  // This is SLOWER and more error-prone than $derived
  if (JSON.stringify(node.properties) !== JSON.stringify(cachedValues)) {
    cachedValues = { ...node.properties };
  }
});

// ✅ Good: Let Svelte handle it
const values = $derived({ ...node.properties });
```

## Migration Guide

### From Problematic Pattern to Correct Pattern

**Step 1: Identify separate state**

```typescript
// ❌ BEFORE
let selectValues = $state<Record<string, { value: string; label: string }>>({});
```

**Step 2: Replace with $derived**

```typescript
// ✅ AFTER
const selectValues = $derived.by(() => {
  // Compute from store data
  return computeSelectValues(node, schema);
});
```

**Step 3: Remove synchronization effects**

```typescript
// ❌ BEFORE: Delete these
$effect(() => {
  selectValues = initializeSelectValues();
});

$effect(() => {
  Object.entries(selectValues).forEach(([key, val]) => {
    updateProperty(key, val.value);
  });
});

// ✅ AFTER: No effects needed!
```

**Step 4: Replace bind: with controlled pattern**

```typescript
// ❌ BEFORE
<Select.Root bind:selected={selectValues[field.name]}>

// ✅ AFTER
<Select.Root
  selected={selectValues[field.name]}
  onSelectedChange={(sel) => updateProperty(field.name, sel?.value)}
>
```

## Common Pitfalls

### Pitfall 1: Using $state for store-backed data

```typescript
// ❌ Wrong
let nodeContent = $state(node.content);

// ✅ Correct
const nodeContent = $derived(node.content);
```

### Pitfall 2: Syncing with $effect

```typescript
// ❌ Wrong: Creates dependency loop
$effect(() => {
  localState = storeState;
});

$effect(() => {
  if (localState !== storeState) {
    updateStore(localState);
  }
});

// ✅ Correct: Derive and update explicitly
const localState = $derived(storeState);

function handleChange(newValue: unknown) {
  updateStore(newValue);
}
```

### Pitfall 3: Binding to derived values

```typescript
// ❌ Wrong: Can't bind to derived
const derivedValue = $derived(node.property);
<Input bind:value={derivedValue} />  // Error: can't bind to derived

// ✅ Correct: Use controlled component
const derivedValue = $derived(node.property);
<Input
  value={derivedValue}
  oninput={(e) => updateProperty('property', e.currentTarget.value)}
/>
```

### Pitfall 4: Over-using $effect

```typescript
// ❌ Wrong: Using effect for derived value
let fullName = $state('');
$effect(() => {
  fullName = `${node.firstName} ${node.lastName}`;
});

// ✅ Correct: Use $derived
const fullName = $derived(`${node.firstName} ${node.lastName}`);
```

## Real-World Example: Schema Property Form

See `/Users/malibio/nodespace/nodespace-core-dev1/packages/design-system/src/lib/examples/SchemaPropertyFormPattern.svelte` for a complete working example demonstrating:

1. ✅ Dynamic schema-driven field generation
2. ✅ Multiple field types (enum, date, text)
3. ✅ Derived state from SharedNodeStore
4. ✅ Controlled component pattern
5. ✅ No infinite loops or circular dependencies
6. ✅ Proper TypeScript types
7. ✅ Clean, maintainable code

## Summary: The Golden Rules

1. **Store is source of truth** - Never duplicate store data in component $state
2. **Derive, don't sync** - Use $derived to compute values from store
3. **Control, don't bind** - Use controlled components (selected + onChange) not bind:selected
4. **Update explicitly** - Write to store through functions, not reactive declarations
5. **Effects are for side effects** - Don't use $effect for derived values or store updates

Follow these patterns and you'll never hit infinite loops or reactivity issues with Svelte 5!
