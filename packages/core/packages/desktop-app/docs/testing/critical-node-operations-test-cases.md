# Critical Node Operations - Test Cases Checklist

**Purpose:** Comprehensive test coverage for all node operations to prevent regressions during refactoring (especially SharedNodeStore integration).

**Approach:** Test-Driven Development (TDD)
1. Write test for each case below
2. Verify test passes on current working version (commit b4b8270)
3. Use tests to validate future refactoring work

---

## 1. Enter Key (Node Creation/Splitting)

### Basic Operations
- [ ] **Basic Enter** - Create new node after current node
  - Given: Node with content "Hello"
  - When: Press Enter at end
  - Then: New empty node created after, cursor in new node

- [ ] **Enter at beginning** - Create node before current (insertAtBeginning)
  - Given: Node with content "Hello"
  - When: Press Enter at position 0
  - Then: New node created before, original content stays, cursor in new node

- [ ] **Enter in middle** - Split content at cursor position
  - Given: Node with content "Hello World"
  - When: Press Enter at position 6 (after "Hello ")
  - Then: First node has "Hello ", second node has "World"

- [ ] **Enter at end** - Create empty node after
  - Given: Node with content "Hello"
  - When: Press Enter at end
  - Then: New empty placeholder node created, cursor positioned

### Header Preservation
- [ ] **Enter with header syntax** - Preserve/inherit header level
  - Given: Node with content "## Section Header"
  - When: Press Enter at end
  - Then: New node has "## " prefix, inherits header level

- [ ] **Enter at beginning of header** - Preserve header on original
  - Given: Node with "## Header"
  - When: Press Enter at position 0
  - Then: Original keeps "## Header", new node has "## " prefix

### Edge Cases
- [ ] **Enter in empty node** - Create sibling, don't duplicate
  - Given: Empty placeholder node
  - When: Press Enter
  - Then: New sibling created, original stays empty

- [ ] **Enter with children (expanded node)** - Transfer children
  - Given: Node "Parent" with children ["Child1", "Child2"], expanded
  - When: Press Enter at end of "Parent"
  - Then: New node created, children transferred to new node

- [ ] **Last character leak prevention** - No trailing characters
  - Given: Multiline node with content
  - When: Press Enter at end
  - Then: No characters left behind in original node

### Sibling Chain Updates
- [ ] **Enter updates next sibling beforeSiblingId**
  - Given: Siblings [A, B, C] where B.beforeSiblingId = A
  - When: Press Enter in A (creates A2)
  - Then: B.beforeSiblingId = A2

---

## 2. Shift+Enter (Newline/Line Break)

### Basic Newline
- [ ] **Basic Shift+Enter** - Insert `\n` without creating new node
  - Given: Node with content "Hello"
  - When: Press Shift+Enter at end
  - Then: Content becomes "Hello\n", stays in same node

- [ ] **Multiple Shift+Enter** - Handle multiple newlines
  - Given: Node with content "Hello"
  - When: Press Shift+Enter twice
  - Then: Content becomes "Hello\n\n"

- [ ] **Shift+Enter rendering** - Visual line break appears
  - Given: Node with content "Hello"
  - When: Press Shift+Enter at end
  - Then: UI shows line break (DIV structure or <br>)

### Inline Formatting Preservation
- [ ] **Shift+Enter in bold** - Preserve **bold** formatting
  - Given: Content "**bold text**", cursor at position 7 (after "**bol")
  - When: Press Shift+Enter
  - Then: Content becomes "**bol**\n**d text**"

- [ ] **Shift+Enter in italic** - Preserve *italic* formatting
  - Given: Content "*italic*", cursor at position 5 (after "*ita")
  - When: Press Shift+Enter
  - Then: Content becomes "*ita*\n*lic*"

- [ ] **Shift+Enter in code** - Preserve `code` formatting
  - Given: Content "`code`", cursor at position 4 (after "`co")
  - When: Press Shift+Enter
  - Then: Content becomes "`co`\n`de`"

- [ ] **Shift+Enter in strikethrough (double)** - Preserve ~~strikethrough~~
  - Given: Content "~~strike~~", cursor at position 9 (after "~~str")
  - When: Press Shift+Enter
  - Then: Content becomes "~~str~~\n~~ike~~"

- [ ] **Shift+Enter in strikethrough (single)** - Preserve ~strikethrough~
  - Given: Content "~strike~", cursor at position 7 (after "~str")
  - When: Press Shift+Enter
  - Then: Content becomes "~str~\n~ike~"

