<!--
  TaskNodeViewer - Page-level viewer for task nodes with task-specific form

  Features:
  - Wraps BaseNodeViewer for node management
  - BaseNodeViewer automatically loads TaskSchemaForm via plugin registry (Issue #709)
  - Direct spoke field binding (status, priority, dueDate, assignee)
  - Type-safe updates via updateTaskNode
  - Follows *NodeViewer pattern (like DateNodeViewer)

  Root Cause: Schema-driven SchemaPropertyForm incompatible with strongly-typed TaskNode
  Solution: TaskSchemaForm registered in taskNodePlugin, loaded automatically by BaseNodeViewer
-->

<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';

  // Props using Svelte 5 runes mode
  // onNodeIdChange and onTitleChange provided for future navigation features
  let {
    nodeId
  }: {
    nodeId: string;
    onNodeIdChange?: (_nodeId: string) => void;
    onTitleChange?: (_title: string) => void;
  } = $props();
</script>

<div class="task-node-viewer">
  <!-- BaseNodeViewer automatically loads TaskSchemaForm from plugin registry -->
  <BaseNodeViewer {nodeId} disableTitleUpdates={true} />
</div>

<style>
  .task-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
</style>
