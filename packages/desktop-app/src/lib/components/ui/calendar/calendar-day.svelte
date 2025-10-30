<script lang="ts">
  import { Calendar as CalendarPrimitive } from 'bits-ui';
  import { cn } from '$lib/utils.js';

  let {
    ref = $bindable(null),
    onclick,
    class: className,
    ...restProps
  }: CalendarPrimitive.DayProps = $props();

  // Intercept click events for debugging
  function handleClick(event: MouseEvent) {
    console.log('[CalendarDay] Date clicked!', event);
    // Call the original onclick handler if provided
    if (onclick) {
      onclick(event as any);
    }
  }
</script>

<CalendarPrimitive.Day
  class={cn(
    'size-8 p-0 font-normal border border-transparent rounded-md inline-flex items-center justify-center',
    // Hover - subtle muted background
    'hover:bg-muted',
    // Today
    '[&[data-today]:not([data-selected])]:border-muted-foreground/30',
    // Selected - subtle border with muted background (same as hover)
    'data-[selected]:border-border data-[selected]:bg-muted data-[selected]:text-foreground data-[selected]:opacity-100',
    'data-[selected]:hover:bg-muted data-[selected]:hover:border-border',
    'data-[selected]:focus-visible:bg-muted data-[selected]:focus-visible:border-border data-[selected]:focus-visible:outline-none',
    // Disabled
    'data-[disabled]:text-muted-foreground data-[disabled]:opacity-50 data-[disabled]:hover:bg-transparent',
    // Unavailable
    'data-[unavailable]:text-destructive-foreground data-[unavailable]:line-through',
    // Outside months
    'data-[outside-month]:text-muted-foreground [&[data-outside-month][data-selected]]:border-muted-foreground/30 [&[data-outside-month][data-selected]]:text-muted-foreground data-[outside-month]:pointer-events-none data-[outside-month]:opacity-50 [&[data-outside-month][data-selected]]:opacity-30',
    className
  )}
  bind:ref
  onclick={handleClick}
  {...restProps}
/>
