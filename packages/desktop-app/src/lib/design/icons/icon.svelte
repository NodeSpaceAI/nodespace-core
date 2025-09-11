<script lang="ts">
  import { textIcon } from './ui/text';
  import { circleIcon } from './ui/circle';
  import { circleRingIcon } from './ui/circleRing';
  import { chevronRightIcon } from './ui/chevronRight';
  import { taskCompleteIcon } from './ui/taskComplete';
  import { taskIncompleteIcon } from './ui/taskIncomplete';
  import { taskInProgressIcon } from './ui/taskInProgress';
  import { aiSquareIcon } from './ui/aiSquare';
  import { calendarIcon } from './ui/calendar';

  export type IconName = 'text' | 'circle' | 'circleRing' | 'chevronRight' | 'taskComplete' | 'taskIncomplete' | 'taskInProgress' | 'aiSquare' | 'calendar';

  const iconRegistry: Record<IconName, string> = {
    text: textIcon,
    circle: circleIcon,
    circleRing: circleRingIcon,
    chevronRight: chevronRightIcon,
    taskComplete: taskCompleteIcon,
    taskIncomplete: taskIncompleteIcon,
    taskInProgress: taskInProgressIcon,
    aiSquare: aiSquareIcon,
    calendar: calendarIcon
  };

  export let name: IconName;
  export let size: number = 20;
  export let className: string = '';
  export let color: string = 'currentColor';

  $: iconPath = iconRegistry[name];
</script>

<svg
  width={size}
  height={size}
  viewBox={name === 'circle' || name === 'circleRing' || name === 'chevronRight' || name === 'taskComplete' || name === 'taskIncomplete' || name === 'taskInProgress' || name === 'aiSquare' || name === 'calendar'
    ? '0 0 24 24' 
    : '0 -960 960 960'}
  fill={color}
  stroke={color}
  stroke-width="2"
  class={`ns-icon ns-icon--${name} ${className}`}
  role="img"
  aria-label={`${name} icon`}
>
  {#if name === 'circle'}
    <circle cx="12" cy="12" r="8" fill={color} />
  {:else if name === 'circleRing'}
    <circle cx="12" cy="12" r="10" fill={color} opacity="0.5" />
    <circle cx="12" cy="12" r="8" fill={color} />
  {:else if name === 'calendar'}
    <path d={iconPath} fill="none" stroke={color} stroke-width="2" />
  {:else if name === 'taskComplete' || name === 'taskIncomplete' || name === 'taskInProgress' || name === 'aiSquare'}
    {@html iconPath}
  {:else}
    <path d={iconPath} fill={color} />
  {/if}
</svg>

<style>
  .ns-icon {
    display: inline-block;
    flex-shrink: 0;
    vertical-align: middle;
  }
</style>