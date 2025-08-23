# Advanced Node Hierarchy System

## Overview

The NodeSpace Advanced Node Hierarchy System provides sophisticated document organization capabilities through intelligent node relationships, keyboard-driven navigation, and context-aware operations. This system enables users to create complex document structures with nested content while maintaining intuitive editing workflows.

## Core Concepts

### Node Types & Capabilities

#### Text Nodes (`type: 'text'`)
- **Purpose**: Standard content nodes for paragraphs, headers, and formatted text
- **Capabilities**: Can have children, can be combined (with restrictions)
- **Behavior**: Support all hierarchy operations and formatting
- **Visual**: Green circle (leaf) or ring (parent) indicator

#### AI-Chat Nodes (`type: 'ai-chat'`)  
- **Purpose**: AI conversation nodes with special structural requirements
- **Capabilities**: Cannot have children, cannot be combined
- **Behavior**: Limited hierarchy operations to preserve conversation structure
- **Visual**: Blue circle indicator (always leaf)

### Node Properties

#### `canHaveChildren`
- **Text nodes**: `true` - Can become parent nodes
- **AI-chat nodes**: `false` - Always remain leaf nodes
- **Impact**: Controls indentation validation and visual indicators

#### `canBeCombined` (Dynamic Function)
- **Regular text**: `true` - Can be joined with adjacent nodes
- **Headers** (`# Header`): `false` - Cannot be combined to preserve structure
- **AI-chat**: `false` - Cannot be combined to preserve conversation integrity

## Keyboard Operations

### Indentation Controls

#### Tab Key - Indent Node
```
Current structure:     After Tab:
• Node A               • Node A  
• Node B         →       └─ • Node B (child of A)
• Node C               • Node C
```

**Rules**:
- Node becomes child of previous sibling
- Only works if previous sibling `canHaveChildren`
- AI-chat nodes are blocked from becoming children
- Children transfer with parent during subsequent operations

#### Shift+Tab - Outdent Node
```
Current structure:     After Shift+Tab:  
• Node A               • Node A
  └─ • Node B    →     • Node B (sibling of A)
  └─ • Node C            └─ • Node C (child of B)
```

**Rules**:
- Node moves up one hierarchy level
- Siblings below the outdented node become its children
- Maintains relative hierarchy relationships
- Cannot outdent root-level nodes

### Content Operations

#### Backspace - Node Joining
**At beginning of node** (cursor position 0):

```
Before:                After:
• Simple text...       • Simple text...Another text
• Another text   →     (cursor positioned at junction)
```

**Combination Rules**:
```typescript
canBeCombinedWith() {
  // AI-chat nodes cannot be combined
  if (nodeType === 'ai-chat') return false;
  
  // Headers cannot be combined  
  if (content.startsWith('#')) {
    const headerMatch = content.match(/^#{1,6}\s/);
    if (headerMatch) return false;
  }
  
  // Regular text nodes can be combined
  return true;
}
```

**Junction Cursor Positioning**:
- Accounts for HTML formatting spans
- Positions at exact merge point between content
- Works with bold, italic, underline formatting
- Example: "text**bold**" + "more" → cursor after "bold", before "more"

#### Enter - New Node Creation
**Content splitting** with formatting preservation:

```
Before (cursor at |):     After:
• Some text|more content   • Some text
                          • more content (formatting preserved)
```

**Children Transfer Rules**:
- **Expanded parent**: New node inherits children
- **Collapsed parent**: Children stay with original node
- **Header levels**: New nodes inherit appropriate header context

## Visual Hierarchy System

### Node Indicators
- **Circle** (`○`): Leaf node (no children)
- **Circle Ring** (`◉`): Parent node (has children)
- **Color coding**: Green (text), Blue (ai-chat), etc.

### Expand/Collapse Controls
- **Chevron** (`>`): Appears on hover for parent nodes
- **Click behavior**: Toggles children visibility
- **Keyboard**: Future enhancement for arrow key navigation
- **State preservation**: Collapsed state affects children transfer

### Layout Structure
```
[Icon] [Chevron?] [Content Area]
  ○        >        Some text content...
         space      └─ Child node
```

## Content Preservation & Formatting

### HTML-Markdown Conversion
The system maintains content in markdown format while displaying HTML:

**Storage** (Markdown):
```markdown
Some **bold** and *italic* text
```

**Display** (HTML):
```html
Some <span class="markdown-bold">bold</span> and <span class="markdown-italic">italic</span> text
```

