<script lang="ts">
  import { Calendar as CalendarPrimitive, type WithoutChildrenOrChild } from 'bits-ui';
  import * as Calendar from './index.js';
  import { cn } from '$lib/utils.js';
  import type { DateValue } from '@internationalized/date';

  let {
    ref = $bindable(null),
    value = $bindable(),
    placeholder = $bindable(),
    onValueChange,
    type,
    class: className,
    weekdayFormat = 'short',
    ...restProps
  }: WithoutChildrenOrChild<CalendarPrimitive.RootProps> = $props();

  /**
   * Value Binding Workaround Pattern
   *
   * PROBLEM: bits-ui CalendarPrimitive.Root doesn't properly propagate bindable value
   * changes in Svelte 5. Direct bind:value={value} doesn't trigger parent updates.
   *
   * SOLUTION: Bridge pattern with intermediate state
   * - internalValue: Local reactive state that bits-ui can bind to
   * - $effect: Syncs external value changes to internal (one-way: parent → child)
   * - handleValueChange: Syncs internal changes to external (one-way: child → parent)
   *
   * This pattern ensures proper two-way binding despite the library limitation.
   * The 'as never' casts are necessary due to discriminated union type conflicts
   * between CalendarPrimitive's type parameter and our generic value binding.
   *
   * TODO: Revisit when bits-ui has full Svelte 5 support or consider upstream PR
   * Related: https://github.com/huntabyte/bits-ui/issues (Svelte 5 compatibility)
   */
  let internalValue = $state<DateValue | DateValue[] | undefined>(value);

  // Sync external value changes to internal
  $effect(() => {
    internalValue = value;
  });

  // Handle internal changes and propagate them
  function handleValueChange(v: DateValue | DateValue[] | undefined) {
    // Update the bindable prop
    value = v;
    // Call the user's callback if provided
    if (onValueChange) {
      onValueChange(v as never);
    }
  }
</script>

<!--
Discriminated Unions + Destructing (required for bindable) do not
get along, so we shut typescript up by casting `value` to `never`.
-->
<CalendarPrimitive.Root
  bind:value={internalValue as never}
  bind:ref
  bind:placeholder
  onValueChange={handleValueChange}
  {type}
  {weekdayFormat}
  class={cn('p-3', className)}
  {...restProps}
>
  {#snippet children({ months, weekdays })}
    <Calendar.Header>
      <Calendar.PrevButton />
      <Calendar.Heading />
      <Calendar.NextButton />
    </Calendar.Header>
    <Calendar.Months>
      {#each months as month (month)}
        <Calendar.Grid>
          <Calendar.GridHead>
            <Calendar.GridRow class="flex">
              {#each weekdays as weekday (weekday)}
                <Calendar.HeadCell>
                  {weekday.slice(0, 2)}
                </Calendar.HeadCell>
              {/each}
            </Calendar.GridRow>
          </Calendar.GridHead>
          <Calendar.GridBody>
            {#each month.weeks as weekDates (weekDates)}
              <Calendar.GridRow class="mt-2 w-full">
                {#each weekDates as date (date)}
                  <Calendar.Cell {date} month={month.value}>
                    <Calendar.Day />
                  </Calendar.Cell>
                {/each}
              </Calendar.GridRow>
            {/each}
          </Calendar.GridBody>
        </Calendar.Grid>
      {/each}
    </Calendar.Months>
  {/snippet}
</CalendarPrimitive.Root>
