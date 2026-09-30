<!-- Fixture database-row component that throws while rendering. -->
<script lang="ts">
  let { databaseId }: { databaseId: string } = $props();

  throw new Error('test-throwing-row: render failed');
</script>

<div data-testid="test-throwing-row" data-database-id={databaseId}></div>
