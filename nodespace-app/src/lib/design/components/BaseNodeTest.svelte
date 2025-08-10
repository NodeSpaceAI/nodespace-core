<!--
  Simple BaseNode Test Component
  Tests the simplified BaseNode with various configurations
-->

<script>
  import BaseNode from './BaseNode.svelte';
</script>

<div class="test-container">
  <h2>BaseNode Simplification Tests</h2>

  <!-- Test 1: Basic node with circle indicator -->
  <div class="test-case">
    <h3>1. Basic Text Node (Circle Indicator)</h3>
    <BaseNode
      nodeType="text"
      nodeId="test-1"
      content="Basic text node with circle indicator"
    />
  </div>

  <!-- Test 2: Node with SVG icon -->
  <div class="test-case">
    <h3>2. Text Node with SVG Icon</h3>
    <BaseNode
      nodeType="text"
      nodeId="test-2"
      content="Text node with SVG icon"
      iconName="text"
    />
  </div>

  <!-- Test 3: Parent node with children -->
  <div class="test-case">
    <h3>3. Parent Node (Ring Effect)</h3>
    <BaseNode
      nodeType="text"
      nodeId="test-3"
      content="Parent node with ring effect"
      hasChildren={true}
    />
  </div>

  <!-- Test 4: Processing states -->
  <div class="test-case">
    <h3>4. Processing States</h3>
    <div class="processing-grid">
      <BaseNode
        nodeType="text"
        nodeId="test-4a"
        content="Processing: Pulse"
        iconName="text"
        isProcessing={true}
        processingAnimation="pulse"
      />
      <BaseNode
        nodeType="text"
        nodeId="test-4b"
        content="Processing: Spin"
        iconName="text"
        isProcessing={true}
        processingAnimation="spin"
      />
      <BaseNode
        nodeType="text"
        nodeId="test-4c"
        content="Processing: Blink"
        iconName="text"
        isProcessing={true}
        processingAnimation="blink"
      />
      <BaseNode
        nodeType="text"
        nodeId="test-4d"
        content="Processing: Fade"
        iconName="text"
        isProcessing={true}
        processingAnimation="fade"
      />
    </div>
  </div>

  <!-- Test 5: Different node types -->
  <div class="test-case">
    <h3>5. Different Node Types</h3>
    <div class="node-types-grid">
      <BaseNode
        nodeType="text"
        nodeId="test-5a"
        content="Text Node"
        iconName="text"
      />
      <BaseNode
        nodeType="task"
        nodeId="test-5b"
        content="Task Node"
      />
      <BaseNode
        nodeType="ai-chat"
        nodeId="test-5c"
        content="AI Chat Node"
      />
      <BaseNode
        nodeType="entity"
        nodeId="test-5d"
        content="Entity Node"
      />
      <BaseNode
        nodeType="query"
        nodeId="test-5e"
        content="Query Node"
      />
    </div>
  </div>
</div>

<style>
  .test-container {
    padding: 20px;
    max-width: 800px;
    margin: 0 auto;
    font-family: system-ui, sans-serif;
  }

  .test-case {
    margin-bottom: 40px;
    border: 1px solid #e0e0e0;
    border-radius: 8px;
    padding: 20px;
  }

  .test-case h3 {
    margin-top: 0;
    color: #333;
    font-size: 18px;
  }

  .processing-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }

  .node-types-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 16px;
  }

  @media (max-width: 640px) {
    .processing-grid {
      grid-template-columns: 1fr;
    }
  }
</style>