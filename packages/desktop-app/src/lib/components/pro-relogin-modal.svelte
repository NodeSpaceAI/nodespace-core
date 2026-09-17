<!--
  Pro-tier re-login modal.

  Surfaced by the app shell when the Pro daemon reports `AUTH_REQUIRED`
  (`SyncStatusEvent.state` = 4) — i.e. the persisted refresh token could
  not be renewed, so CloudSyncService needs a fresh interactive sign-in.

  Purely presentational: the shell owns the visibility + the actions.
  - "Sign In Again" kicks off the PKCE flow (`pro_initiate_oauth`); the
    daemon opens the browser and the modal closes as `sync:status`
    transitions away from `auth-required`.
  - "Work Offline" dismisses the modal for this auth-required episode.
    Local edits keep saving and replay once a session is restored, so
    nothing is lost by staying offline.
-->

<script lang="ts">
  import { focusTrap } from '$lib/actions/focus-trap';

  interface Props {
    /** Whether the modal is visible. */
    open?: boolean;
    /** Free-form daemon detail (e.g. the refresh error); shown if present. */
    detail?: string;
    /** True while an InitiateOAuth call is in flight — disables the buttons. */
    pending?: boolean;
    /** Start a fresh interactive sign-in. */
    onSignIn: () => void;
    /** Keep working without syncing for now. */
    onWorkOffline: () => void;
  }

  let { open = false, detail = '', pending = false, onSignIn, onWorkOffline }: Props = $props();
</script>

{#if open}
  <div class="relogin-overlay" onclick={onWorkOffline} role="presentation" tabindex="-1">
    <div
      class="relogin-content"
      use:focusTrap={{ onEscape: onWorkOffline }}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      role="dialog"
      aria-labelledby="relogin-title"
      aria-describedby="relogin-body"
      aria-modal="true"
      tabindex="0"
    >
      <h2 id="relogin-title">Sign-in required</h2>
      <p id="relogin-body" class="relogin-body">
        Your NodeSpace Pro session expired and couldn't be renewed automatically. Sign in again to
        keep syncing across your devices — or keep working offline. Your changes are saved locally
        and will sync once you're signed back in.
      </p>

      {#if detail}
        <p class="relogin-detail">{detail}</p>
      {/if}

      <div class="relogin-actions">
        <button class="btn btn-secondary" type="button" onclick={onWorkOffline} disabled={pending}>
          Work Offline
        </button>
        <button class="btn btn-primary" type="button" onclick={onSignIn} disabled={pending}>
          {pending ? 'Opening…' : 'Sign In Again'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .relogin-overlay {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .relogin-content {
    background: hsl(var(--popover));
    color: hsl(var(--popover-foreground));
    border-radius: 8px;
    padding: 24px;
    width: min(440px, calc(100vw - 48px));
  }

  .relogin-content h2 {
    margin: 0 0 12px 0;
    font-size: 20px;
  }

  .relogin-body {
    margin: 0;
    font-size: 14px;
    line-height: 1.5;
    color: hsl(var(--muted-foreground));
  }

  .relogin-detail {
    margin: 12px 0 0 0;
    padding: 8px 10px;
    background: hsl(var(--muted));
    border-radius: 4px;
    font-size: 12px;
    color: hsl(var(--muted-foreground));
    word-break: break-word;
  }

  .relogin-actions {
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
    opacity: 0.7;
  }

  .btn-primary {
    background-color: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
  }

  .btn-primary:hover:not(:disabled) {
    background-color: hsl(var(--primary-hover));
  }

  .btn-secondary {
    background-color: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .btn-secondary:hover:not(:disabled) {
    background-color: hsl(var(--muted) / 0.7);
  }
</style>
