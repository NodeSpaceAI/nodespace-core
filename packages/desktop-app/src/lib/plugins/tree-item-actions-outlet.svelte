<!--
  TreeItemActionsOutlet — renders the active tree-item actions (ADR-082 §3.2) of
  one item of the sidebar collection tree, in priority order.

  Each action gets its own {@link ExtensionOutlet}, keyed by the contribution key
  (`<extension id>/<contribution id>`) and given the item's `{ nodeId, nodeType }`,
  so one action appearing, disappearing or failing never remounts or takes down
  the others (ADR-082 §3.4). `when(item)` is asked inside a derivation, so the list
  follows the state the predicates read. With no action visible for the item it
  renders nothing, not even its container.

  It does not hide itself: the tree that hosts it reveals the `tree-item-actions`
  container while the item is hovered or holds focus.
-->
<script lang="ts">
  import type { TreeItemActionProps } from './ui-extensions';
  import { getActiveTreeItemActions } from './ui-extensions.svelte';
  import ExtensionOutlet from './extension-outlet.svelte';

  let { nodeId, nodeType }: TreeItemActionProps = $props();

  const item = $derived<TreeItemActionProps>({ nodeId, nodeType });
  const actions = $derived(getActiveTreeItemActions(item));
</script>

{#if actions.length > 0}
  <div class="tree-item-actions">
    {#each actions as action (action.key)}
      <ExtensionOutlet load={action.load} props={item} />
    {/each}
  </div>
{/if}

<style>
  .tree-item-actions {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    gap: 0.125rem;
    margin-left: 0.25rem;
  }
</style>
