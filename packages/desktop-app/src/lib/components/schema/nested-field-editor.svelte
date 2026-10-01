<!--
  NestedFieldEditor — recursive, schema-driven editor for nested (object/array)
  property values.

  Pure and presentational: it takes a `field` definition, the current `value`,
  and an `onChange` callback. It NEVER touches the store — every edit rebuilds the
  value immutably (via nested-property-ops) and emits it up through `onChange`.
  This keeps it unit-testable and reusable, and lets the owning modal own
  persistence.

  Rendering:
  - object field (field.fields): a row per sub-field. Leaf sub-fields render a
    SchemaFieldLeaf; object/array sub-fields recurse into a NestedFieldEditor
    inside a per-field Collapsible. Each row has a delete button that removes the
    key from the object.
  - array field: a per-element Collapsible. Object elements recurse (synthetic
    object field built from itemFields); scalar elements render a SchemaFieldLeaf.
    Each element has a delete button; an "Add item" button appends a new element.

  Hierarchy is tonal, not stroked: object elements sit on a muted fill, scalar
  elements are bare inputs, nesting is expressed by indentation alone, and the
  add button is ghost.

  Arbitrary nesting depth works by recursion. Null/undefined values are treated as
  an empty object/array so a partially-filled value always renders.
-->
<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import { Button } from '$lib/components/ui/button';
  import XIcon from '@lucide/svelte/icons/x';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import SchemaFieldLeaf from './schema-field-leaf.svelte';
  import Self from './nested-field-editor.svelte';
  import type { SchemaField } from '$lib/types/schema-node';
  import { labelForField } from '$lib/utils/schema-field-label';
  import {
    setObjectKey,
    deleteObjectKey,
    replaceArrayIndex,
    deleteArrayIndex,
    addArrayItem,
    makeEmptyArrayItem,
    isNestedField,
    shiftItemOpenStateOnDelete
  } from '$lib/utils/nested-property-ops';

  let {
    field,
    value,
    onChange,
    depth = 0
  }: {
    field: SchemaField;
    value: unknown;
    onChange: (_value: unknown) => void;
    depth?: number;
  } = $props();

  // Per-child expand/collapse state (nested objects keyed by name, array elements
  // by index). Managed via open/onOpenChange rather than bind:open so an undefined
  // initial entry reads as collapsed without a binding warning.
  let openKeys = $state<Record<string, boolean>>({});

  const record = $derived(
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {}
  );
  const items = $derived(Array.isArray(value) ? (value as unknown[]) : []);

  // Delete an array element, shifting the per-element open-state down so expand/
  // collapse follows the content across the delete (both openKeys and the each
  // block are index-keyed).
  function removeArrayItem(index: number) {
    openKeys = shiftItemOpenStateOnDelete(openKeys, index);
    onChange(deleteArrayIndex(value, index));
  }

  // Synthetic object field describing each element of an array-of-objects, so the
  // recursion can treat an element exactly like a nested object.
  function arrayObjectItemField(index: number): SchemaField {
    return {
      name: `${field.name}[${index}]`,
      friendlyName: `${labelForField(field)} ${index + 1}`,
      type: 'object',
      protection: field.protection,
      indexed: false,
      fields: field.itemFields ?? []
    };
  }

  // Synthetic leaf field describing each scalar element of an array, carrying any
  // enum values declared on the array field so enum item arrays still resolve.
  function arrayScalarItemField(index: number): SchemaField {
    return {
      name: `${field.name}[${index}]`,
      friendlyName: `${labelForField(field)} ${index + 1}`,
      type: field.itemType ?? 'text',
      protection: field.protection,
      indexed: false,
      coreValues: field.coreValues,
      userValues: field.userValues
    };
  }
</script>

