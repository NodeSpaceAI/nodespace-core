<!--
  PlayNodeViewer - Page-level viewer for a play (ADR-090 §2)

  Shows a play read-only, except for its switch:
  - Header: title, description, state (on, off, or suspended with its message
    and time) and the on/off switch
  - Body: one lane per rule, in order: the trigger described from its fields,
    each condition's and action's authored description, and an invariant
    marker. A step's raw content (the CEL, the params) shows on demand.

  The play is read from SharedNodeStore on every render and never copied
  (ADR-049), so a write from a chat, the CLI, a sync or an engine suspension
  redraws the view. Rules are changed through a chat bound to the play, not
  here.

  Named as a *NodeViewer for page-level-viewer consistency, but renders its own
  layout directly rather than wrapping BaseNodeViewer (like CollectionNodeViewer).
-->

<script lang="ts">
  import { Switch } from '$lib/components/ui/switch';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { isA } from '$lib/types/core-node-types';
  import type { PlayNode } from '$lib/types';
  import {
    actionForEach,
    describeTrigger,
    playState,
    playTitle,
    rawAction,
    rawTrigger,
    rulesWarnedOnDisable
  } from '$lib/components/play/play-node-model';

  // Props using Svelte 5 runes mode - unified NodeViewerProps interface.
  // The tab title is derived from the node's content by tab-system.svelte — no push here.
  let {
    nodeId
  }: {
    nodeId: string;
  } = $props();

  // The store's node is the play: no local copy (ADR-049).
  const play = $derived.by(() => {
    const node = sharedNodeStore.getNode(nodeId);
    return node && isA(node.nodeType, 'play') ? (node as unknown as PlayNode) : undefined;
  });
  const playStatus = $derived(play ? playState(play) : 'off');
  const warnedRules = $derived(play ? rulesWarnedOnDisable(play) : []);

  // Whether the switch-off warning is open. View state, not play state.
  let confirmingDisable = $state(false);

  function writeEnabled(enabled: boolean) {
    // Switching a suspended play on sends `enabled: true` although it is
    // already stored: that write is what clears the suspension.
    sharedNodeStore.updatePlayNode(
      nodeId,
      { enabled },
      { type: 'viewer', viewerId: 'play-node-viewer' }
    );
  }

  function handleSwitch(on: boolean) {
    if (!on && warnedRules.length > 0) {
      confirmingDisable = true;
      return;
    }
    writeEnabled(on);
  }

  function confirmDisable() {
    confirmingDisable = false;
    writeEnabled(false);
  }

  function cancelDisable() {
    confirmingDisable = false;
  }

  function formatTime(timestamp: string): string {
    const date = new Date(timestamp);
    return Number.isNaN(date.getTime()) ? timestamp : date.toLocaleString();
  }
</script>

