# ADR: ContentEditable Text Editing with Smart Markdown Conversion

**Date**: January 2025  
**Status**: Accepted  
**Decision Makers**: NodeSpace Core Team  

## Context

The current CodeMirror-based hybrid markdown rendering implementation has fundamental cursor positioning issues and architectural complexity that fights against web standards. After extensive research including UX analysis of Logseq and mldoc systems, we need a simpler, more reliable approach.

## Decision

Replace CodeMirror with **ContentEditable-based text editing** featuring **smart markdown-to-node conversion**.

### Key Innovation: Markdown Syntax → Physical Node Hierarchy

Instead of competing hierarchy systems, convert markdown syntax into NodeSpace's existing physical node relationships:

- `- Research tasks` → Removes `- `, creates child node relationship
- Soft newline + markdown → Creates appropriate new nodes intelligently  
- Physical nodes serve as the "list structure" - no visual bullets needed

## Rationale

### Technical Benefits
- ✅ Eliminates cursor positioning issues (ContentEditable handles naturally)
- ✅ Simpler architecture (standard web patterns vs complex hybrid rendering)  
- ✅ Better performance (native browser optimizations)
- ✅ Smaller bundle (~19% reduction, removes CodeMirror dependency)

### User Experience Benefits  
- ✅ Fast markdown typing (`# Header`, `- bullet` syntax)
- ✅ Intelligent hierarchy building (bullets → actual parent/child nodes)
- ✅ No competing mental models (one hierarchy system)  
- ✅ Perfect AI integration (seamless ChatGPT/Claude markdown import)

### Architectural Benefits
- ✅ Preserves existing keyboard system (Tab/Shift-Tab indenting)
- ✅ Enhances node capabilities (each "bullet" becomes full-featured node)
- ✅ Consistent with NodeSpace design system and patterns

## Implementation

**Phase 1**: ContentEditable foundation with basic WYSIWYG  
**Phase 2**: Smart node conversion (bullet-to-node, soft newline intelligence)  
**Phase 3**: AI integration and performance optimization

## Consequences

### Positive
- Solves fundamental cursor positioning problems
- Provides intuitive markdown typing experience
- Enables seamless AI content integration
- Maintains all existing functionality while enhancing capabilities

### Negative  
- Requires significant refactoring of text editing components
- ContentEditable has its own challenges (browser inconsistencies)
- Need to handle edge cases in markdown-to-node conversion

### Mitigation
- Keep current CodeMirror work as reference branch
- Comprehensive testing across browsers and typing patterns
- Gradual migration with backward compatibility

## Related Documents

- [Complete Architecture Documentation](../development/contenteditable-text-editing.md)
- [Original Issue #26](https://github.com/malibio/nodespace-core/issues/26)
- [UX Research Analysis](../development/contenteditable-text-editing.md#research-findings)

## Migration Path

1. **Preserve existing work**: Keep `feature/issue-26-hybrid-markdown-reference` branch
2. **Start fresh**: Create `feature/contenteditable-text-editing` from main
3. **Cherry-pick valuable components**: Performance improvements, documentation, design system integration
4. **Maintain compatibility**: All existing keyboard shortcuts and node relationships preserved

---

This decision represents a pivot from fighting against web standards to leveraging them, resulting in a more reliable, performant, and user-friendly text editing experience that enhances rather than competes with NodeSpace's sophisticated node hierarchy system.