### Inline Formatting Support
- **Bold**: `Cmd+B` / `Ctrl+B` → `**text**`
- **Italic**: `Cmd+I` / `Ctrl+I` → `*text*`  
- **Underline**: `Cmd+U` / `Ctrl+U` → `__text__`
- **Selection preservation**: Formatting maintains text selection
- **Toggle support**: Repeated shortcuts toggle formatting on/off

### Header Management
**Syntax detection**:
```
Type: "## "  →  Automatic H2 styling
Type: "### " →  Automatic H3 styling
```

**Rules**:
- Headers 1-6 supported (`#` to `######`)
- Automatic syntax removal and CSS styling application
- Headers cannot be combined (preserve document structure)
- New nodes inherit header context when appropriate

## Performance & Architecture

### Reactive System
- **Smart updates**: Differentiates user input vs external changes
- **No focus loss**: Typing remains smooth during all operations
- **Minimal DOM manipulation**: Svelte handles most updates automatically
- **Event-driven**: Clean component communication with custom events

### Component Architecture
```
BaseNodeViewer (Container)
├── TextNode (Type-specific wrapper)  
│   └── MinimalBaseNode (Generic base)
│       └── ContentEditable element
└── [Other node types...]
```

**Responsibilities**:
- **BaseNodeViewer**: Hierarchy management, node operations
- **TextNode**: Type-specific logic, header handling  
- **MinimalBaseNode**: Core editing, keyboard shortcuts, formatting

### Memory Management
- **No memory leaks**: Proper cleanup of DOM references
- **Efficient updates**: Only changed nodes re-render
- **Event cleanup**: Automatic listener cleanup on component destroy

## Usage Examples

### Basic Hierarchy Creation
1. Type content in first node
2. Press `Enter` to create new node
3. Press `Tab` to indent (make child)
4. Press `Shift+Tab` to outdent (make sibling)

### Content Joining  
1. Position cursor at start of second node
2. Press `Backspace` to join with previous node
3. Cursor positions at junction point
4. Continue typing seamlessly

### Header Workflows
1. Type `## ` at start of line → Automatic H2 styling
2. Headers cannot be joined (preserved structure)
3. New nodes after headers inherit context

### Advanced Operations
1. **Bulk indentation**: Select nodes, use Tab/Shift+Tab
2. **Children inheritance**: Expand/collapse affects Enter behavior  
3. **Format preservation**: All operations maintain text formatting

## Error Handling & Edge Cases

### Invalid Operations
- **Tab on first node**: No previous sibling → No action
- **Shift+Tab on root**: Cannot outdent → No action
- **Join incompatible nodes**: Header + text → No action
- **AI-chat children**: Tab blocked → No action

### Boundary Conditions  
- **Empty nodes**: Can be joined and removed
- **Single character nodes**: Proper cursor positioning
- **Unicode content**: Full grapheme cluster support
- **Long content**: Performance optimized for large documents

### Recovery Patterns
- **Failed operations**: No state corruption, clean rollback
- **Network issues**: Local-first operations continue working
- **Browser compatibility**: Graceful degradation for older browsers

## Best Practices

### Content Organization
- Use headers for major sections (cannot be joined accidentally)
- Group related content under parent nodes  
- Use AI-chat nodes for conversation threads (preserved structure)
- Leverage indentation for logical document hierarchy

### Keyboard Efficiency
- **Tab/Shift+Tab**: Quick hierarchy changes
- **Enter**: Split content while preserving formatting
- **Backspace**: Join related content seamlessly
- **Cmd+B/I/U**: Format without breaking flow

### Performance Optimization
- Avoid excessive nesting (diminishing returns after 6+ levels)
- Use headers for structure instead of deep indentation
- Batch operations when possible (future enhancement)
- Leverage collapsed state for large document sections

## Technical Implementation Details

### Event Flow
```
User Action → Component Event → BaseNodeViewer Handler → Data Update → Svelte Reactivity → DOM Update
```

### Cursor Position Calculation
```typescript  
// Account for HTML formatting in position calculation
const htmlContent = markdownToHtml(markdownContent);
const textLength = htmlContent.textContent.length;
// Walk DOM text nodes to find exact position
const targetPosition = walkTextNodes(element, textLength);
```

### Content Merging Algorithm
```typescript
function handleContentMerge(currentNode, prevNode) {
  const junctionPosition = prevNode.content.length;
  prevNode.content = prevNode.content + currentNode.content;
  transferChildren(currentNode, prevNode);
  removeNode(currentNode);
  positionCursor(prevNode, junctionPosition);
}
```

This comprehensive system provides the foundation for sophisticated document editing while maintaining intuitive user interactions and robust technical architecture.