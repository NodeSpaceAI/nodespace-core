<!--
  TextNode Demo Component
  
  Demonstrates TextNode functionality for testing and development.
  Shows various configurations and use cases.
-->

<script lang="ts">
  import TextNode from './TextNode.svelte';

  let demoNodes = [
    {
      id: 'demo-1',
      content: 'Simple text without markdown. Click to edit!',
      editable: true,
      markdown: false
    },
    {
      id: 'demo-2',
      content:
        '# Welcome to NodeSpace\n\nThis TextNode supports **markdown** formatting:\n\n- *Italic text*\n- **Bold text** \n- `inline code`\n- [Links](https://example.com)\n\n## Features\n\n1. Multi-line editing\n2. Auto-save\n3. Markdown rendering',
      editable: true,
      markdown: true
    },
    {
      id: 'demo-3',
      content:
        'This node is **read-only** and cannot be edited. It still renders markdown but demonstrates display-only mode.',
      editable: false,
      markdown: true
    },
    {
      id: 'new-node',
      content: '',
      editable: true,
      markdown: true
    }
  ];

  function handleSave(event: CustomEvent) {
    console.log('Node saved:', event.detail);
  }

  function handleError(event: CustomEvent) {
    console.error('Node error:', event.detail);
  }

  function handleContentChanged(event: CustomEvent) {
    console.log('Node content changed:', event.detail);
    // This event fires during editing before save, allowing parent components
    // to track which nodes have unsaved changes
  }

  function handleFocus(event: CustomEvent) {
    console.log('Node focused:', event.detail);
    // When a node gains focus, you could save other nodes with pending changes
  }

  function handleBlur(event: CustomEvent) {
    console.log('Node blurred:', event.detail);
    // When a node loses focus, it automatically saves and renders markdown
  }
</script>

