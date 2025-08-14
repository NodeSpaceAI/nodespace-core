# BaseNode → MinimalBaseNode Transition Guide

## Overview
The original `BaseNode` component is being replaced by `MinimalBaseNode` as the foundation component for all node types.

### Why the Change?
- **Cleaner API**: Streamlined props and more focused functionality
- **Better Performance**: Reduced reactive loops and overhead
- **Enhanced Extensibility**: Designed specifically for multiple node types
- **Improved Maintainability**: Clearer separation of concerns

### Migration Status
- ✅ **MinimalBaseNode**: Fully implemented with multiline support
- ✅ **TextNode**: Updated to use MinimalBaseNode with header functionality
- ✅ **BaseNodeViewer**: Updated to work with new architecture
- ❌ **BaseNode**: Legacy component to be removed once migration is complete

### Key Differences

#### Props Interface
```typescript
// OLD: BaseNode (legacy)
export let content: string;
export let nodeId: string;

// NEW: MinimalBaseNode (current)
export let nodeId: string;
export let nodeType: string = 'text';
export let autoFocus: boolean = false;
export let allowNewNodeOnEnter: boolean = true;
export let splitContentOnEnter: boolean = false;
export let multiline: boolean = false;
export let content: string = '';
export let className: string = '';
```

#### Event System
```typescript
// NEW: Enhanced event system
contentChanged: { content: string }
createNewNode: { 
  afterNodeId: string; 
  nodeType: string; 
  currentContent?: string; 
  newContent?: string;
  inheritHeaderLevel?: number; // TextNode extension
}
```

#### CSS Classes
```css
/* NEW: Multiline support */
.markdown-editor--singleline { /* single-line mode */ }
.markdown-editor--multiline { /* multiline mode */ }

/* NEW: Component-specific styling */
.text-node--h1 .markdown-editor { /* header styling */ }
```

### Migration Checklist for Future Components

When creating new node types or updating existing ones:

- [ ] Use `MinimalBaseNode` as the foundation
- [ ] Pass appropriate props (`nodeType`, `multiline`, etc.)
- [ ] Handle new event signatures
- [ ] Use CSS classes for styling
- [ ] Test multiline behavior if applicable
- [ ] Update TypeScript interfaces
- [ ] Remove any BaseNode dependencies

### Files to Update (Future)
- Any components currently importing `BaseNode`
- CSS files with BaseNode-specific classes
- Test files referencing BaseNode
- Documentation mentioning BaseNode

---

**Current Status**: TextNode migration complete. BaseNode removal pending after all components updated.