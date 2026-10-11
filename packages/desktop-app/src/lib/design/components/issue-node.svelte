<!--
  IssueNode - An issue in the outline

  An issue is a task that is development work, and in the outline it behaves as a task
  does: the same state icon, the same click-to-advance, the same open button. This
  component renders TaskNode for the `issue` type and passes its events on, so every rule
  of a task row holds for an issue row. What sets an issue apart (its review statuses,
  its pull request and commits) is in its properties form, IssueSchemaForm.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import TaskNode from './task-node.svelte';

  // Props using Svelte 5 runes mode - same interface as TaskNode
  let {
    nodeId,
    nodeType = 'issue',
    autoFocus = false,
    content = $bindable(''),
    children = [],
    metadata = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    children?: string[];
    metadata?: Record<string, unknown>;
  } = $props();

  const dispatch = createEventDispatcher();

  /** Pass a TaskNode event on to the parent component. */
  function forwardEvent<T>(eventName: string) {
    return (event: CustomEvent<T>) => dispatch(eventName, event.detail);
  }
</script>

<TaskNode
  {nodeId}
  {nodeType}
  {autoFocus}
  bind:content
  {children}
  {metadata}
  on:taskStateChanged={forwardEvent('taskStateChanged')}
  on:createNewNode={forwardEvent('createNewNode')}
  on:contentChanged={forwardEvent('contentChanged')}
  on:indentNode={forwardEvent('indentNode')}
  on:outdentNode={forwardEvent('outdentNode')}
  on:navigateArrow={forwardEvent('navigateArrow')}
  on:combineWithPrevious={forwardEvent('combineWithPrevious')}
  on:deleteNode={forwardEvent('deleteNode')}
  on:focus={forwardEvent('focus')}
  on:blur={forwardEvent('blur')}
  on:nodeReferenceSelected={forwardEvent('nodeReferenceSelected')}
  on:slashCommandSelected={forwardEvent('slashCommandSelected')}
  on:nodeTypeChanged={forwardEvent('nodeTypeChanged')}
/>
