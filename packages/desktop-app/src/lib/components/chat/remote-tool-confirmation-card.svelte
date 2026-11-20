<!--
  RemoteToolConfirmationCard - the agent is blocked on a remote tool call that
  needs the user's yes. It is a message in the chat, so what was asked and
  answered stays in the history.

  What it shows, and why:
  - The credentials on their own line, from a separate field, always visible.
  - The destination (the daemon's summary) in full, as plain text: the host is
    never cut. Control, bidi, zero-width and tag characters are shown as visible
    escapes (`toDisplaySafe`), here and in the arguments.
  - The arguments cut to a preview with an expander that shows all of them and
    their real size.
  - Two choices, in NodeSpace's own words: "Don't allow" (the default, focused
    when the card appears; Escape does it too) and "Allow once". Allowing is
    never remembered.

  The answer carries the id and digest the daemon put on the card, unchanged.
  Before it can be answered, the card recomputes the digest of what it SHOWS and
  compares it with the daemon's: a card whose text was changed after the daemon
  wrote it cannot be answered. A card without the daemon's id and digest (another
  device's, or a viewer that cannot answer) is a read-only record. It answers at
  most once; a failed answer is shown and the choices come back.
-->

<script module lang="ts">
  /** How long after it is shown and intact before the choices can be used. */
  export const ENABLE_DELAY_MS = 500;
</script>

<script lang="ts">
  import type { AiChatMessageNode } from '$lib/types/ai-chat-node';
  import {
    argumentPreview,
    confirmationDigest,
    credentialsMatchSummary,
    toDisplaySafe,
    utf8Length
  } from '$lib/utils/confirmation-card';


  let {
    message,
    onAnswer
  }: {
    message: AiChatMessageNode;
    /**
     * Sends the answer: the daemon's request id and digest, unchanged, and the
     * choice. Resolves to whether the daemon accepted it. Absent when this
     * viewer cannot answer: the card is then a record.
     */
    onAnswer?: (_requestId: string, _digest: string, _allow: boolean) => Promise<boolean>;
  } = $props();

  const status = $derived(message.confirmationStatus ?? 'pending');
  const summary = $derived(message.content);
  const argumentsJson = $derived(message.confirmationArguments ?? '');
  const credentials = $derived(message.confirmationCredentials ?? []);
  const requestId = $derived(message.confirmationId);
  const digest = $derived(message.confirmationDigest);

  /** Another device's card has no id or digest: it is a record, not a question. */
  const answerable = $derived(
    status === 'pending' && !!onAnswer && !!requestId && !!digest
  );

  const preview = $derived(argumentPreview(argumentsJson));
  const argumentBytes = $derived(utf8Length(argumentsJson));

  /** Whether the text shown is what the daemon asked; null while being checked. */
  let intact = $state<boolean | null>(null);
  $effect(() => {
    const summaryNow = summary;
    const argumentsNow = argumentsJson;
    const expected = digest;
    const creds = credentials;
    if (!expected) {
      intact = null;
      return;
    }
    intact = null;
    void confirmationDigest(summaryNow, argumentsNow).then((actual) => {
      if (summaryNow === summary && argumentsNow === argumentsJson) {
        intact = actual === expected && credentialsMatchSummary(summaryNow, creds);
      }
    });
  });

  /**
   * The choices are enabled a moment after the card is shown and intact, so a
   * click aimed at something else that lands as the card appears cannot answer
   * it.
   */
  let armed = $state(false);
  $effect(() => {
    if (!(answerable && intact === true)) {
      armed = false;
      return;
    }
    const timer = setTimeout(() => (armed = true), ENABLE_DELAY_MS);
    return () => clearTimeout(timer);
  });

  let answeredFor = $state<string | null>(null);
  const answered = $derived(answeredFor !== null && answeredFor === requestId);
  let failure = $state<string | null>(null);
  let refused = $state(false);

  let denyButton: HTMLElement | undefined = $state();

  const uid = $props.id();
  const titleId = `${uid}-title`;

  // Focus lands on the safe choice, never on Allow, once: a later re-render
  // (the card re-checking itself) does not take it back, and a card that cannot
  // be answered never takes it.
  let focusTaken = false;
  $effect(() => {
    // A disabled button cannot take focus, so this waits for the choices to be armed.
    if (armed && answerable && intact === true && !focusTaken && denyButton) {
      focusTaken = true;
      denyButton.focus();
    }
  });

  async function answer(allow: boolean): Promise<void> {
    if (!armed || !answerable || intact !== true || answered || !onAnswer || !requestId || !digest)
      return;
    const id = requestId;
    answeredFor = id;
    failure = null;
    refused = false;
    try {
      const accepted = await onAnswer(id, digest, allow);
      if (!accepted) refused = true;
    } catch (err) {
      if (answeredFor === id) answeredFor = null;
      failure = err instanceof Error ? err.message : String(err);
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && armed && answerable && intact === true && !answered) {
      event.preventDefault();
      void answer(false);
    }
  }

  const RESOLVED_TEXT: Record<string, string> = {
    allowed: 'You allowed this call. Whether it ran is in the reply below.',
    denied: 'You did not allow this call.',
    expired: 'No answer in time. The call was not run.',
    cancelled: 'The question ended without an answer. The call was not run.'
  };
