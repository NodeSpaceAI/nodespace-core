<script lang="ts">
  import type { Node } from '$lib/types';
  import type { ConflictResolution } from '$lib/services/version-conflict-resolver';

  /**
   * Version Conflict Modal
   *
   * Displays conflict resolution UI when optimistic concurrency control
   * detects simultaneous modifications to the same node.
   *
   * Provides three resolution strategies:
   * 1. Use your changes (overwrite current version)
   * 2. Use current version (discard your changes)
   * 3. Manual merge (advanced users)
   */

  interface Props {
    /** Whether modal is visible */
    open?: boolean;

    /** Your attempted changes */
    yourChanges: Partial<Node>;

    /** Current node state from database */
    currentNode: Node;

    /** Auto-merge result (if available) */
    autoMergeResult?: ConflictResolution;

    /** Callback when user chooses a resolution */
    onResolve: (_resolvedNode: Node) => void;

    /** Callback when user cancels */
    onCancel: () => void;
  }

  let {
    open = false,
    yourChanges,
    currentNode,
    autoMergeResult,
    onResolve,
    onCancel
  }: Props = $props();

  function handleUseYours() {
    const resolved: Node = {
      ...currentNode,
      ...yourChanges,
      version: currentNode.version // Use current version for retry
    };
    onResolve(resolved);
  }

  function handleUseCurrent() {
    onResolve(currentNode);
  }

  function handleMerge() {
    // For now, just use auto-merge if available
    // TODO: Implement manual merge editor in future
    if (autoMergeResult?.mergedNode) {
      onResolve(autoMergeResult.mergedNode);
    }
  }

  // Determine which fields changed
  let changedFields = $derived(Object.keys(yourChanges));
</script>

