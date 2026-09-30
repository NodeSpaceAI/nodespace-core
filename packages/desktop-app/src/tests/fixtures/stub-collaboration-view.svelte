<!-- Stand-in for the live collaboration view: takes its `collectionId` prop and renders a marker. -->
<script lang="ts">
  let { collectionId }: { collectionId: string } = $props();
</script>

<div data-testid="stub-collaboration-view" data-collection-id={collectionId}></div>
