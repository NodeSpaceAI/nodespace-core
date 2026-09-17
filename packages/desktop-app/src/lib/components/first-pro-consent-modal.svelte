<!--
  First-Pro data-sharing consent modal.

  Presentational only: the slot wrapper (`first-pro-consent-slot`) owns visibility
  and the actions. This is the gate that keeps local data from reaching the shared
  public workspace without an explicit, informed, irreversible choice.

  - "Merge into public workspace" is disabled until the user ticks the
    acknowledgement checkbox (and is never the default-focused control).
  - "Keep this database local-only" — and Escape / overlay click — dismiss without
    sharing anything.
-->
<script lang="ts">
  import { focusTrap } from '$lib/actions/focus-trap';

  interface Props {
    /** Whether the modal is visible. */
    open?: boolean;
    /** True while the enable-sync call is in flight — disables the merge button
     * (Keep local stays actionable so a decline is never a no-op). */
    pending?: boolean;
    /** Opt in: merge this database into the public workspace (irreversible). */
    onMerge: () => void;
    /** Decline: keep this database local-only, share nothing. */
    onKeepLocal: () => void;
  }

  let { open = false, pending = false, onMerge, onKeepLocal }: Props = $props();

  let acknowledged = $state(false);

  function handleMerge() {
    if (!acknowledged || pending) return;
    acknowledged = false;
    onMerge();
  }

  function handleKeepLocal() {
    acknowledged = false;
    onKeepLocal();
  }
</script>

{#if open}
  <div class="consent-overlay" onclick={handleKeepLocal} role="presentation" tabindex="-1">
    <div
      class="consent-content"
      use:focusTrap={{ onEscape: handleKeepLocal }}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      role="dialog"
      aria-labelledby="consent-title"
      aria-describedby="consent-body"
      aria-modal="true"
      tabindex="0"
    >
      <h2 id="consent-title">Share this database with the public workspace?</h2>
      <div id="consent-body" class="consent-body">
        <p>
          Merging adds every note in this database to the shared
          <strong>public workspace</strong>, where other people using NodeSpace can
          <strong>read</strong> them. Editing and deleting stay yours alone.
        </p>
        <p class="consent-warning">
          This cannot be undone. Once your notes are in the public workspace, others may already have
          read or copied them, so they can never be un-shared.
        </p>
      </div>

      <label class="consent-ack">
        <input type="checkbox" bind:checked={acknowledged} disabled={pending} />
        <span>I understand this is permanent and cannot be undone.</span>
      </label>

      <div class="consent-actions">
        <!-- Never disabled: declining must always register, even mid-merge, so a
             click can't be swallowed into a silent no-op. -->
        <button class="btn btn-secondary" type="button" onclick={handleKeepLocal}>
          Keep this database local-only
        </button>
        <button
          class="btn btn-danger"
          type="button"
          onclick={handleMerge}
          disabled={pending || !acknowledged}
        >
          {pending ? 'Merging…' : 'Merge into public workspace'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .consent-overlay {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .consent-content {
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border-radius: 8px;
    padding: 24px;
    width: min(480px, calc(100vw - 48px));
  }

  .consent-content h2 {
    margin: 0 0 12px 0;
    font-size: 20px;
  }

  .consent-body {
    font-size: 14px;
    line-height: 1.5;
    color: hsl(var(--muted-foreground));
  }

  .consent-body p {
    margin: 0 0 10px 0;
  }

  .consent-warning {
    padding: 10px 12px;
    background: hsl(var(--destructive) / 0.12);
    border: 1px solid hsl(var(--destructive) / 0.4);
    border-radius: 6px;
    color: hsl(var(--foreground));
    font-weight: 500;
  }

  .consent-ack {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-top: 16px;
    font-size: 14px;
    color: hsl(var(--foreground));
    cursor: pointer;
  }

  .consent-ack input {
    margin-top: 2px;
  }

  .consent-actions {
    display: flex;
    gap: 12px;
    justify-content: flex-end;
    margin-top: 24px;
  }

  .btn {
    padding: 10px 18px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
  }

  .btn:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .btn-secondary {
    background-color: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .btn-secondary:hover:not(:disabled) {
    background-color: hsl(var(--muted) / 0.7);
  }

  .btn-danger {
    background-color: hsl(var(--destructive));
    color: hsl(var(--destructive-foreground));
  }

  .btn-danger:hover:not(:disabled) {
    background-color: hsl(var(--destructive-hover));
  }
</style>
