<!--
  CollectionRowMenu — the actions menu of one collection row in the sidebar.

  It is the keyboard route to everything dragging does: rename the collection,
  move it into another collection, and take it out of a collection it sits in
  (one entry per parent, since a collection can sit in several). The sidebar
  reveals the trigger while the row is hovered or holds focus.
-->
<script lang="ts">
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

  interface Props {
    /** The collection the menu acts on. */
    name: string;
    /** The collections it sits in, one "Remove from" entry each. */
    parents: { id: string; name: string }[];
    /** Whether there is any collection it could still be moved into. */
    canMove: boolean;
    onRename: () => void;
    onMove: () => void;
    onRemoveFrom: (_parentId: string) => void;
    /**
     * Hide the menu without unmounting it. The sidebar hides it while the
     * collection is being renamed: unmounting an open menu returns focus to its
     * trigger, which would take the focus off the rename field.
     */
    hidden?: boolean;
  }

  let { name, parents, canMove, onRename, onMove, onRemoveFrom, hidden = false }: Props = $props();

  // Choosing Rename opens a field that takes focus. The menu returns focus to its
  // trigger as it closes, which would take the field's focus away again and end
  // the rename, so the rename starts once the menu has closed and the menu's own
  // focus return is cancelled. The menu reports the close more than once, so both
  // reports are cancelled and the rename starts on the first.
  let renameRequested = false;
  let renameStarted = false;

  function handleOpenChange(open: boolean) {
    if (open) {
      renameRequested = false;
      renameStarted = false;
    }
  }

  function handleCloseAutoFocus(event: Event) {
    if (!renameRequested) return;
    event.preventDefault();
    if (renameStarted) return;
    renameStarted = true;
    onRename();
  }
</script>

<div class="collection-menu" class:hidden>
  <DropdownMenu.Root onOpenChange={handleOpenChange}>
    <DropdownMenu.Trigger
      class="collection-menu-btn"
      aria-label={`Actions for ${name}`}
      title="Rename, move or remove"
    >
      <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <circle cx="5" cy="12" r="1.6" />
        <circle cx="12" cy="12" r="1.6" />
        <circle cx="19" cy="12" r="1.6" />
      </svg>
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" onCloseAutoFocus={handleCloseAutoFocus}>
      <DropdownMenu.Item onSelect={() => (renameRequested = true)}>Rename</DropdownMenu.Item>
      {#if canMove}
        <DropdownMenu.Item onSelect={onMove}>Move to collection…</DropdownMenu.Item>
      {/if}
      {#each parents as parent (parent.id)}
        <DropdownMenu.Item onSelect={() => onRemoveFrom(parent.id)}>
          Remove from {parent.name}
        </DropdownMenu.Item>
      {/each}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</div>

<style>
  .collection-menu.hidden {
    display: none;
  }

  .collection-menu {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    margin-left: 0.25rem;
    opacity: 0;
  }

  /* Shown while the row is hovered or holds focus. Opacity, not visibility, keeps
     the trigger in the tab order, so tabbing onto it reveals it. */
  :global(.collection-item:hover) .collection-menu,
  :global(.collection-item:focus-within) .collection-menu,
  .collection-menu:has(:global([data-state='open'])) {
    opacity: 1;
  }

  .collection-menu :global(.collection-menu-btn) {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.25rem;
    height: 1.25rem;
    padding: 0;
    border: none;
    border-radius: 0.25rem;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }

  .collection-menu :global(.collection-menu-btn:hover),
  .collection-menu :global(.collection-menu-btn[data-state='open']) {
    background: hsl(var(--accent));
  }

  .collection-menu :global(.collection-menu-btn svg) {
    width: 0.875rem;
    height: 0.875rem;
  }
</style>
