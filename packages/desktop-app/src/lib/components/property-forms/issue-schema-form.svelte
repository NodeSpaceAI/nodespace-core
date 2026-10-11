<!--
  IssueSchemaForm - The properties form of an issue

  An issue is a task that is development work, so its form is the task form with the
  issue's own status values and the two fields that say where the work landed
  (pullRequest, commits). TaskSchemaForm renders both types; this component is what the
  issue plugin registers, and it selects the issue variant.

  Props:
  - nodeId: ID of the issue to display properties for
-->

<script lang="ts">
  import TaskSchemaForm from './task-schema-form.svelte';

  let { nodeId }: { nodeId: string } = $props();
</script>

<TaskSchemaForm {nodeId} nodeType="issue" />
