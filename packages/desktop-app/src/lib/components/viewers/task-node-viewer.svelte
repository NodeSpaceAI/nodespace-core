<!--
  TaskNodeViewer - Page-level viewer for task nodes with task-specific form

  Features:
  - Wraps BaseNodeViewer for node management
  - BaseNodeViewer automatically loads TaskSchemaForm via plugin registry
  - Direct type-specific property binding (status, priority, dueDate)
  - Type-safe updates via updateTaskNode
  - Follows *NodeViewer pattern (like DateNodeViewer)

  Root cause: the generic schema-driven form has no way to express TaskNode's strongly-typed
  business logic (optimistic apply, immediate persist, OCC conflict handling via
  sharedNodeStore.updateTaskNode).
  Solution: TaskSchemaForm registered in taskNodePlugin, loaded automatically by BaseNodeViewer
-->

<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';

  // Props using Svelte 5 runes mode
  // onNodeIdChange provided for future navigation features
  let {
    nodeId
  }: {
    nodeId: string;
    onNodeIdChange?: (_nodeId: string) => void;
  } = $props();
</script>

<div class="task-node-viewer">
  <!-- BaseNodeViewer automatically loads TaskSchemaForm from plugin registry -->
  <BaseNodeViewer {nodeId} />
</div>

<style>
  .task-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
</style>
