<script lang="ts" context="module">
  export type IconName =
    | 'text'
    | 'circle'
    | 'circleRing'
    | 'chevronRight'
    | 'taskComplete'
    | 'taskIncomplete'
    | 'taskInProgress'
    | 'aiSquare'
    | 'calendar';
</script>

<script lang="ts">
  import { textIcon } from './ui/text';
  import { circleIcon } from './ui/circle';
  import { circleRingIcon } from './ui/circle-ring';
  import { chevronRightIcon } from './ui/chevron-right';
  import { taskCompleteIcon } from './ui/task-complete';
  import { taskIncompleteIcon } from './ui/task-incomplete';
  import { taskInProgressIcon } from './ui/task-in-progress';
  import { aiSquareIcon } from './ui/ai-square';
  import { calendarIcon } from './ui/calendar';

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

  export let name: IconName = 'circle'; // Default to circle icon if none specified
  export let size: number = 20;
  export let className: string = '';
  export let color: string = 'currentColor';

  $: iconPath = iconRegistry[name];
</script>

<svg
  width={size}
  height={size}
  viewBox={name === 'circle' ||
  name === 'circleRing' ||
  name === 'chevronRight' ||
  name === 'taskComplete' ||
  name === 'taskIncomplete' ||
  name === 'taskInProgress' ||
  name === 'aiSquare' ||
  name === 'calendar'
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
  {:else if name === 'taskComplete'}
    <circle
      cx="8"
      cy="8"
      r="7"
      fill="currentColor"
      opacity="0.1"
      stroke="currentColor"
      stroke-width="1"
    />
    <path
      d="M6 8l1.5 1.5L11 6"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      fill="none"
    />
  {:else if name === 'taskIncomplete'}
    <circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" stroke-width="1" opacity="0.6" />
  {:else if name === 'taskInProgress'}
    <circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" stroke-width="1" opacity="0.3" />
    <path d="M8 1 A7 7 0 0 1 8 15 Z" fill="currentColor" opacity="0.6" />
  {:else if name === 'aiSquare'}
    <rect
      x="2"
      y="2"
      width="12"
      height="12"
      rx="2"
      ry="2"
      fill="currentColor"
      opacity="0.1"
      stroke="currentColor"
      stroke-width="1"
    />
    <text
      x="8"
      y="10.5"
      font-family="system-ui, -apple-system, sans-serif"
      font-size="6"
      font-weight="600"
      text-anchor="middle"
      fill="currentColor">AI</text
    >
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