<div class="text-node-demo">
  <h1 class="demo-title">TextNode Component Demo</h1>

  <div class="demo-description">
    <p>This demo showcases the TextNode component with various configurations:</p>
    <ul>
      <li><strong>Click-to-edit:</strong> Click on any editable node to start editing</li>
      <li><strong>Keyboard shortcuts:</strong> Ctrl+Enter to save, Esc to cancel</li>
      <li><strong>Auto-save:</strong> Changes are automatically saved after 2 seconds</li>
      <li><strong>Markdown:</strong> Support for basic markdown formatting</li>
      <li><strong>Auto-resize:</strong> Textarea expands to fit content</li>
    </ul>
  </div>

  <div class="demo-nodes">
    {#each demoNodes as node (node.id)}
      <div class="demo-node-container">
        <h3 class="demo-node-title">
          {node.id === 'new-node' ? 'New Node' : `Demo Node ${node.id.split('-')[1]}`}
          <span class="demo-node-config">
            {node.editable ? '(Editable)' : '(Read-only)'}
            {node.markdown ? ' + Markdown' : ''}
          </span>
        </h3>

        <TextNode
          nodeId={node.id}
          content={node.content}
          editable={node.editable}
          markdown={node.markdown}
          placeholder={node.id === 'new-node'
            ? 'Start typing to create a new text node...'
            : 'Click to add text...'}
          on:save={handleSave}
          on:error={handleError}
          on:contentChanged={handleContentChanged}
          on:focus={handleFocus}
          on:blur={handleBlur}
        />
      </div>
    {/each}
  </div>

  <div class="demo-instructions">
    <h2>Try These Features:</h2>
    <div class="instruction-grid">
      <div class="instruction">
        <h4>Basic Editing</h4>
        <p>
          Click on the first node to start editing. Type some text and press Ctrl+Enter to save, or
          Esc to cancel.
        </p>
      </div>

      <div class="instruction">
        <h4>Markdown Formatting</h4>
        <p>
          Try the second node with markdown. Add **bold text**, *italic text*, # headers, and see
          them rendered.
        </p>
      </div>

      <div class="instruction">
        <h4>Multi-line Editing</h4>
        <p>
          TextNodes support multi-line editing. Use Shift+Enter for line breaks, or just press Enter
          normally in the textarea.
        </p>
      </div>

      <div class="instruction">
        <h4>Markdown Rendering</h4>
        <p>
          Second node shows markdown rendering in display mode. Edit to see raw markdown, blur to
          see rendered output.
        </p>
      </div>

      <div class="instruction">
        <h4>Read-only Mode</h4>
        <p>
          Third node demonstrates read-only display with markdown rendering but no editing
          capability.
        </p>
      </div>

      <div class="instruction">
        <h4>Auto-resize</h4>
        <p>Add multiple lines of text to see the textarea automatically expand.</p>
      </div>

      <div class="instruction">
        <h4>New Node</h4>
        <p>The last node starts empty. Click to add content and it will be automatically saved.</p>
      </div>

      <div class="instruction">
        <h4>Read-only</h4>
        <p>The third node cannot be edited, demonstrating display-only mode.</p>
      </div>
    </div>
  </div>
</div>

<style>
  .text-node-demo {
    max-width: 1200px;
    margin: 0 auto;
    padding: var(--ns-spacing-6, 24px);
    font-family:
      system-ui,
      -apple-system,
      sans-serif;
  }

  .demo-title {
    font-size: 2rem;
    margin-bottom: var(--ns-spacing-4, 16px);
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .demo-description {
    background: var(--ns-color-surface-panel, #f8f9fa);
    padding: var(--ns-spacing-4, 16px);
    border-radius: var(--ns-radius-lg, 8px);
    margin-bottom: var(--ns-spacing-6, 24px);
  }

  .demo-description p {
    margin-bottom: var(--ns-spacing-2, 8px);
  }

  .demo-description ul {
    margin: 0;
    padding-left: var(--ns-spacing-4, 16px);
  }

  .demo-description li {
    margin-bottom: var(--ns-spacing-1, 4px);
  }

  .demo-nodes {
    display: grid;
    gap: var(--ns-spacing-6, 24px);
    margin-bottom: var(--ns-spacing-8, 32px);
  }

  .demo-node-container {
    border: 1px solid var(--ns-color-border-default, #e5e7eb);
    border-radius: var(--ns-radius-lg, 8px);
    padding: var(--ns-spacing-4, 16px);
    background: var(--ns-color-surface-default, white);
  }

  .demo-node-title {
    margin: 0 0 var(--ns-spacing-3, 12px) 0;
    font-size: 1.1rem;
    display: flex;
    align-items: center;
    gap: var(--ns-spacing-2, 8px);
  }

  .demo-node-config {
    font-size: 0.8rem;
    color: var(--ns-color-text-tertiary, #6b7280);
    font-weight: normal;
  }

  .demo-instructions {
    background: var(--ns-color-surface-panel, #f8f9fa);
    padding: var(--ns-spacing-6, 24px);
    border-radius: var(--ns-radius-lg, 8px);
  }

  .demo-instructions h2 {
    margin: 0 0 var(--ns-spacing-4, 16px) 0;
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .instruction-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: var(--ns-spacing-4, 16px);
  }

  .instruction {
    background: var(--ns-color-surface-default, white);
    padding: var(--ns-spacing-3, 12px);
    border-radius: var(--ns-radius-md, 6px);
    border: 1px solid var(--ns-color-border-subtle, #f3f4f6);
  }

  .instruction h4 {
    margin: 0 0 var(--ns-spacing-2, 8px) 0;
    color: var(--ns-color-text-primary, #1a1a1a);
  }

  .instruction p {
    margin: 0;
    font-size: 0.9rem;
    color: var(--ns-color-text-secondary, #4b5563);
    line-height: 1.4;
  }

  /* Responsive adjustments */
  @media (max-width: 768px) {
    .text-node-demo {
      padding: var(--ns-spacing-4, 16px);
    }

    .demo-title {
      font-size: 1.5rem;
    }

    .instruction-grid {
      grid-template-columns: 1fr;
    }

    .demo-node-title {
      flex-direction: column;
      align-items: flex-start;
      gap: var(--ns-spacing-1, 4px);
    }
  }
</style>
