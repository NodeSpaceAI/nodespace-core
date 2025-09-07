<script context="module" lang="ts">
  // Re-export types from registry for better TypeScript support
  export type { NodeType, NodeState, NodeIconProps } from './registry.js';
  // Legacy export for backward compatibility during transition
  export type { IconName } from './types.js';
</script>

<script lang="ts">
  import { getIconConfig, resolveNodeState, type NodeType, type NodeState } from './registry.js';
  import type { IconName } from './types.js';
  
  // Legacy icon imports for backward compatibility
  import { textIcon } from './ui/text';
  import { circleIcon } from './ui/circle';
  import { circleRingIcon } from './ui/circleRing';
  import { chevronRightIcon } from './ui/chevronRight';
  import { taskCompleteIcon } from './ui/taskComplete';
  import { taskIncompleteIcon } from './ui/taskIncomplete';
  import { taskInProgressIcon } from './ui/taskInProgress';
  import { aiSquareIcon } from './ui/aiSquare';

  // Legacy icon registry for backward compatibility
  const legacyIconRegistry: Record<IconName, string> = {
    text: textIcon,
    circle: circleIcon,
    circleRing: circleRingIcon,
    chevronRight: chevronRightIcon,
    taskComplete: taskCompleteIcon,
    taskIncomplete: taskIncompleteIcon,
    taskInProgress: taskInProgressIcon,
    aiSquare: aiSquareIcon
  };

  // New semantic props (preferred)
  export let nodeType: NodeType | undefined = undefined;
  export let state: NodeState | undefined = undefined;
  export let hasChildren: boolean = false;
  
  // Legacy props (backward compatibility)
  export let name: IconName | undefined = undefined;
  
  // Common props
  export let size: number = 20;
  export let className: string = '';
  export let color: string = 'currentColor';

  // Determine which mode we're operating in
  $: isSemanticMode = nodeType !== undefined;
  
  // Get icon configuration for semantic mode
  $: iconConfig = isSemanticMode ? getIconConfig(nodeType!) : null;
  $: resolvedState = isSemanticMode ? resolveNodeState(nodeType!, state) : 'default';
  
  // Legacy mode support
  $: legacyIconPath = !isSemanticMode && name ? legacyIconRegistry[name] : null;
  
  // Validation - silently fallback for missing legacy icons
  
  // Derive semantic class
  $: semanticClass = iconConfig?.semanticClass || 'node-icon';
  
  // Derive effective color
  $: effectiveColor = color === 'currentColor' && iconConfig ? iconConfig.colorVar : color;
</script>

<!-- 
  Smart Icon Component with Registry-Based Architecture
  
  Features:
  - Semantic node-based API: <Icon nodeType="text" hasChildren={true} />
  - Component-based rendering (no HTML injection)
  - Design system integration with semantic classes
  - Backward compatibility with legacy name-based API
  - Type-safe with full TypeScript support
-->

{#if isSemanticMode}
  <!-- New semantic mode: direct component rendering without wrapper -->
  <svelte:component 
    this={iconConfig.component} 
    {size} 
    color={effectiveColor}
    {hasChildren}
    state={resolvedState}
    className={`${semanticClass} ${className}`}
  />
{:else if name}
  <!-- Legacy mode: backward compatibility -->
  <svg
    width={size}
    height={size}
    viewBox={name === 'circle' || name === 'circleRing' || name === 'chevronRight' || name === 'taskComplete' || name === 'taskIncomplete' || name === 'taskInProgress' || name === 'aiSquare'
      ? '0 0 16 16'
      : '0 -960 960 960'}
    fill={color}
    class={`ns-icon ns-icon--${name} ${className}`}
    role="img"
    aria-label={`${name} icon`}
  >
    {#if legacyIconPath}
      {#if name === 'circle'}
        <!-- Simple filled circle: both circles always present, ring transparent -->
        <!-- Background ring: 16px diameter (transparent when no children) -->
        <circle cx="8" cy="8" r="8" fill={color} opacity="0" />
        <!-- Inner circle: 11px diameter (r=5.5) -->
        <circle cx="8" cy="8" r="5.5" fill={color} />
      {:else if name === 'circleRing'}
        <!-- Layered circles: 16px parent ring + 11px inner circle -->
        <!-- Background ring: 16px diameter filled area -->
        <circle cx="8" cy="8" r="8" fill={color} opacity="0.5" />
        <!-- Inner circle: 11px diameter (r=5.5) -->
        <circle cx="8" cy="8" r="5.5" fill={color} />
      {:else if name === 'chevronRight'}
        <!-- Chevron right: 16x16 viewBox, points right by default -->
        <path d={legacyIconPath} fill={color} />
      {:else if name === 'taskComplete' || name === 'taskIncomplete' || name === 'taskInProgress' || name === 'aiSquare'}
        <!-- Task and AI icons: use innerHTML for complex SVG -->
        {@html legacyIconPath}
      {:else}
        <path d={legacyIconPath} />
      {/if}
    {/if}
  </svg>
{:else}
  <!-- Error state: neither semantic nor legacy props provided -->
  <div class={`ns-icon ns-icon--error ${className}`} title="Icon configuration error">
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor">
      <circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1"/>
      <text x="8" y="11" font-size="8" text-anchor="middle" fill="currentColor">?</text>
    </svg>
  </div>
{/if}

<style>
  .ns-icon {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
  }

  /* Icon-specific styling can be added here for each icon type */
</style>