{#if field.type === 'object'}
  <div class="grid gap-3" class:pl-3={depth > 0}>
    {#each field.fields ?? [] as sub (sub.name)}
      {@const subValue = record[sub.name]}
      {@const subId = `nested-${sub.name}-${depth}`}
      <div class="grid gap-1.5">
        {#if isNestedField(sub)}
          <Collapsible.Root
            open={!!openKeys[sub.name]}
            onOpenChange={(o) => (openKeys = { ...openKeys, [sub.name]: o })}
          >
            <div class="flex items-center justify-between gap-2">
              <Collapsible.Trigger
                class="flex flex-1 items-center gap-2 py-1 text-left text-sm font-medium transition-all hover:opacity-80"
              >
                <svg
                  class="h-3.5 w-3.5 text-muted-foreground transition-transform duration-200"
                  class:rotate-180={openKeys[sub.name]}
                  viewBox="0 0 16 16"
                  fill="none"
                >
                  <path
                    d="M4 6l4 4 4-4"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
                <span>{labelForField(sub)}</span>
                <span class="text-xs text-muted-foreground">
                  {sub.type === 'array'
                    ? `${Array.isArray(subValue) ? subValue.length : 0} items`
                    : `${sub.fields?.length ?? 0} fields`}
                </span>
              </Collapsible.Trigger>
              <Button
                variant="ghost"
                size="icon"
                class="text-muted-foreground hover:text-destructive size-7"
                aria-label={`Remove ${sub.name}`}
                onclick={() => onChange(deleteObjectKey(value, sub.name))}
              >
                <XIcon class="size-4" />
              </Button>
            </div>
            <Collapsible.Content class="pt-1">
              <Self
                field={sub}
                value={subValue}
                depth={depth + 1}
                onChange={(newSub) => onChange(setObjectKey(value, sub.name, newSub))}
              />
            </Collapsible.Content>
          </Collapsible.Root>
        {:else}
          <div class="flex items-start justify-between gap-2">
            <label for={subId} class="text-sm font-medium">
              {labelForField(sub)}
            </label>
            <Button
              variant="ghost"
              size="icon"
              class="text-muted-foreground hover:text-destructive size-7 shrink-0"
              aria-label={`Remove ${sub.name}`}
              onclick={() => onChange(deleteObjectKey(value, sub.name))}
            >
              <XIcon class="size-4" />
            </Button>
          </div>
          <SchemaFieldLeaf
            field={sub}
            value={subValue}
            fieldId={subId}
            onChange={(newSub) => onChange(setObjectKey(value, sub.name, newSub))}
          />
        {/if}
      </div>
    {/each}
  </div>
{:else if field.type === 'array'}
  <div class="grid gap-2" class:pl-3={depth > 0}>
    {#each items as item, index (index)}
      {@const itemKey = `item-${index}`}
      {@const objectItem = field.itemType === 'object'}
      {@const itemId = `nested-${field.name}-${index}-${depth}`}
      <div class={objectItem ? 'rounded-md bg-muted/40' : ''}>
        {#if objectItem}
          <Collapsible.Root
            open={!!openKeys[itemKey]}
            onOpenChange={(o) => (openKeys = { ...openKeys, [itemKey]: o })}
          >
            <div class="flex items-center justify-between gap-2 px-2">
              <Collapsible.Trigger
                class="flex flex-1 items-center gap-2 py-2 text-left text-sm font-medium transition-all hover:opacity-80"
              >
                <svg
                  class="h-3.5 w-3.5 text-muted-foreground transition-transform duration-200"
                  class:rotate-180={openKeys[itemKey]}
                  viewBox="0 0 16 16"
                  fill="none"
                >
                  <path
                    d="M4 6l4 4 4-4"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
                <span>Item {index + 1}</span>
              </Collapsible.Trigger>
              <Button
                variant="ghost"
                size="icon"
                class="text-muted-foreground hover:text-destructive size-7"
                aria-label={`Remove item ${index + 1}`}
                onclick={() => removeArrayItem(index)}
              >
                <XIcon class="size-4" />
              </Button>
            </div>
            <Collapsible.Content class="px-3 pb-3">
              <Self
                field={arrayObjectItemField(index)}
                value={item}
                depth={depth + 1}
                onChange={(newItem) => onChange(replaceArrayIndex(value, index, newItem))}
              />
            </Collapsible.Content>
          </Collapsible.Root>
        {:else}
          <div class="flex items-center gap-2">
            <div class="flex-1">
              <SchemaFieldLeaf
                field={arrayScalarItemField(index)}
                value={item}
                fieldId={itemId}
                onChange={(newItem) => onChange(replaceArrayIndex(value, index, newItem))}
              />
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="text-muted-foreground hover:text-destructive size-7 shrink-0"
              aria-label={`Remove item ${index + 1}`}
              onclick={() => removeArrayItem(index)}
            >
              <XIcon class="size-4" />
            </Button>
          </div>
        {/if}
      </div>
    {/each}
    <div>
      <Button
        variant="ghost"
        size="sm"
        onclick={() => onChange(addArrayItem(value, makeEmptyArrayItem(field)))}
      >
        <PlusIcon class="mr-1.5 size-4" /> Add item
      </Button>
    </div>
  </div>
{:else}
  <!-- A leaf field reached directly (defensive): render its control. -->
  <SchemaFieldLeaf {field} {value} fieldId={`nested-${field.name}-${depth}`} {onChange} />
{/if}
