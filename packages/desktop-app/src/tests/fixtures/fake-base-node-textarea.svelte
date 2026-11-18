<!--
  BaseNode stand-in with a real textarea, for tests that drive keyboard handling in a
  `*Node` wrapper (quote-block-node.svelte). It mirrors the real controller's relevant
  contract: an `input` event on the textarea dispatches `contentChanged` with its value.
-->
<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  let { content = $bindable('') }: { content?: string } = $props();

  const dispatch = createEventDispatcher();
</script>

<textarea
  data-testid="fake-textarea"
  value={content}
  oninput={(e) => dispatch('contentChanged', { content: e.currentTarget.value })}
></textarea>
