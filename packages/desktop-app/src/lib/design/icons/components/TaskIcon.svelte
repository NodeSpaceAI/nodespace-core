<!--
  Task Icon Component
  
  Specialized icon for task nodes with state support (pending, inProgress, completed).
  Eliminates the need for separate task icon files and provides clean state management.
  
  States:
  - pending/default: Empty circle (incomplete task)
  - inProgress: Circle with progress indicator
  - completed: Circle with checkmark
  
  Features:
  - State-based rendering without HTML injection
  - Consistent 16x16 viewBox
  - Clean SVG paths for performance
  - Semantic state representation
-->

<script lang="ts">
  import type { NodeState } from '../registry.js';
  
  export let size: number = 20;
  export let color: string = 'currentColor';
  export let state: NodeState = 'pending';
  export let className: string = '';
  
  // Resolve display state - treat 'default' as 'pending' for tasks
  $: displayState = state === 'default' ? 'pending' : state;
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill={color}
  class={`task-icon task-icon--${displayState} ${className}`}
  role="img"
  aria-label="Task {displayState} icon"
>
  {#if displayState === 'completed'}
    <!-- Completed task: filled circle with checkmark -->
    <circle cx="8" cy="8" r="7" fill={color} opacity="0.1" stroke={color} stroke-width="1"/>
    <path 
      d="M6 8l1.5 1.5L11 6" 
      stroke={color} 
      stroke-width="1.5" 
      stroke-linecap="round" 
      stroke-linejoin="round" 
      fill="none"
    />
  {:else if displayState === 'inProgress'}
    <!-- In progress task: circle with progress arc -->
    <circle cx="8" cy="8" r="7" fill={color} opacity="0.1" stroke={color} stroke-width="1"/>
    <!-- Progress arc (roughly 60% complete) -->
    <path 
      d="M 8 1 A 7 7 0 0 1 14.2 5.8" 
      stroke={color} 
      stroke-width="2" 
      stroke-linecap="round"
      fill="none"
    />
    <!-- Center dot to indicate active progress -->
    <circle cx="8" cy="8" r="2" fill={color} opacity="0.6"/>
  {:else}
    <!-- Pending/default task: empty circle outline -->
    <circle 
      cx="8" 
      cy="8" 
      r="7" 
      fill="none" 
      stroke={color} 
      stroke-width="1.5"
    />
  {/if}
</svg>

<style>
  .task-icon {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
  }
  
  /* State-specific styling can be added here if needed */
  .task-icon--pending {
    /* Additional styling for pending state */
  }
  
  .task-icon--inProgress {
    /* Additional styling for in-progress state */
  }
  
  .task-icon--completed {
    /* Additional styling for completed state */
  }
</style>