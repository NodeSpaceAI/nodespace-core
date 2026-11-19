<!--
  AcpPermissionCard - the agent is blocked on a permission request.

  Shows only the once-only choices (allow once, reject once) and a Cancel that
  answers `cancelled`. An "always" choice makes the agent write a standing rule
  into its own settings, so it is never offered here. The card is a live
  region (`role="alert"`) so it is announced when it appears, every choice is a
  native button named in words, and it answers at most once per request.
-->

<script lang="ts">
  import type { AcpPermissionOutcome, AcpPermissionRequest } from '$lib/types/acp-session';
  import {
    onceOnlyOptions,
    permissionOptionKindLabel,
    toolKindLabel
  } from '$lib/utils/acp-chat';

  let {
    request,
    onAnswer
  }: {
    request: AcpPermissionRequest;
    /** Absent when the session cannot take an answer: the choices are shown disabled. */
    onAnswer?: (_requestId: string, _outcome: AcpPermissionOutcome) => void | Promise<void>;
  } = $props();

  const options = $derived(onceOnlyOptions(request.options));

  /** The request already answered from this card; a new request starts fresh. */
  let answeredId = $state<string | null>(null);
  const answered = $derived(answeredId === request.requestId);

  const uid = $props.id();
  const titleId = `${uid}-title`;
  const detailId = `${uid}-detail`;

  function answer(outcome: AcpPermissionOutcome): void {
    if (answered || !onAnswer) return;
    answeredId = request.requestId;
    void onAnswer(request.requestId, outcome);
  }
</script>

<section
  class="acp-permission"
  role="alert"
  aria-labelledby={titleId}
  aria-describedby={detailId}
  data-request-id={request.requestId}
>
  <h3 id={titleId} class="acp-permission-title">Permission needed</h3>
  <p id={detailId} class="acp-permission-detail">
    <span class="acp-permission-kind">{toolKindLabel(request.toolKind)}</span>
    {request.toolTitle}
  </p>
  {#if options.length === 0}
    <p class="acp-permission-note">The agent offered no one-time choice. You can cancel the request.</p>
  {/if}
  <div class="acp-permission-actions" role="group" aria-label="Choose a response">
    {#each options as option (option.optionId)}
      <button
        type="button"
        class="acp-permission-button"
        class:allow={option.kind === 'allow_once'}
        data-option-id={option.optionId}
        disabled={answered || !onAnswer}
        onclick={() => answer({ selected: option.optionId })}
      >
        <span class="acp-permission-choice">{permissionOptionKindLabel(option.kind)}</span>
        {#if option.name.trim().toLowerCase() !== permissionOptionKindLabel(option.kind).toLowerCase()}
          <span class="acp-permission-name">{option.name}</span>
        {/if}
      </button>
    {/each}
    <button
      type="button"
      class="acp-permission-button"
      data-option-id="cancel"
      disabled={answered || !onAnswer}
      onclick={() => answer('cancelled')}
    >
      <span class="acp-permission-choice">Cancel request</span>
    </button>
  </div>
  {#if answered}
    <p class="acp-permission-note">Answer sent.</p>
  {/if}
</section>

<style>
  .acp-permission {
    margin: 0.5rem 1rem;
    padding: 0.75rem 1rem;
    border: 2px solid hsl(var(--primary));
    border-radius: 0.5rem;
    background: hsl(var(--card));
    color: hsl(var(--card-foreground));
    font-size: 0.875rem;
  }

  .acp-permission-title {
    margin: 0 0 0.25rem;
    font-size: 0.9375rem;
    font-weight: 600;
  }

  .acp-permission-detail {
    margin: 0 0 0.5rem;
    word-break: break-word;
  }

  .acp-permission-kind {
    display: inline-block;
    margin-right: 0.375rem;
    padding: 0 0.375rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-permission-note {
    margin: 0 0 0.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .acp-permission-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .acp-permission-button {
    display: inline-flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 0.375rem 0.75rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font: inherit;
    cursor: pointer;
  }

  .acp-permission-button.allow {
    border-color: hsl(var(--primary));
  }

  .acp-permission-button:hover:not(:disabled) {
    background: hsl(var(--accent));
  }

  .acp-permission-button:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .acp-permission-choice {
    font-weight: 600;
  }

  .acp-permission-name {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }
</style>
