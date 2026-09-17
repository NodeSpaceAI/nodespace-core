<script lang="ts">
  import { Checkbox as CheckboxPrimitive } from 'bits-ui';
  import CheckIcon from '@lucide/svelte/icons/check';
  import { cn } from '$lib/utils.js';
  import type { CheckboxProps } from './index.js';

  let {
    ref = $bindable(null),
    class: className,
    checked = $bindable(false),
    indeterminate = $bindable(false),
    ...restProps
  }: CheckboxProps = $props();
</script>

<CheckboxPrimitive.Root
  bind:ref
  bind:checked
  bind:indeterminate
  data-slot="checkbox"
  class={cn(
    'peer size-4 shrink-0 rounded-sm border border-primary shadow-xs outline-none',
    // No focus indicator at all, and `outline-none` keeps the user agent's own
    // ring suppressed. This is a DELIBERATE, documented exception to WCAG 2.4.7
    // — see the Focus section in DESIGN.md, which records the reasoning and the
    // cost. It is not an oversight, and it is not a treatment waiting to be
    // filled in; adding one here would contradict a decision, so read that
    // section before changing this line.
    //
    // Why no painted treatment is available: a checkbox is a 16px square whose
    // paintable surfaces are its border and its fill, and the fill already
    // carries checked state. `bg-accent` on the unchecked state measures a
    // healthy 5.19:1 light / 3.75:1 dark against the page, but a CHECKED box
    // gains almost nothing — accent against primary is 1.41:1 — so focus would
    // be visible on empty boxes and invisible on ticked ones. A border-color
    // change fails the same way at 1.41:1, and a border-WIDTH change is the
    // geometry idiom this whole rule exists to prevent.
    //
    // The ring that stood here was `ring-ring/50`: an alpha value composited
    // against whatever sat behind it, measuring 1.84:1 light and 2.28:1 dark
    // against a 3:1 floor. It was not doing the job it appeared to do either.
    'disabled:cursor-not-allowed disabled:opacity-50',
    'data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground data-[state=checked]:border-primary',
    'aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40',
    className
  )}
  {...restProps}
>
  {#snippet children({ checked: isChecked })}
    <span class="flex items-center justify-center text-current">
      <CheckIcon class={cn('size-3.5', !isChecked && 'invisible')} />
    </span>
  {/snippet}
</CheckboxPrimitive.Root>
