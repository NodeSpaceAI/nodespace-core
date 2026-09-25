<script lang="ts">
  import { Switch as SwitchPrimitive } from 'bits-ui';
  import { cn } from '$lib/utils';
  import type { SwitchProps } from './index.js';

  let {
    ref = $bindable(null),
    class: className,
    ...restProps
  }: SwitchProps = $props();
</script>

<SwitchPrimitive.Root
  bind:ref
  data-slot="switch"
  class={cn(
    // No focus indicator, and `focus-visible:outline-none` keeps the UA ring
    // suppressed. Deliberate documented exception to WCAG 2.4.7 — see Checkbox
    // for the measured reasoning and the Focus section in DESIGN.md for the
    // decision. The track's fill is already how on/off is shown, so borrowing
    // it for focus would read as the switch having been toggled.
    'peer inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent shadow-xs transition-colors focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:bg-primary data-[state=unchecked]:bg-switch-track',
    className
  )}
  {...restProps}
>
  <SwitchPrimitive.Thumb
    data-slot="switch-thumb"
    class={cn(
      'pointer-events-none block size-4 rounded-full bg-background shadow-sm ring-0 transition-transform data-[state=checked]:translate-x-4 data-[state=unchecked]:translate-x-0'
    )}
  />
</SwitchPrimitive.Root>
