<!--
  Task Icon Component

  CSS-based task icon following the exact design system patterns.
  Uses CSS circles instead of SVG for consistent cross-browser rendering.

  States:
  - pending: Empty circle outline (border only)
  - inProgress: Half-filled circle with gradient
  - completed: Filled circle with CSS checkmark

  Features:
  - CSS-based for cross-browser consistency
  - Perfect 16x16px rendering inside 20x20px container
  - Theme-aware checkmark colors
  - Ring effect support for parent tasks
-->

<script lang="ts">
  import type { NodeState } from '../registry.js';

  export let size: number = 20;
  export let state: NodeState = 'pending';
  export let className: string = '';
  export let hasChildren: boolean = false;

  // Resolve display state - treat 'default' as 'pending' for tasks
  $: displayState = state === 'default' ? 'pending' : state;
</script>

<!-- CSS-based task icon following design system exactly -->
<div
  class="task-icon {className}"
  class:task-icon-with-ring={hasChildren}
  style="--circle-diameter: {size}px"
  role="img"
  aria-label="Task {displayState} icon"
>
  {#if hasChildren}
    <!-- Ring effect for parent tasks -->
    <div class="task-ring"></div>
  {/if}

  <!-- Task circle with state-specific styling -->
  <div
    class="task-circle task-circle-{displayState}"
    class:task-circle-pending={displayState === 'pending'}
    class:task-circle-in-progress={displayState === 'inProgress'}
    class:task-circle-completed={displayState === 'completed'}
  ></div>
</div>

<style>
  /* CSS-based task icons following design system exactly */
  .task-icon {
    width: var(--circle-diameter, 20px);
    height: var(--circle-diameter, 20px);
    position: relative;
    display: block;
    flex-shrink: 0;
  }

  .task-icon-with-ring {
    width: calc(var(--circle-diameter, 20px) + 4px);
    height: calc(var(--circle-diameter, 20px) + 4px);
  }

  /* Basic circle states - Perfect 16x16 rendering */
  .task-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
  }

  .task-circle-pending {
    background: transparent;
    border: 1px solid hsl(var(--primary));
    box-sizing: border-box;
  }

  .task-circle-in-progress {
    background: linear-gradient(90deg, hsl(var(--primary)) 50%, transparent 50%);
    border: 1px solid hsl(var(--primary));
    box-sizing: border-box;
  }

  .task-circle-completed {
    background: hsl(var(--primary));
  }

  /* Checkmark for completed tasks - theme-aware stroke color */
  .task-circle-completed::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 10px;
    height: 8px;
    transform: translate(-50%, -50%);
  }

  /* Light theme checkmark - matches #FAF9F5 */
  .task-circle-completed::after {
    background-image: url("data:image/svg+xml,%3Csvg width='10' height='8' viewBox='0 0 10 8' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 4L3.5 6.5L9 1' stroke='%23FAF9F5' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: contain;
  }

  /* Dark theme checkmark - matches #252523 */
  :global(.dark) .task-circle-completed::after {
    background-image: url("data:image/svg+xml,%3Csvg width='10' height='8' viewBox='0 0 10 8' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 4L3.5 6.5L9 1' stroke='%23252523' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: contain;
  }

  /* Parent task rings */
  .task-ring {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 1px solid hsl(var(--primary));
    opacity: 0.5;
    position: absolute;
    top: 0;
    left: 0;
    box-sizing: border-box;
  }
</style>
