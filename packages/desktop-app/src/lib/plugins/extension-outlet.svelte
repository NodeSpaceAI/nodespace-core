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

  const loaded = $derived(
    load().catch((error: unknown) => {
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
