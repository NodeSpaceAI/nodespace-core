<!--
  IssueNodeViewer - Page-level viewer for issue nodes

  An issue is a task that is development work. Like TaskNodeViewer, this wraps
  BaseNodeViewer, which loads IssueSchemaForm from the plugin registry: the task form
  with the issue's own statuses, its pull request and its commits.
-->

<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';

  let {
    nodeId
  }: {
    nodeId: string;
    onNodeIdChange?: (_nodeId: string) => void;
  } = $props();
</script>

<div class="issue-node-viewer">
  <BaseNodeViewer {nodeId} />
</div>

<style>
  .issue-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
</style>
