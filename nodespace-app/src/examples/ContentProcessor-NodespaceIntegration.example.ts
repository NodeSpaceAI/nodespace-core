/**
 * ContentProcessor + NodeReferenceService Integration Example
 *
 * Demonstrates the enhanced ContentProcessor working with nodespace:// URIs
 * and real-time reference resolution (Phase 2.1 Days 4-5)
 */

import { contentProcessor } from '../lib/services/contentProcessor';
// import NodeReferenceService from '../lib/services/NodeReferenceService';
import { eventBus } from '../lib/services/EventBus';

// Example: Setting up ContentProcessor with NodeReferenceService integration
export async function demonstrateNodespaceIntegration() {
  console.log('🔗 ContentProcessor + NodeReferenceService Integration Demo');

  // Example content with various reference types
  const sampleContent = `
    # My Document
    
    This document contains several types of references:
    
    ## Traditional Wiki Links
    - [[Traditional Wiki Link]]
    - [[Another Wiki Link|Custom Display Text]]
    
    ## Modern Nodespace References
    - [Project Overview](nodespace://node/project-overview-123)
    - [Meeting Notes](nodespace://node/meeting-notes-456?hierarchy=true)
    - [Research Data](nodespace://node/research-data-789)
    
    ## Mixed Content
    Regular **bold text** and *italic text* with both [[wiki links]] 
    and [modern refs](nodespace://node/modern-ref-abc).
  `;

  // 1. Basic Content Processing
  console.log('\n📝 1. Basic Content Processing:');
  const ast = contentProcessor.parseMarkdown(sampleContent);
  console.log('- Total characters:', ast.metadata.totalCharacters);
  console.log('- Has wiki links:', ast.metadata.hasWikiLinks);
  console.log('- Has nodespace refs:', ast.metadata.hasNodespaceRefs);
  console.log('- Nodespace ref count:', ast.metadata.nodeRefCount);

  // 2. Detect Nodespace URIs
  console.log('\n🔍 2. Nodespace URI Detection:');
  const nodespaceLinks = contentProcessor.detectNodespaceURIs(sampleContent);
  console.log('Found', nodespaceLinks.length, 'nodespace references:');
  nodespaceLinks.forEach((link, index) => {
    console.log(`  ${index + 1}. "${link.displayText}" → ${link.uri}`);
    console.log(`     Valid: ${link.isValid}, Node ID: ${link.nodeId}`);
  });

  // 3. Render to HTML
  console.log('\n🎨 3. HTML Rendering:');
  const html = contentProcessor.markdownToDisplay(sampleContent);
  console.log('Generated HTML length:', html.length);

  // Show a snippet of the nodespace reference rendering
  const noderefMatch = html.match(/<a class="ns-noderef[^>]*>.*?<\/a>/);
  if (noderefMatch) {
    console.log('Sample nodespace reference HTML:');
    console.log('  ', noderefMatch[0]);
  }

  // 4. Event Processing
  console.log('\n📡 4. Event-Driven Processing:');
  const sourceNodeId = 'demo-document-001';

  // Set up event listeners to capture emitted events
  const capturedEvents: unknown[] = [];
  const originalEmit = eventBus.emit;
  eventBus.emit = (event: unknown) => {
    capturedEvents.push(event);
    return originalEmit.call(eventBus, event);
  };

  // Process content with event emission
  contentProcessor.processContentWithEventEmission(sampleContent, sourceNodeId);

  console.log('Captured', capturedEvents.length, 'events:');
  capturedEvents.forEach((event, index) => {
    console.log(`  ${index + 1}. ${event.type} (${event.namespace})`);
    if (event.type === 'backlink:detected') {
      console.log(`     Source: ${event.sourceNodeId} → Target: ${event.targetNodeId}`);
      console.log(`     Link Type: ${event.linkType}, Text: "${event.linkText}"`);
    }
  });

  // Restore original emit
  eventBus.emit = originalEmit;

  // 5. Cache Management
  console.log('\n💾 5. Cache Management:');
  const cacheStats = contentProcessor.getReferencesCacheStats();
  console.log('Cache stats:', cacheStats);

  // Clear cache
  contentProcessor.clearReferenceCache();
  console.log('Cache cleared');

  // 6. Performance Test
  console.log('\n⚡ 6. Performance Test:');
  const largeContent = Array.from(
    { length: 50 },
    (_, i) => `Reference ${i}: [Node ${i}](nodespace://node/node-${i})`
  ).join(' ');

  const startTime = performance.now();
  const largeLinks = contentProcessor.detectNodespaceURIs(largeContent);
  const endTime = performance.now();

  console.log(`Processed ${largeLinks.length} references in ${(endTime - startTime).toFixed(2)}ms`);

  // 7. AST Round-trip Test
  console.log('\n🔄 7. AST Round-trip Test:');
  const originalText = 'Check [My Node](nodespace://node/test-123) for details.';
  const parsedAst = contentProcessor.parseMarkdown(originalText);
  const reconstructed = contentProcessor.astToMarkdown(parsedAst);

  console.log('Original:', originalText);
  console.log('Reconstructed:', reconstructed);
  console.log('Round-trip successful:', originalText.trim() === reconstructed.trim());

  console.log('\n✅ Integration demonstration complete!');
}

// Example: Real-time content processing with reference updates
export function demonstrateRealTimeProcessing() {
  console.log('\n🔄 Real-time Processing Demo');

  // Simulate real-time content updates
  const contentVersions = [
    'Initial content',
    'Content with [[wiki link]]',
    'Content with [node ref](nodespace://node/ref-123)',
    'Mixed content: [[wiki]] and [node](nodespace://node/node-456)'
  ];

  contentVersions.forEach((content, version) => {
    console.log(`\nVersion ${version + 1}: "${content}"`);

    const ast = contentProcessor.parseMarkdown(content);
    const nodespaceRefs = contentProcessor.detectNodespaceURIs(content);

    console.log(`  - AST nodes: ${ast.children.length}`);
    console.log(`  - Nodespace refs: ${nodespaceRefs.length}`);

    if (nodespaceRefs.length > 0) {
      console.log(`  - Reference URIs: ${nodespaceRefs.map((r) => r.uri).join(', ')}`);
    }
  });
}

// Export for use in applications
export { contentProcessor };

// Example usage demonstration
if (import.meta.main) {
  demonstrateNodespaceIntegration();
  demonstrateRealTimeProcessing();
}
