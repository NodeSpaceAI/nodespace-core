<!--
  ExtensionOutlet — mounts one lazily-loaded UI-extension component.

  Generic host part of the extension API: the app shell (chrome slots) and node
  viewers (viewer tabs) render a registry contribution through it without
  importing the component directly. The dynamic import runs once per `load`
  value, and callers key their {#each} by the contribution key
  (`<extension id>/<contribution id>`) so a contribution that goes away, or
  changes, mounts a fresh outlet (ADR-082 §2.3).

  Isolation (ADR-082 §2.4): a rejected `load()` renders nothing and is logged; it
  is retried only on the next mount, so nothing loops. A component that throws
  while rendering, or in an effect, is caught by the boundary and removes only
  its own outlet; siblings keep rendering. Both are logged.
-->
<script lang="ts" generics="Props extends Record<string, unknown> = Record<string, never>">
  import type { Component } from 'svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ExtensionOutlet');

  let {
    load,
    props = {} as Props
  }: {
    load: () => Promise<{ default: Component<Props> }>;
    props?: Props;
  } = $props();

  // The Promise constructor turns a `load()` that throws synchronously, or returns
  // a plain value, into the same rejection path as a failed import, so neither can
  // escape from the outlet's own block (the boundary below only covers the component).
  // A module without a default component is rejected the same way, so it is logged
  // instead of silently rendering nothing.
  const loaded = $derived(
    new Promise<{ default: Component<Props> }>((resolve) => resolve(load()))
      .then((mod) => {
        if (typeof mod?.default !== 'function') {
          throw new TypeError('load() must resolve to a module with a default component');
        }
        return mod;
      })
      .catch((error: unknown) => {
        log.error('Extension component failed to load', { error });
        return null;
      })
  );
</script>

{#await loaded then mod}
  {#if mod}
    {@const Loaded = mod.default}
    <!-- Inside the await branch, not around it: the branch is built in a promise
         callback, where a boundary further out cannot see a component's throw. -->
    <svelte:boundary onerror={(error) => log.error('Extension component threw', { error })}>
      <Loaded {...props} />
      {#snippet failed()}{/snippet}
    </svelte:boundary>
  {/if}
{/await}