<div class="play-node-viewer">
  {#if play}
    {@const title = playTitle(play)}
    <div class="play-header">
      <div class="play-title">
        <h1>{title}</h1>
        <span class="play-state" data-state={playStatus}>
          {playStatus === 'on' ? 'On' : playStatus === 'off' ? 'Off' : 'Suspended'}
        </span>
        <Switch
          class="play-switch"
          aria-label="Play on"
          bind:checked={() => playStatus === 'on', handleSwitch}
        />
      </div>

      {#if play.description}
        <p class="play-description">{play.description}</p>
      {/if}

      {#if playStatus === 'suspended' && play.suspendedAt}
        <div class="play-suspension" role="status">
          <p class="suspension-title">
            Suspended on this device since {formatTime(play.suspendedAt)}
          </p>
          {#if play.suspendedMessage}
            <p class="suspension-message">{play.suspendedMessage}</p>
          {/if}
          <p class="suspension-hint">Turn the play on to run it again.</p>
        </div>
      {/if}
    </div>

    <div class="play-content">
      {#if play.rules.length === 0}
        <p class="empty-state">This play has no rules.</p>
      {:else}
        <ol class="rule-list">
          <!-- Keyed by position: a play written around validation can repeat a
               rule name, and it still has to render. -->
          {#each play.rules as rule, ruleIndex (ruleIndex)}
            <li class="rule-lane" aria-label={rule.name}>
              <div class="rule-heading">
                <h2>{rule.name}</h2>
                {#if rule.class === 'invariant'}
                  <span
                    class="invariant-marker"
                    title="Runs inside the write that triggers it, and can refuse it"
                  >
                    invariant
                  </span>
                {/if}
              </div>
              <p class="rule-description">{rule.description}</p>

              <ol class="step-list">
                <li class="step" data-step="trigger">
                  <span class="step-label">When</span>
                  <div class="step-body">
                    <p class="step-description">{describeTrigger(rule.trigger)}</p>
                    <details class="step-raw">
                      <summary>Show trigger</summary>
                      <pre>{rawTrigger(rule.trigger)}</pre>
                    </details>
                  </div>
                </li>

                {#each rule.conditions ?? [] as condition, index (index)}
                  <li class="step" data-step="condition">
                    <span class="step-label">{index === 0 ? 'If' : 'And'}</span>
                    <div class="step-body">
                      <p class="step-description">{condition.description}</p>
                      <details class="step-raw">
                        <summary>Show expression</summary>
                        <pre>{condition.expr}</pre>
                      </details>
                    </div>
                  </li>
                {/each}

                {#each rule.actions ?? [] as action, index (index)}
                  {@const forEach = actionForEach(action)}
                  <li class="step" data-step="action">
                    <span class="step-label">{index === 0 ? 'Then' : 'And'}</span>
                    <div class="step-body">
                      <p class="step-description">{action.description}</p>
                      {#if forEach}
                        <p class="step-for-each">For each of <code>{forEach}</code></p>
                      {/if}
                      <details class="step-raw">
                        <summary>Show action</summary>
                        <pre>{rawAction(action)}</pre>
                      </details>
                    </div>
                  </li>
                {/each}
              </ol>
            </li>
          {/each}
        </ol>
      {/if}
    </div>

    <Dialog.Root bind:open={confirmingDisable}>
      <Dialog.Content class="sm:max-w-md">
        <Dialog.Header>
          <Dialog.Title>Turn off {title}?</Dialog.Title>
          <!-- The rules and the consequence are part of the description, so a
               screen reader announces what the warning names, not only its lead-in. -->
          <Dialog.Description>
            <p>
              This play ships with NodeSpace and enforces
              {warnedRules.length === 1 ? 'a rule' : 'rules'} on every write. While it is off, nothing
              enforces:
            </p>
            <ul class="warned-rules">
              {#each warnedRules as rule, index (index)}
                <li>{rule.description}</li>
              {/each}
            </ul>
            <p>
              Nodes written from now on are no longer held to
              {warnedRules.length === 1 ? 'it' : 'them'}. Nodes already written keep what they got.
            </p>
          </Dialog.Description>
        </Dialog.Header>

        <Dialog.Footer>
          <Button variant="outline" onclick={cancelDisable}>Cancel</Button>
          <Button variant="destructive" onclick={confirmDisable}>Turn off</Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
  {:else}
    <p class="empty-state">This play is not available.</p>
  {/if}
</div>

<style>
  .play-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .play-header {
    padding: 1.5rem 2rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  .play-title {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .play-title h1 {
    font-size: 1.5rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .play-state {
    margin-left: auto;
    flex-shrink: 0;
    font-size: 0.75rem;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
  }

  .play-state[data-state='on'] {
    color: hsl(var(--success));
    background: hsl(var(--success) / 0.1);
  }

  .play-state[data-state='suspended'] {
    color: hsl(var(--warning));
    background: hsl(var(--warning) / 0.1);
  }

  .play-title :global(.play-switch) {
    flex-shrink: 0;
  }

  .play-description {
    margin: 0.5rem 0 0;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .play-suspension {
    margin-top: 1rem;
    padding: 0.75rem 1rem;
    border: 1px solid hsl(var(--warning) / 0.4);
    border-radius: 0.375rem;
    background: hsl(var(--warning) / 0.1);
    font-size: 0.8125rem;
  }

  .play-suspension p {
    margin: 0;
  }

  .suspension-title {
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .suspension-message {
    margin-top: 0.25rem;
    color: hsl(var(--foreground));
    overflow-wrap: anywhere;
  }

  .play-suspension .suspension-message,
  .play-suspension .suspension-hint {
    margin-top: 0.25rem;
  }

  .suspension-hint {
    color: hsl(var(--muted-foreground));
  }

  .play-content {
    flex: 1;
    overflow-y: auto;
    padding: 1.5rem 2rem;
  }

  .empty-state {
    padding: 3rem;
    text-align: center;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .rule-list,
  .step-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .rule-list {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .rule-lane {
    padding: 1rem 1.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--background));
  }

  .rule-heading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .rule-heading h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .invariant-marker {
    font-size: 0.6875rem;
    padding: 0.05rem 0.4rem;
    border-radius: 0.25rem;
    color: hsl(var(--warning));
    background: hsl(var(--warning) / 0.1);
  }

  .rule-description {
    margin: 0.25rem 0 0.75rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .step {
    display: flex;
    gap: 0.75rem;
    padding: 0.5rem 0;
    border-top: 1px solid hsl(var(--border));
  }

  .step-label {
    flex: 0 0 3rem;
    font-size: 0.75rem;
    font-weight: 600;
    line-height: 1.25rem;
    color: hsl(var(--muted-foreground));
  }

  .step-body {
    flex: 1;
    min-width: 0;
  }

  .step-description {
    margin: 0;
    font-size: 0.875rem;
    line-height: 1.25rem;
    color: hsl(var(--foreground));
  }

  .step-for-each {
    margin: 0.125rem 0 0;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .step-raw {
    margin-top: 0.25rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }

  .step-raw summary {
    cursor: pointer;
    width: fit-content;
  }

  .step-raw pre,
  .step-for-each code {
    font-family: ui-monospace, monospace;
    color: hsl(var(--foreground));
    background: hsl(var(--muted));
    border-radius: 0.25rem;
  }

  .step-for-each code {
    padding: 0.05rem 0.25rem;
  }

  .step-raw pre {
    margin: 0.375rem 0 0;
    padding: 0.5rem 0.75rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .warned-rules {
    margin: 0.5rem 0;
    padding-left: 1.25rem;
    text-align: left;
    color: hsl(var(--foreground));
  }
</style>