- [ ] **Shift+Enter with mixed formatting** - Multiple formats on same line
  - Given: Content "**bold** and *italic*", cursor at position 13
  - When: Press Shift+Enter
  - Then: Formatting preserved on both lines

### Cursor Positioning
- [ ] **Shift+Enter cursor positioning** - Cursor after opening markers
  - Given: Content "**bold**", cursor at position 6
  - When: Press Shift+Enter
  - Then: Cursor positioned after "**" on new line (position 2 in new line)

### Arrow Navigation After Shift+Enter
- [ ] **Arrow up/down in multiline node**
  - Given: Node with "Line 1\nLine 2" (created via Shift+Enter)
  - When: Press ArrowDown from Line 1
  - Then: Cursor moves to Line 2 (stays in same node)

---

## 3. Backspace (Node Merging/Deletion)

### Basic Merging
- [ ] **Basic backspace merge** - Combine current with previous
  - Given: Node A "Hello", Node B "World"
  - When: Backspace at position 0 in Node B
  - Then: Node A becomes "HelloWorld", Node B deleted

- [ ] **Backspace merge cursor position** - Cursor at junction
  - Given: Node A "Hello" (5 chars), Node B "World"
  - When: Backspace in Node B at position 0
  - Then: Merged content "HelloWorld", cursor at position 5

- [ ] **Backspace with inline formatting** - Preserve formatting
  - Given: Node A "**bold**", Node B "text"
  - When: Backspace in Node B
  - Then: Merged content preserves bold formatting

### Child Handling
- [ ] **Backspace with children** - Promote children to parent
  - Given: Node A, Node B (to delete) with children [C1, C2]
  - When: Backspace merge deletes Node B
  - Then: C1 and C2 become children of Node A's parent

- [ ] **Children promotion preserves depth** - Depth maintained
  - Given: Node B (depth 2) with children (depth 3)
  - When: Delete Node B
  - Then: Children promoted to depth 2

- [ ] **Children disappearing prevention** - Children not lost
  - Given: Node with multiple children
  - When: Delete node via backspace merge
  - Then: All children still exist with correct parent

### Sibling Chain Updates
- [ ] **Backspace sibling chain update** - beforeSiblingId repaired
  - Given: Siblings [A, B, C] where B.beforeSiblingId=A, C.beforeSiblingId=B
  - When: Delete B
  - Then: C.beforeSiblingId=A

### Non-Merge Backspace
- [ ] **Backspace in middle of content** - Delete character only
  - Given: Node "Hello" with cursor at position 3
  - When: Press Backspace
  - Then: Content becomes "Helo" (no merge triggered)

---

## 4. Indent/Outdent Operations

### Basic Indent
- [ ] **Basic indent** - Make node child of previous sibling
  - Given: Siblings [A, B, C]
  - When: Indent B
  - Then: B becomes child of A, C remains sibling of A

- [ ] **Indent first child** - Cannot indent (no previous sibling)
  - Given: First child node
  - When: Attempt indent
  - Then: Operation fails/does nothing

- [ ] **Indent with existing children** - Append to end
  - Given: Node A with children [C1, C2], Node B is sibling
  - When: Indent B (make child of A)
  - Then: B.beforeSiblingId = C2 (inserted after last child)

- [ ] **Indent updates sibling chain** - Remove from old chain
  - Given: Siblings [A, B, C] where B.beforeSiblingId=A, C.beforeSiblingId=B
  - When: Indent B
  - Then: C.beforeSiblingId=A (B removed from chain)

- [ ] **Indent depth calculation** - Depth increases by 1
  - Given: Node A (depth 1), Node B (depth 1)
  - When: Indent B to make child of A
  - Then: B.depth = 2

### Basic Outdent
- [ ] **Basic outdent** - Move node up one level
  - Given: Node A with child B
  - When: Outdent B
  - Then: B becomes sibling of A

- [ ] **Outdent root node** - Cannot outdent (no parent)
  - Given: Root level node
  - When: Attempt outdent
  - Then: Operation fails/does nothing

- [ ] **Outdent positioning** - Insert after old parent
  - Given: Node A with child B, sibling C
  - When: Outdent B
  - Then: B positioned after A in sibling list

### Outdent with Siblings Transfer
- [ ] **Outdent transfers siblings below as children**
  - Given: Parent P with children [A, B, C]
  - When: Outdent A
  - Then: B and C become children of A

- [ ] **Transferred siblings maintain order**
  - Given: Parent with children [A, B, C, D]
  - When: Outdent A
  - Then: A's children are [B, C, D] in that order

- [ ] **Outdent sibling chain integrity** - Valid chain after transfer
  - Given: Complex sibling chain
  - When: Outdent node with siblings below
  - Then: All beforeSiblingId references valid

