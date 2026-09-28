<!--
  Minimal BaseNode stand-in for tests that mount a `*Node` wrapper component
  (e.g. code-block-node.svelte) without dragging in the real BaseNode's Tauri-backed
  services, autocomplete, slash commands and calendar UI — none of that is under
  test here. See pane-content-reconnect-hydration.test.ts for the established
  precedent of stubbing sibling `.svelte` components with `vi.mock`; this fixture
  goes one step further by also dispatching `contentChanged`, so a test can drive
  the wrapper's real content-update handler the same way the real BaseNode does.
-->
<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  let { nodeId, content = '' }: { nodeId: string; content?: string } = $props();

  const dispatch = createEventDispatcher();
</script>

<div data-testid="fake-base-node" data-node-id={nodeId}>{content}</div>
<button
  type="button"
  data-testid="simulate-local-edit"
  onclick={() => dispatch('contentChanged', { content: `${content} // edited` })}
>
  simulate local edit
</button>
