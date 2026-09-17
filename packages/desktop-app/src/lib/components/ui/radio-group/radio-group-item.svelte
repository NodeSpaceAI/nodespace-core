<script lang="ts">
  import { RadioGroup as RadioGroupPrimitive } from 'bits-ui';
  import CircleIcon from '@lucide/svelte/icons/circle';
  import { cn } from '$lib/utils';

  let {
    ref = $bindable(null),
    class: className,
    ...restProps
  }: RadioGroupPrimitive.ItemProps = $props();
</script>

<RadioGroupPrimitive.Item
  bind:ref
  data-slot="radio-group-item"
  class={cn(
    // No focus treatment, for the same reason as Checkbox and Switch: the dot
    // inside is the selected state, so there is no paintable surface left that
    // does not already mean something else.
    'border-input text-primary aspect-square size-4 rounded-full border shadow-xs focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50',
    className
  )}
  {...restProps}
>
  {#snippet children({ checked })}
    <span class="flex items-center justify-center">
      {#if checked}
        <CircleIcon class="size-2.5 fill-current text-current" />
      {/if}
    </span>
  {/snippet}
</RadioGroupPrimitive.Item>
