<!--
  CollectionMoveDialog — pick the collection to put another collection into.

  The choices are the collections the moved one can actually go into (the
  caller leaves out itself, its parents and everything inside it), so the dialog
  never offers a move that would make a cycle. It is the keyboard route to what
  dragging a row onto another does.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    open: boolean;
    /** The collection being moved. */
    name: string;
    /** The collections it can go into. */
    targets: { id: string; name: string }[];
    onMove: (_parentId: string) => void;
  }

  let { open = $bindable(false), name, targets, onMove }: Props = $props();

  let selectedId = $state('');

  // Start on the first choice each time the dialog opens, and only then: the
  // list of choices is recomputed whenever the collections change, which must
  // not undo what the person has picked.
  $effect(() => {
    if (open) selectedId = untrack(() => targets[0]?.id ?? '');
  });

  function confirm() {
    if (!selectedId) return;
    onMove(selectedId);
    open = false;
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Move “{name}”</Dialog.Title>
      <Dialog.Description>
        Put it inside another collection. It stays in the collections it is already in.
      </Dialog.Description>
    </Dialog.Header>

    <div class="grid gap-2">
      <label class="text-muted-foreground text-sm font-medium" for="collection-move-target">
        Move into
      </label>
      <select
        id="collection-move-target"
        class="collection-move-select"
        value={selectedId}
        onchange={(event) => (selectedId = event.currentTarget.value)}
        disabled={targets.length === 0}
      >
        {#each targets as target (target.id)}
          <option value={target.id} selected={target.id === selectedId}>{target.name}</option>
        {/each}
      </select>
      {#if targets.length === 0}
        <p class="text-muted-foreground text-sm">There is no other collection it can go into.</p>
      {/if}
    </div>

    <Dialog.Footer>
      <Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
      <Button variant="default" disabled={!selectedId} onclick={confirm}>Move</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<style>
  .collection-move-select {
    width: 100%;
    height: 2.25rem;
    padding: 0 0.5rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-size: 0.875rem;
  }

  .collection-move-select:focus-visible {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 1px;
  }
</style>