### Depth Recalculation
- [ ] **Outdent updates descendant depths** - Recursive depth update
  - Given: Node A (depth 2) with children (depth 3) and grandchildren (depth 4)
  - When: Outdent A to depth 1
  - Then: Children become depth 2, grandchildren depth 3

- [ ] **Indent updates descendant depths**
  - Given: Node with children
  - When: Indent node
  - Then: All descendants increase depth by 1

### Deep Hierarchy
- [ ] **Indent/outdent in deep hierarchy** - Works at any depth
  - Given: Node at depth 5
  - When: Indent or outdent
  - Then: Operation succeeds, depths recalculated correctly

### Parent Validation
- [ ] **Outdent parent validation** - Parent must exist
  - Given: Node with non-existent parent reference
  - When: Attempt outdent
  - Then: Operation handled gracefully

---

## 5. Sibling Chain Integrity

### Creation
- [ ] **Creating node updates next sibling** - Chain maintained
  - Given: Siblings [A, B] where B.beforeSiblingId=A
  - When: Create node N after A
  - Then: B.beforeSiblingId=N, N.beforeSiblingId=A

- [ ] **Creating first child** - beforeSiblingId is null
  - Given: Parent with no children
  - When: Create first child
  - Then: Child.beforeSiblingId = null

### Deletion
- [ ] **Deleting node repairs chain** - Next sibling updated
  - Given: Siblings [A, B, C] where C.beforeSiblingId=B
  - When: Delete B
  - Then: C.beforeSiblingId=A

- [ ] **Deleting last sibling** - No updates needed
  - Given: Siblings [A, B] where B is last
  - When: Delete B
  - Then: A.beforeSiblingId unchanged

### Movement
- [ ] **Moving node removes from old chain** - No orphaned references
  - Given: Node in sibling chain
  - When: Move to different parent (indent)
  - Then: Old sibling chain repaired, no dangling references

### Validation
- [ ] **Multiple siblings maintain order** - 3+ siblings correct
  - Given: Siblings [A, B, C, D]
  - Then: Can traverse A→B→C→D via beforeSiblingId

- [ ] **Exactly one first child** - Only one null beforeSiblingId
  - Given: Parent with children
  - Then: Exactly one child has beforeSiblingId=null

- [ ] **No circular references** - Chain doesn't loop
  - Given: Any sibling chain
  - Then: Traversing chain terminates (no infinite loop)

- [ ] **No orphaned nodes** - All nodes reachable
  - Given: Parent with children
  - Then: All children reachable from first child by following chain

---

## 6. Node Ordering (insertAtBeginning)

### Visual Order Validation
- [ ] **insertAtBeginning=true visual order** - New node appears BEFORE
  - Given: Node A "First"
  - When: Create node with insertAtBeginning=true
  - Then: Visual order is [NewNode, A]

- [ ] **insertAtBeginning=false visual order** - New node appears AFTER
  - Given: Node A "First"
  - When: Create node normally (Enter at end)
  - Then: Visual order is [A, NewNode]

- [ ] **Multiple insertAtBeginning operations** - Order correct
  - Given: Node A
  - When: Create B with insertAtBeginning=true, then C with insertAtBeginning=true
  - Then: Visual order is [C, B, A]

### Nested Operations
- [ ] **insertAtBeginning for child nodes** - Works in nested context
  - Given: Parent with child A
  - When: Create child B with insertAtBeginning=true
  - Then: Visual order is [B, A] within parent

- [ ] **Deep hierarchy ordering** - Maintains order at all levels
  - Given: Deep hierarchy (depth 4+)
  - When: Create nodes with various insertAtBeginning values
  - Then: Each level maintains correct visual order

### Mixed Operations
- [ ] **Mix of insertAtBeginning modes** - Both modes work together
  - Given: Node A
  - When: Create B (normal), create C (insertAtBeginning=true), create D (normal)
  - Then: Visual order matches creation intent

### Header Nodes
- [ ] **Header with insertAtBeginning** - Correct positioning
  - Given: Header node "## Header"
  - When: Press Enter at beginning
  - Then: New header created before, order correct

---

## 7. Database Persistence

### Placeholder Behavior
- [ ] **Empty placeholder not persisted** - Only exists in memory
  - Given: Empty node created (Press Enter)
  - When: Check database
  - Then: Node does NOT exist in database

- [ ] **Node persisted after content added** - First character triggers
  - Given: Empty placeholder
  - When: Type one character
  - Then: Node appears in database

