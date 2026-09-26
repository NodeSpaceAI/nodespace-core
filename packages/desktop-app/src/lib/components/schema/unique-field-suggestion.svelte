<!--
  UniqueFieldSuggestion — the dismissible "already exists" suggestion for a
  schema field declaring `unique` (ADR-065). Renders nothing until the
  field's UniqueFieldCheck holds a match; then offers adopt-existing (open the
  match) or keep-as-new (dismiss). See unique-field-check.svelte.ts.

  Props:
  - check: the field's UniqueFieldCheck
  - field: the schema field, for the message's wording
-->
<script lang="ts">
  import { Alert, AlertDescription } from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import type { SchemaField } from '$lib/types/schema-node';
  import { labelForField } from '$lib/utils/schema-field-label';
  import type { UniqueFieldCheck } from './unique-field-check.svelte';
  import UserRoundSearchIcon from '@lucide/svelte/icons/user-round-search';

  let { check, field }: { check: UniqueFieldCheck; field: SchemaField } = $props();

  // The match's title is its server-computed display name (title_template
  // for types that declare one), not recomposed here.
  const matchName = $derived(check.match?.title || undefined);
</script>

{#if check.match}
  <Alert variant="warning">
    <UserRoundSearchIcon class="h-4 w-4" />
    <AlertDescription class="unique-field-message">
      This {labelForField(field).toLowerCase()} already exists{matchName ? `: ${matchName}` : ''} — use
      the existing one instead?
    </AlertDescription>
    <!-- AlertDescription renders a <p>, which cannot contain block content
         without the browser silently restructuring the DOM — so the action
         buttons are a sibling, not a child. -->
    <div class="flex gap-2">
      <Button type="button" size="sm" variant="outline" onclick={() => check.adopt()}>
        Use existing
      </Button>
      <Button type="button" size="sm" variant="ghost" onclick={() => check.dismiss()}>
        Keep as new
      </Button>
    </div>
  </Alert>
{/if}

<style>
  /* `class` on <AlertDescription> is forwarded to that component's own
     element, so the compiler can't see the usage here. */
  :global(.unique-field-message) {
    margin: 0 0 0.5rem 0;
  }
</style>