</script>

<section
  tabindex="-1"
  class="confirmation"
  class:pending={status === 'pending'}
  role={status === 'pending' && answerable ? 'alert' : 'group'}
  aria-labelledby={titleId}
  data-confirmation-status={status}
  data-request-id={requestId ?? undefined}
  onkeydown={onKeydown}
>
  <h3 id={titleId} class="confirmation-title">
    {status === 'pending' ? 'Run a tool?' : 'Tool call'}
  </h3>

  <p class="confirmation-credentials" dir="ltr" data-testid="credentials">
    {#if credentials.length > 0}
      <span class="confirmation-label">Credentials:</span>
      {#each credentials as entry, i (i)}
        <code>{toDisplaySafe(entry)}</code>
      {/each}
    {:else}
      <span class="confirmation-label">No credentials</span>
    {/if}
  </p>

  <p class="confirmation-summary" dir="ltr" data-testid="summary">{toDisplaySafe(summary)}</p>

  <div class="confirmation-arguments">
    <span class="confirmation-label">Arguments ({argumentBytes} bytes)</span>
    <pre class="confirmation-preview" dir="ltr" data-testid="arguments-preview">{toDisplaySafe(
        preview.text
      )}{preview.moreBytes > 0 ? ` … (${preview.moreBytes} more bytes)` : ''}</pre>
    {#if preview.moreBytes > 0}
      <details>
        <summary>Show all arguments</summary>
        <pre class="confirmation-full" dir="ltr" data-testid="arguments-full">{toDisplaySafe(argumentsJson)}</pre>
      </details>
    {/if}
  </div>

  {#if status === 'pending'}
    {#if !answerable}
      <p class="confirmation-note" role="status">
        Waiting on the device that is running this chat.
      </p>
    {:else if intact === false}
      <p class="confirmation-note error" role="alert">
        This question was changed after it was asked and cannot be answered.
      </p>
    {:else}
      <div class="confirmation-actions" role="group" aria-label="Choose a response">
        <button
          type="button"
          class="confirmation-button"
          data-choice="deny"
          bind:this={denyButton}
          disabled={answered || intact !== true || !armed}
          onclick={() => answer(false)}
        >
          Don't allow
        </button>
        <button
          type="button"
          class="confirmation-button allow"
          data-choice="allow"
          disabled={answered || intact !== true || !armed}
          onclick={() => answer(true)}
        >
          Allow once
        </button>
      </div>
      {#if answered && !refused}
        <p class="confirmation-note" role="status">Answer sent.</p>
      {/if}
      {#if refused}
        <p class="confirmation-note error" role="alert">
          The answer was not accepted: the question has already ended.
        </p>
      {/if}
      {#if failure}
        <p class="confirmation-note error" role="alert">
          Could not send the answer: {failure}. You can answer again.
        </p>
      {/if}
    {/if}
  {:else if !requestId}
    <!-- Only a card this device wrote carries an id. A resolved card without one
         came from another device (or was written by something else): its status
         is not shown as something this device's user answered. -->
    <p class="confirmation-note" role="status" data-testid="foreign">
      A question recorded on another device.
    </p>
  {:else}
    <p class="confirmation-note" role="status" data-testid="resolved">
      {RESOLVED_TEXT[status] ?? 'This question has ended.'}
    </p>
  {/if}
</section>

<style>
  .confirmation {
    margin: 0.5rem 1rem;
    padding: 0.75rem 1rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--card));
    color: hsl(var(--card-foreground));
    font-size: 0.875rem;
  }

  .confirmation.pending {
    border: 2px solid hsl(var(--primary));
  }

  .confirmation-title {
    margin: 0 0 0.375rem;
    font-size: 0.9375rem;
    font-weight: 600;
  }

  .confirmation-label {
    margin-right: 0.375rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: hsl(var(--muted-foreground));
  }

  .confirmation-credentials,
  .confirmation-summary,
  .confirmation-preview,
  .confirmation-full {
    unicode-bidi: isolate;
  }

  .confirmation-credentials {
    margin: 0 0 0.5rem;
    word-break: break-word;
  }

  .confirmation-credentials code {
    margin-right: 0.375rem;
    padding: 0 0.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    font-size: 0.8125rem;
  }

  .confirmation-summary {
    margin: 0 0 0.5rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .confirmation-arguments {
    margin: 0 0 0.5rem;
  }

  .confirmation-preview,
  .confirmation-full {
    margin: 0.25rem 0 0;
    padding: 0.375rem 0.5rem;
    border-radius: 0.25rem;
    background: hsl(var(--muted));
    font-size: 0.8125rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .confirmation-full {
    max-height: 16rem;
    overflow: auto;
  }

  details summary {
    cursor: pointer;
    margin-top: 0.25rem;
    font-size: 0.8125rem;
  }

  .confirmation-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .confirmation-button {
    padding: 0.375rem 0.75rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }

  .confirmation-button.allow {
    border-color: hsl(var(--primary));
  }

  .confirmation-button:hover:not(:disabled) {
    background: hsl(var(--accent));
  }

  .confirmation-button:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .confirmation-note {
    margin: 0.5rem 0 0;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .confirmation-note.error {
    color: hsl(var(--destructive));
  }
</style>
