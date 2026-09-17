<!--
  NestedFieldTrigger — the control a property form renders in place of an input
  for a nested (object/array) field.

  It looks like the other field controls (same height/border as an Input) but
  opens NestedPropertyModal instead of editing in place, and summarises the
  current value so the collapsed form still says how much is filled in.

  Shared by every property form so all three render nested fields identically.
-->
<script lang="ts">
  import type { SchemaField } from '$lib/types/schema-node';
  import { nestedFieldSummary } from '$lib/utils/nested-property-ops';

  let {
    field,
    value,
    fieldId,
    onopen
  }: {
    field: SchemaField;
    value: unknown;
    fieldId: string;
    onopen: () => void;
  } = $props();
</script>

<button
  type="button"
  id={fieldId}
  class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm transition-all hover:opacity-80 focus-visible:outline-none focus-visible:bg-accent focus-visible:text-accent-foreground"
  onclick={onopen}
>
  <span class="text-muted-foreground">{nestedFieldSummary(field, value)}</span>
  <span class="text-xs text-muted-foreground">Edit</span>
</button>
