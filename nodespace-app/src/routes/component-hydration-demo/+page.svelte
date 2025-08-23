<!--
  Component Hydration Demo - Issue #74 Validation
  
  Demonstrates the complete Phase 2 implementation:
  1. ContentProcessor renders markdown with nodespace references
  2. References are decorated using the new component system  
  3. Component placeholders are hydrated with Svelte components
  4. Plugin architecture is ready for future node types
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { contentProcessor } from '$lib/services/contentProcessor';
  import { componentHydrationSystem } from '$lib/services/ComponentHydrationSystem';
  import type { HydrationResult } from '$lib/services/ComponentHydrationSystem';

  // Content container for hydration

  let contentContainer: HTMLElement | undefined = undefined;
  let hydrationResult: HydrationResult | null = null;
  let processingStatus = 'Ready';

  // Sample content with various node reference types
  const sampleContent = `# Component Hydration Demo

This demonstrates the new component-based node reference system.

## Core Node Types

Here are some references to core node types:

- Task reference: nodespace://task/123/my-important-task
- User reference: nodespace://user/456/john-doe  
- Date reference: nodespace://date/789/2024-03-15
- Document reference: nodespace://document/101/project-spec.pdf

## Plugin Architecture Ready

The system is designed to support plugin node types (bundled at build time):

- PDF reference: nodespace://pdf/201/manual.pdf (would use PdfNodeReference from plugin)
- Image reference: nodespace://image/202/diagram.png (would use ImageNodeReference from plugin)
- Video reference: nodespace://video/203/demo.mp4 (would use VideoNodeReference from plugin)

## Mixed Content

You can also mix references with regular markdown:

This is a **bold** reference to nodespace://task/124/review-pr and some *italic* text 
with another reference nodespace://user/457/jane-smith in the middle.
`;

  async function processAndHydrate() {
    if (!contentContainer) return;

    try {
      processingStatus = 'Processing content...';

      // Step 1: Process markdown content with nodespace references
      const processed = await contentProcessor.processContent(sampleContent);

      processingStatus = 'Rendering HTML...';

      // Step 2: Render to HTML (this creates component placeholders)
      const html = contentProcessor.renderToHTML(processed);

      // Step 3: Insert HTML into container
      contentContainer.innerHTML = html;

      processingStatus = 'Hydrating components...';

      // Step 4: Hydrate component placeholders
      hydrationResult = await componentHydrationSystem.hydrate({
        container: contentContainer,
        onComponentMounted: (element, component) => {
          console.log('Component mounted:', { element, component });
        },
        onError: (error, placeholder) => {
          console.error('Component hydration failed:', { error, placeholder });
        }
      });

      processingStatus = `Completed: ${hydrationResult.hydrated} components hydrated, ${hydrationResult.failed} failed`;
    } catch (error) {
      console.error('Demo error:', error);
      processingStatus = `Error: ${error}`;
    }
  }

  function cleanup() {
    if (contentContainer) {
      componentHydrationSystem.cleanup(contentContainer);
    }
    hydrationResult = null;
    processingStatus = 'Cleaned up';
  }

  onMount(() => {
    // Auto-run the demo on mount
    setTimeout(processAndHydrate, 100);
  });
</script>

<div class="demo-container">
  <header class="demo-header">
    <h1>Component Hydration Demo</h1>
    <p class="subtitle">Issue #74 - Rich Node Reference Decorations</p>

    <div class="controls">
      <button on:click={processAndHydrate} class="btn btn-primary"> Process & Hydrate </button>

      <button on:click={cleanup} class="btn btn-secondary"> Cleanup Components </button>
    </div>

    <div class="status">
      <strong>Status:</strong>
      {processingStatus}
    </div>

    {#if hydrationResult}
      <div class="stats">
        <strong>Results:</strong>
        Hydrated: {hydrationResult.hydrated}, Failed: {hydrationResult.failed}, Errors: {hydrationResult
          .errors.length}
      </div>
    {/if}
  </header>

  <main class="demo-content">
    <div class="source-section">
      <h2>Source Markdown</h2>
      <pre class="source-code">{sampleContent}</pre>
    </div>

    <div class="output-section">
      <h2>Rendered Output with Components</h2>
      <div bind:this={contentContainer} class="content-container">
        <!-- Content will be inserted and hydrated here -->
      </div>
    </div>
  </main>
</div>

<style>
  .demo-container {
    max-width: 1200px;
    margin: 0 auto;
    padding: 2rem;
    font-family:
      system-ui,
      -apple-system,
      sans-serif;
  }

  .demo-header {
    margin-bottom: 2rem;
    padding-bottom: 1rem;
    border-bottom: 2px solid #e5e7eb;
  }

  .demo-header h1 {
    color: #1f2937;
    margin-bottom: 0.5rem;
  }

  .subtitle {
    color: #6b7280;
    margin-bottom: 1rem;
  }

  .controls {
    display: flex;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .btn {
    padding: 0.5rem 1rem;
    border: none;
    border-radius: 0.25rem;
    cursor: pointer;
    font-size: 0.875rem;
    font-weight: 500;
  }

  .btn-primary {
    background: #3b82f6;
    color: white;
  }

  .btn-primary:hover {
    background: #2563eb;
  }

  .btn-secondary {
    background: #6b7280;
    color: white;
  }

  .btn-secondary:hover {
    background: #4b5563;
  }

  .status,
  .stats {
    font-size: 0.875rem;
    color: #4b5563;
    margin-bottom: 0.5rem;
  }

  .demo-content {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2rem;
  }

  .source-section,
  .output-section {
    background: #f9fafb;
    border-radius: 0.5rem;
    padding: 1.5rem;
  }

  .source-section h2,
  .output-section h2 {
    color: #374151;
    margin-bottom: 1rem;
    font-size: 1.25rem;
  }

  .source-code {
    background: #1f2937;
    color: #d1d5db;
    padding: 1rem;
    border-radius: 0.25rem;
    overflow-x: auto;
    font-size: 0.875rem;
    line-height: 1.5;
  }

  .content-container {
    min-height: 200px;
    background: white;
    border: 1px solid #d1d5db;
    border-radius: 0.25rem;
    padding: 1rem;
  }

  /* Style the component placeholders while they're loading */
  :global(.ns-component-placeholder[data-hydrate='pending']) {
    background: #fef3c7;
    border: 1px dashed #f59e0b;
    padding: 0.125rem 0.25rem;
    border-radius: 0.125rem;
  }

  :global(.ns-component-placeholder[data-hydrate='failed']) {
    background: #fee2e2;
    border: 1px dashed #ef4444;
    padding: 0.125rem 0.25rem;
    border-radius: 0.125rem;
  }

  /* Responsive design */
  @media (max-width: 768px) {
    .demo-content {
      grid-template-columns: 1fr;
    }

    .controls {
      flex-direction: column;
    }
  }
</style>
