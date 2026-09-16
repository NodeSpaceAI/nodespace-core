<script lang="ts">
  import { getDeleteConfirmationState } from '$lib/services/delete-confirmation.svelte';
  import { focusTrap } from '$lib/actions/focus-trap';

  const confirmation = getDeleteConfirmationState();
  // Keyboard handling lives elsewhere: focusTrap owns Escape + Tab and lands
  // initial focus on Cancel, and each button activates on its own Enter
  // natively. Deliberately NO global Enter→confirm handler — with focus
  // defaulting to Cancel, that would make Enter delete the node while the
  // highlighted control says Cancel, on a dialog that warns "cannot be undone".
</script>

{#if confirmation.pending}
  <div
    class="overlay"
    role="none"
    onclick={confirmation.cancel}
    tabindex="-1"
  >
    <div
      class="modal"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="delete-modal-title"
      aria-describedby="delete-modal-desc"
      use:focusTrap={{ onEscape: confirmation.cancel }}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      tabindex="0"
    >
      <h2 id="delete-modal-title">Delete node and {confirmation.pending.descendantCount}
        {confirmation.pending.descendantCount === 1 ? 'descendant' : 'descendants'}?</h2>
      <p id="delete-modal-desc">This cannot be undone.</p>
      <div class="actions">
        <button class="btn-cancel" onclick={confirmation.cancel}>Cancel</button>
        <button class="btn-delete" onclick={confirmation.confirm}>Delete</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: var(--radius);
    padding: 24px;
    max-width: 360px;
    width: 100%;
  }

  h2 {
    margin: 0 0 8px;
    font-size: 16px;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  p {
    margin: 0 0 20px;
    font-size: 13px;
    color: hsl(var(--muted-foreground));
  }

  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  button {
    padding: 7px 16px;
    border-radius: 6px;
    border: none;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-cancel {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .btn-cancel:hover {
    background: hsl(var(--hover-background));
  }

  .btn-delete {
    background: hsl(var(--destructive));
    color: hsl(var(--destructive-foreground));
  }

  .btn-delete:hover {
    background: hsl(var(--destructive) / 0.85);
  }
</style>
