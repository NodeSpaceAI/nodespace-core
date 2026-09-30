<!-- Fixture component that throws while rendering. -->
<script lang="ts">
  throw new Error('test-throwing: render failed');
</script>

<div data-testid="test-throwing"></div>
