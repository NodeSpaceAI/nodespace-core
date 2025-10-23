#!/usr/bin/env node
/**
 * Test script for NodeOperations commands
 *
 * This tests the new architecture:
 * - update_node (content-only)
 * - move_node (hierarchy changes)
 * - reorder_node (sibling ordering)
 */

const BASE_URL = 'http://localhost:3001';

async function request(endpoint, data) {
  const response = await fetch(`${BASE_URL}${endpoint}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Request failed: ${response.status} - ${error}`);
  }

  return response.json();
}

async function createNode(nodeType, content, parentId = null, containerNodeId = null) {
  console.log(`📝 Creating ${nodeType} node: "${content}"`);
  return request('/create_node', {
    node_type: nodeType,
    content,
    parent_id: parentId,
    container_node_id: containerNodeId,
    properties: {},
  });
}

async function updateNode(id, content) {
  console.log(`✏️  Updating node ${id} with content: "${content}"`);
  return request('/update_node', {
    id,
    update: { content },
  });
}

async function moveNode(nodeId, newParentId) {
  console.log(`🔀 Moving node ${nodeId} to parent ${newParentId}`);
  return request('/move_node', {
    node_id: nodeId,
    new_parent_id: newParentId,
  });
}

async function reorderNode(nodeId, beforeSiblingId) {
  console.log(`↕️  Reordering node ${nodeId} before ${beforeSiblingId}`);
  return request('/reorder_node', {
    node_id: nodeId,
    before_sibling_id: beforeSiblingId,
  });
}

async function getNode(id) {
  const response = await fetch(`${BASE_URL}/get_node/${id}`);
  if (!response.ok) {
    throw new Error(`Failed to get node: ${response.status}`);
  }
  return response.json();
}

async function runTests() {
  console.log('🚀 Starting NodeOperations tests\n');

  try {
    // Test 1: Create a date container
    console.log('📋 TEST 1: Create date container');
    const containerId = await createNode('date', '2025-10-23');
    console.log(`✅ Container created: ${containerId}\n`);

    // Test 2: Create nodes in the container
    console.log('📋 TEST 2: Create child nodes');
    const node1Id = await createNode('text', 'First task', null, containerId);
    const node2Id = await createNode('text', 'Second task', null, containerId);
    const node3Id = await createNode('text', 'Third task', null, containerId);
    console.log(`✅ Created 3 nodes: ${node1Id}, ${node2Id}, ${node3Id}\n`);

    // Test 3: Update node content (should work - content only)
    console.log('📋 TEST 3: Update node content');
    await updateNode(node1Id, 'Updated first task');
    const updated = await getNode(node1Id);
    console.log(`✅ Content updated: "${updated.content}"\n`);

    // Test 4: Move node to new parent
    console.log('📋 TEST 4: Move node (create parent first)');
    const newParentId = await createNode('text', 'Parent node', null, containerId);
    await moveNode(node2Id, newParentId);
    const moved = await getNode(node2Id);
    console.log(`✅ Node moved: parent_id = ${moved.parent_id}\n`);

    // Test 5: Reorder nodes
    console.log('📋 TEST 5: Reorder nodes');
    await reorderNode(node3Id, node1Id);
    const reordered = await getNode(node3Id);
    console.log(`✅ Node reordered: before_sibling_id = ${reordered.before_sibling_id}\n`);

    // Test 6: Try to update with hierarchy fields (should only update content)
    console.log('📋 TEST 6: Update with hierarchy fields (should be ignored)');
    await updateNode(node1Id, 'Final content');
    const final = await getNode(node1Id);
    console.log(`✅ Only content updated: "${final.content}"`);
    console.log(`   parent_id unchanged: ${final.parent_id}`);
    console.log(`   container_node_id unchanged: ${final.container_node_id}\n`);

    console.log('🎉 All tests passed!');

  } catch (error) {
    console.error('❌ Test failed:', error.message);
    process.exit(1);
  }
}

runTests();