### Update Operations
- [ ] **Updates persist correctly** - Edits saved
  - Given: Persisted node "Hello"
  - When: Edit to "Hello World"
  - Then: Database reflects "Hello World"

- [ ] **Sibling updates persist** - beforeSiblingId changes saved
  - Given: Sibling chain in database
  - When: Reorder via indent/outdent
  - Then: Database reflects new beforeSiblingId values

### Concurrency
- [ ] **No duplicate creates** - UNIQUE constraint not violated
  - Given: Rapid typing in new node
  - When: Multiple updates queued
  - Then: No UNIQUE constraint errors

- [ ] **No database locking errors** - Concurrent writes handled
  - Given: Multiple nodes being edited
  - When: Rapid operations (type, indent, delete)
  - Then: No "database is locked" errors

### Deletion
- [ ] **Delete cascades properly** - Children handled first
  - Given: Node with children
  - When: Delete node
  - Then: Children reassigned before database CASCADE

- [ ] **No duplicate deletes** - Delete called once
  - Given: Node to delete
  - When: Merge/delete operation
  - Then: Database delete called exactly once

### Load Operations
- [ ] **No write-back loop on load** - Loading doesn't trigger writes
  - Given: Database with nodes
  - When: Load nodes on app start
  - Then: No database write operations triggered

---

## 8. Event System

### Event Emission
- [ ] **Events fire once per operation** - No duplicates
  - Given: Any node operation
  - When: Execute operation
  - Then: Exactly one event fired (not 2+)

- [ ] **node:created event fires** - On node creation
  - Given: Create new node
  - When: Press Enter
  - Then: 'node:created' event emitted with nodeId

- [ ] **node:updated event fires** - On content/property change
  - Given: Edit node content
  - When: Type character
  - Then: 'node:updated' event emitted

- [ ] **node:deleted event fires** - On deletion
  - Given: Delete node
  - When: Backspace merge or explicit delete
  - Then: 'node:deleted' event emitted

- [ ] **hierarchy:changed event fires** - On structural changes
  - Given: Indent/outdent operation
  - When: Execute indent
  - Then: 'hierarchy:changed' event emitted

### Event Ordering
- [ ] **Event sequence is correct** - Events in logical order
  - Given: Create node, edit content, delete
  - When: Operations execute
  - Then: Events fire in order: created → updated → deleted

---

## 9. Content Processing

### Markdown Parsing
- [ ] **Header detection** - Parse header levels
  - Given: Content "## Header"
  - When: Parse content
  - Then: Detected as level 2 header

- [ ] **Inline formatting detection** - Bold, italic, code, strikethrough
  - Given: Content with mixed formatting
  - When: Parse content
  - Then: All formats detected correctly

### @Mention Detection
- [ ] **@mention parsing** - Detect node references
  - Given: Content "Hello @node-123"
  - When: Parse content
  - Then: Mention detected, nodeId extracted

- [ ] **Multiple @mentions** - Handle multiple references
  - Given: Content with 3 @mentions
  - When: Parse content
  - Then: All 3 mentions detected

### Content Debouncing
- [ ] **Fast operations debounced** - Reference updates batched
  - Given: Rapid typing (10 characters in 100ms)
  - When: Typing completes
  - Then: Reference update called once, not 10 times

- [ ] **Expensive operations debounced** - Vector embeddings batched
  - Given: Rapid typing
  - When: Pause after typing
  - Then: Expensive operation (embedding) called once after debounce

### Dual Representation
- [ ] **Display vs markdown consistency** - Both representations match
  - Given: Content with formatting "**bold**"
  - When: Convert display → markdown → display
  - Then: Round-trip preserves content

---

## 10. Edge Cases & Regressions

### Slash Commands
- [ ] **Slash command dropdown shows** - Typing "/" triggers dropdown
  - Given: Empty node
  - When: Type "/"
  - Then: Dropdown appears with command options

- [ ] **Slash command selection works** - Click/Enter selects
  - Given: Dropdown visible with options
  - When: Click option or press Enter
  - Then: Command executed, dropdown closes

- [ ] **Slash command keyboard navigation** - Arrow keys work
  - Given: Dropdown with 3 options
  - When: Press ArrowDown twice
  - Then: Third option highlighted

### Cursor Positioning
- [ ] **Cursor at beginning of formatted text**
  - Given: Content "**bold**", cursor at position 0
  - When: Type character
  - Then: Character inserted correctly

- [ ] **Cursor at end of formatted text**
  - Given: Content "**bold**", cursor at end
  - When: Type character
  - Then: Character inserted after closing markers

