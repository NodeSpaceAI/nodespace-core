<!--
  Checkbox Icon Component

  CSS-based checkbox icon distinguishable from the circular task icon.
  Uses a rounded square instead of a circle to match native checkbox semantics.

  States:
  - pending: Empty rounded-square outline (unchecked)
  - completed: Filled rounded-square with checkmark (checked)

  Features:
  - Rounded square (border-radius: 3px) to distinguish from task circles
  - Same sizing container as task-icon for layout consistency
  - Theme-aware checkmark colors
  - Ring effect via outline on the square (no gap, no separate element)
-->

<script lang="ts">
  import type { NodeState } from '../registry.js';

  export let size: number = 20;
  export let state: NodeState = 'pending';
  export let className: string = '';
  export let hasChildren: boolean = false;

  // Treat 'default' and 'inProgress' as 'pending' — checkboxes are binary
  $: displayState = state === 'completed' ? 'completed' : 'pending';
</script>

<div
  class="checkbox-icon {className}"
  style="--icon-size: {size}px"
  role="img"
  aria-label="Checkbox {displayState === 'completed' ? 'checked' : 'unchecked'} icon"
>
  <div
    class="checkbox-square"
    class:checkbox-square-unchecked={displayState === 'pending'}
    class:checkbox-square-checked={displayState === 'completed'}
    class:checkbox-square-with-ring={hasChildren}
  ></div>
</div>

<style>
  .checkbox-icon {
    width: var(--icon-size, 20px);
    height: var(--icon-size, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  /* Rounded square — 14x14 centered in the 20px container */
  .checkbox-square {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    position: absolute;
    top: 3px;
    left: 3px;
    box-sizing: border-box;
  }

  .checkbox-square-unchecked {
    background: transparent;
    border: 1.5px solid hsl(var(--primary));
  }

  .checkbox-square-checked {
    background: hsl(var(--primary));
    border: 1.5px solid hsl(var(--primary));
  }

  /* Ring via outline — sits directly on the square's border, no gap */
  .checkbox-square-with-ring {
    outline: 2px solid hsl(var(--primary) / 0.5);
    outline-offset: 0;
    border-radius: 3px;
  }

  /* Checkmark for checked state — light theme */
  .checkbox-square-checked::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 9px;
    height: 7px;
    transform: translate(-50%, -50%);
    background-repeat: no-repeat;
    background-position: center;
    background-size: contain;
    background-image: url("data:image/svg+xml,%3Csvg width='9' height='7' viewBox='0 0 9 7' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 3.5L3.5 6L8 1' stroke='%23FAF9F5' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  }

  /* Dark theme checkmark */
  :global(.dark) .checkbox-square-checked::after {
    background-image: url("data:image/svg+xml,%3Csvg width='9' height='7' viewBox='0 0 9 7' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 3.5L3.5 6L8 1' stroke='%23252523' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  }
</style>
