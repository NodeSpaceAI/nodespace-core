<script lang="ts">
  import { Tabs as TabsPrimitive } from 'bits-ui';
  import { cn } from '$lib/utils';

  let { ref = $bindable(null), class: className, ...restProps }: TabsPrimitive.ContentProps = $props();
</script>

<TabsPrimitive.Content
  bind:ref
  data-slot="tabs-content"
  class={cn(
    // A tab panel is a scroll container, not a control: it takes focus only so
    // the keyboard can scroll it, and there is nothing here for `Enter` to
    // activate. Repainting a whole pane would be a flash of color rather than a
    // focus hint, so this one drops its ring and gains nothing in its place.
    'mt-2 focus-visible:outline-none',
    className
  )}
  {...restProps}
/>