- [ ] **Cursor in middle of markers**
  - Given: Content "**|bold**" (cursor between **)
  - When: Type character
  - Then: Character handled correctly (don't break markers)

### Empty Content
- [ ] **Empty node behaviors** - Handle whitespace-only
  - Given: Node with content "   " (spaces only)
  - When: Operations performed
  - Then: Treated as empty appropriately

- [ ] **Placeholder node lifecycle** - Create → type → persist
  - Given: Empty placeholder
  - When: User types, then deletes all content
  - Then: Node becomes placeholder again

### Performance
- [ ] **Large hierarchy performance** - 100+ nodes responsive
  - Given: 100 nodes, 5 levels deep
  - When: Create, indent, outdent operations
  - Then: Operations complete in <100ms

- [ ] **Deep nesting performance** - 10+ levels deep
  - Given: Node at depth 10
  - When: Indent/outdent operations
  - Then: Depth recalculation completes quickly

### Multiple Viewers (Future)
- [ ] **Multi-viewer state sync** - Changes propagate
  - Given: Two viewers of same parent node
  - When: Edit in viewer 1
  - Then: Viewer 2 sees change immediately

- [ ] **Multi-viewer cursor isolation** - Independent cursors
  - Given: Two viewers
  - When: Each has different active node
  - Then: Cursors don't interfere

---

## 11. Integration Tests (End-to-End)

### Complete Workflows
- [ ] **Create → Edit → Indent → Delete** - Full lifecycle
  - Given: Empty document
  - When: Execute full workflow
  - Then: Each step works correctly, state consistent

- [ ] **Build outline structure** - Realistic usage
  - Given: Empty document
  - When: Create multi-level outline (headers, bullets, tasks)
  - Then: Structure reflects intent, all operations work

- [ ] **Rapid editing session** - Stress test
  - Given: Document with content
  - When: Type rapidly, press Enter 10x, indent/outdent 5x
  - Then: No errors, final state correct

### Persistence Integration
- [ ] **Create → Save → Reload** - Persistence round-trip
  - Given: Create and edit nodes
  - When: Reload app
  - Then: All nodes restored with correct content and structure

- [ ] **Edit → Save → Reload** - Update persistence
  - Given: Existing nodes
  - When: Edit and reload
  - Then: Edits persisted correctly

---

## 12. Regression Prevention (Known Issues)

Based on your commit history, these specific bugs must stay fixed:

- [ ] **Issue #185** - Shift+Enter inline formatting splitting works
- [ ] **Issue #184** - Outdent sibling chain integrity maintained
- [ ] **Issue #176** - Cursor positioning for arrow navigation and Shift+Enter
- [ ] **Issue #190** - Database locking race condition on deletion
- [ ] **Issue #187** - Slash command selection callback wired correctly
- [ ] **Last character leak** - Enter at end doesn't leave trailing chars
- [ ] **Cursor jump on Enter** - No unwanted cursor movement
- [ ] **Children disappearing** - Backspace merge preserves children
- [ ] **Header syntax on Enter at beginning** - Preserved correctly

---

## Implementation Notes

### Test Organization
```
src/tests/
├── integration/
│   ├── enter-key-operations.test.ts (Section 1)
│   ├── shift-enter-newlines.test.ts (Section 2)
│   ├── backspace-merge.test.ts (Section 3)
│   ├── indent-outdent.test.ts (Section 4)
│   └── complete-workflows.test.ts (Section 11)
├── services/
│   ├── sibling-chain-integrity.test.ts (Section 5)
│   ├── node-ordering.test.ts (Section 6)
│   └── database-persistence.test.ts (Section 7)
└── regression/
    └── known-issues.test.ts (Section 12)
```

### Test Assertions
Each test should verify:
1. **In-memory state** - `_nodes` and `_uiState` correct
2. **Visual order** - `getVisibleNodes()` returns correct order
3. **Database state** - Nodes persisted (or not) as expected
4. **Event emissions** - Correct events fired exactly once
5. **No console errors** - No unexpected errors logged

### Running Tests
```bash
# Run all tests
bun run test

# Run specific category
bunx vitest run src/tests/integration/shift-enter-newlines.test.ts

# Watch mode for TDD
bunx vitest src/tests/integration/enter-key-operations.test.ts
```

---

## Current Test Status (Baseline: commit b4b8270)

This version (before SharedNodeStore refactor) should pass all manual tests.
Use this as the **baseline** for TDD:

1. Write test for a case
2. Verify test PASSES on b4b8270
3. Apply SharedNodeStore refactor
4. If test FAILS, fix SharedNodeStore until it passes
5. Repeat for all cases

**Goal:** 100% of these cases have automated tests that catch regressions.
