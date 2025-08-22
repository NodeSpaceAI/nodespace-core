<!--
  NodeRef Demo Page - Showcase BaseNode Decoration System
  
  Dedicated route for demonstrating the comprehensive Phase 2.2 implementation
  of the BaseNode decorateReference() pattern and CSS framework.
-->

<script lang="ts">
  import NodeReferenceDemo from '$lib/components/NodeReferenceDemo.svelte';
</script>

<svelte:head>
  <title>NodeSpace - BaseNode Decoration System Demo</title>
  <meta
    name="description"
    content="Interactive demonstration of the NodeSpace BaseNode decoration system with rich visual node references"
  />
</svelte:head>

<main class="min-h-screen bg-background">
  <NodeReferenceDemo />
</main>