{#if open}
  <!-- Modal overlay -->
  <div
    class="conflict-modal-overlay"
    onclick={onCancel}
    onkeydown={(e) => e.key === 'Escape' && onCancel()}
    role="presentation"
    tabindex="-1"
  >
    <!-- Modal content (stop propagation to prevent closing when clicking inside) -->
    <div
      class="conflict-modal-content"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      role="dialog"
      aria-labelledby="conflict-modal-title"
      aria-modal="true"
      tabindex="0"
    >
      <!-- Header -->
      <div class="conflict-modal-header">
        <h2 id="conflict-modal-title">Version Conflict Detected</h2>
        <p class="conflict-subtitle">
          This node was modified while you were editing. Choose how to resolve:
        </p>
      </div>

      <!-- Auto-merge notification (if available) -->
      {#if autoMergeResult?.autoMerged}
        <div class="conflict-auto-merge-notice">
          <span class="notice-icon">✓</span>
          <div class="notice-content">
            <strong>Auto-merge available:</strong>
            <p>{autoMergeResult.explanation}</p>
          </div>
        </div>
      {/if}

      <!-- Comparison view -->
      <div class="conflict-comparison">
        <!-- Your changes -->
        <div class="conflict-column">
          <h3>Your Changes</h3>
          <div class="conflict-details">
            <p class="version-info">
              Expected version: {currentNode.version - 1}
            </p>
            <div class="changes-list">
              {#each changedFields as field}
                <div class="change-item">
                  <strong>{field}:</strong>
                  {#if field === 'content'}
                    <pre class="content-preview">{yourChanges.content}</pre>
                  {:else if field === 'properties'}
                    <pre class="properties-preview">{JSON.stringify(
                        yourChanges.properties,
                        null,
                        2
                      )}</pre>
                  {:else}
                    <span>{yourChanges[field as keyof Node]}</span>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        </div>

        <!-- Current version -->
        <div class="conflict-column">
          <h3>Current Version</h3>
          <div class="conflict-details">
            <p class="version-info">
              Actual version: {currentNode.version}
            </p>
            <div class="changes-list">
              {#each changedFields as field}
                <div class="change-item">
                  <strong>{field}:</strong>
                  {#if field === 'content'}
                    <pre class="content-preview">{currentNode.content}</pre>
                  {:else if field === 'properties'}
                    <pre class="properties-preview">{JSON.stringify(
                        currentNode.properties,
                        null,
                        2
                      )}</pre>
                  {:else}
                    <span>{currentNode[field as keyof Node]}</span>
                  {/if}
                </div>
              {/each}
            </div>
            <p class="modified-at">
              Last modified: {new Date(currentNode.modifiedAt).toLocaleString()}
            </p>
          </div>
        </div>
      </div>

      <!-- Resolution actions -->
      <div class="conflict-actions">
        <button class="btn btn-primary" onclick={handleUseYours}>Use Your Changes</button>
        <button class="btn btn-secondary" onclick={handleUseCurrent}> Use Current Version </button>
        {#if autoMergeResult?.mergedNode}
          <button class="btn btn-success" onclick={handleMerge}>Auto-Merge</button>
        {/if}
        <button class="btn btn-tertiary" onclick={onCancel}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .conflict-modal-overlay {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .conflict-modal-content {
    background-color: white;
    border-radius: 8px;
    padding: 24px;
    max-width: 800px;
    max-height: 90vh;
    overflow-y: auto;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
  }

  .conflict-modal-header {
    margin-bottom: 20px;
  }

  .conflict-modal-header h2 {
    margin: 0 0 8px 0;
    font-size: 24px;
    color: #d32f2f;
  }

  .conflict-subtitle {
    margin: 0;
    color: #666;
    font-size: 14px;
  }

  .conflict-auto-merge-notice {
    display: flex;
    gap: 12px;
    padding: 12px;
    background-color: #e8f5e9;
    border-left: 4px solid #4caf50;
    border-radius: 4px;
    margin-bottom: 20px;
  }

  .notice-icon {
    font-size: 24px;
    color: #4caf50;
  }

  .notice-content {
    flex: 1;
  }

  .notice-content strong {
    color: #2e7d32;
  }

  .notice-content p {
    margin: 4px 0 0 0;
    font-size: 14px;
    color: #555;
  }

  .conflict-comparison {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    margin-bottom: 20px;
  }

  .conflict-column {
    border: 1px solid #ddd;
    border-radius: 4px;
    padding: 16px;
  }

  .conflict-column h3 {
    margin: 0 0 12px 0;
    font-size: 16px;
    color: #333;
  }

  .conflict-details {
    font-size: 14px;
  }

  .version-info {
    margin: 0 0 12px 0;
    color: #666;
    font-weight: 600;
  }

  .changes-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .change-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .change-item strong {
    color: #555;
    text-transform: capitalize;
  }

  .content-preview,
  .properties-preview {
    margin: 0;
    padding: 8px;
    background-color: #f5f5f5;
    border-radius: 4px;
    font-size: 12px;
    overflow-x: auto;
    max-height: 200px;
    overflow-y: auto;
  }

  .modified-at {
    margin: 12px 0 0 0;
    font-size: 12px;
    color: #999;
  }

  .conflict-actions {
    display: flex;
    gap: 12px;
    justify-content: flex-end;
  }

  .btn {
    padding: 10px 20px;
    border: none;
    border-radius: 4px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background-color 0.2s,
      transform 0.1s;
  }

  .btn:hover {
    transform: translateY(-1px);
  }

  .btn:active {
    transform: translateY(0);
  }

  .btn-primary {
    background-color: #1976d2;
    color: white;
  }

  .btn-primary:hover {
    background-color: #1565c0;
  }

  .btn-secondary {
    background-color: #757575;
    color: white;
  }

  .btn-secondary:hover {
    background-color: #616161;
  }

  .btn-success {
    background-color: #4caf50;
    color: white;
  }

  .btn-success:hover {
    background-color: #43a047;
  }

  .btn-tertiary {
    background-color: #e0e0e0;
    color: #333;
  }

  .btn-tertiary:hover {
    background-color: #d5d5d5;
  }
</style>
