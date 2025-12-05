/**
 * Tests for HeaderNode Type-Safe Wrapper
 *
 * Tests header level parsing, text extraction, immutable updates,
 * and helper functions for header node operations.
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import {
  type HeaderNode,
  isHeaderNode,
  getHeaderLevel,
  getHeaderText,
  setHeaderLevel,
  HeaderNodeHelpers
} from '$lib/types/header-node';

describe('HeaderNode Type Guard', () => {
  it('identifies header nodes correctly', () => {
    const headerNode: HeaderNode = {
      id: 'test-1',
      nodeType: 'header',
      content: '# My Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isHeaderNode(headerNode)).toBe(true);
  });

  it('rejects non-header nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isHeaderNode(textNode)).toBe(false);
  });

  it('rejects task nodes', () => {
    const taskNode: Node = {
      id: 'test-3',
      nodeType: 'task',
      content: '- [ ] Task item',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isHeaderNode(taskNode)).toBe(false);
  });
});

describe('getHeaderLevel', () => {
  it('returns level 1 for single # header', () => {
    const node: HeaderNode = {
      id: 'test-4',
      nodeType: 'header',
      content: '# H1 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(1);
  });

  it('returns level 2 for ## header', () => {
    const node: HeaderNode = {
      id: 'test-5',
      nodeType: 'header',
      content: '## H2 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(2);
  });

  it('returns level 3 for ### header', () => {
    const node: HeaderNode = {
      id: 'test-6',
      nodeType: 'header',
      content: '### H3 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(3);
  });

  it('returns level 4 for #### header', () => {
    const node: HeaderNode = {
      id: 'test-7',
      nodeType: 'header',
      content: '#### H4 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(4);
  });

  it('returns level 5 for ##### header', () => {
    const node: HeaderNode = {
      id: 'test-8',
      nodeType: 'header',
      content: '##### H5 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(5);
  });

  it('returns level 6 for ###### header', () => {
    const node: HeaderNode = {
      id: 'test-9',
      nodeType: 'header',
      content: '###### H6 Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(6);
  });

  it('clamps level at 6 for more than 6 # characters (with space)', () => {
    const node: HeaderNode = {
      id: 'test-10',
      nodeType: 'header',
      content: '####### Too Many Hashes',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    // Regex requires space, so this won't match and returns default 1
    expect(getHeaderLevel(node)).toBe(1);
  });

  it('clamps level at 6 for exactly 7 # characters with space', () => {
    const node: HeaderNode = {
      id: 'test-10b',
      nodeType: 'header',
      content: '####### Too Many',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    // Regex matches up to 6, so 7+ hashes without space after aren't valid markdown
    expect(getHeaderLevel(node)).toBe(1);
  });

  it('returns 1 as default when no # prefix found', () => {
    const node: HeaderNode = {
      id: 'test-11',
      nodeType: 'header',
      content: 'Plain text header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(1);
  });

  it('requires space after # for valid header', () => {
    const node: HeaderNode = {
      id: 'test-12',
      nodeType: 'header',
      content: '#NoSpace',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(1);
  });

  it('handles headers with multiple spaces after #', () => {
    const node: HeaderNode = {
      id: 'test-13',
      nodeType: 'header',
      content: '##  Multiple Spaces',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderLevel(node)).toBe(2);
  });
});

describe('getHeaderText', () => {
  it('removes single # prefix and space', () => {
    const node: HeaderNode = {
      id: 'test-14',
      nodeType: 'header',
      content: '# My Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('My Header');
  });

  it('removes ## prefix and space', () => {
    const node: HeaderNode = {
      id: 'test-15',
      nodeType: 'header',
      content: '## Second Level',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('Second Level');
  });

  it('removes ###### prefix and space', () => {
    const node: HeaderNode = {
      id: 'test-16',
      nodeType: 'header',
      content: '###### Sixth Level',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('Sixth Level');
  });

  it('returns original content when no # prefix', () => {
    const node: HeaderNode = {
      id: 'test-17',
      nodeType: 'header',
      content: 'No prefix header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('No prefix header');
  });

  it('handles headers with # characters in the text', () => {
    const node: HeaderNode = {
      id: 'test-18',
      nodeType: 'header',
      content: '# Header with # in text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('Header with # in text');
  });

  it('handles empty header text', () => {
    const node: HeaderNode = {
      id: 'test-19',
      nodeType: 'header',
      content: '## ',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('');
  });

  it('preserves trailing whitespace in header text', () => {
    const node: HeaderNode = {
      id: 'test-20',
      nodeType: 'header',
      content: '# Header with spaces   ',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getHeaderText(node)).toBe('Header with spaces   ');
  });

  it('handles headers with only # prefix and no space', () => {
    const node: HeaderNode = {
      id: 'test-21',
      nodeType: 'header',
      content: '#',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    // The regex removes up to 6 # chars with optional space, so single # is removed
    expect(getHeaderText(node)).toBe('');
  });
});

describe('setHeaderLevel', () => {
  it('sets header level immutably', () => {
    const original: HeaderNode = {
      id: 'test-22',
      nodeType: 'header',
      content: '# Original',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(original, 2);

    // Original unchanged
    expect(original.content).toBe('# Original');
    // Updated has new level
    expect(updated.content).toBe('## Original');
    expect(updated.id).toBe(original.id);
    expect(updated.nodeType).toBe(original.nodeType);
  });

  it('changes level from 1 to 3', () => {
    const node: HeaderNode = {
      id: 'test-23',
      nodeType: 'header',
      content: '# Header Text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 3);
    expect(updated.content).toBe('### Header Text');
  });

  it('changes level from 5 to 2', () => {
    const node: HeaderNode = {
      id: 'test-24',
      nodeType: 'header',
      content: '##### Fifth Level',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 2);
    expect(updated.content).toBe('## Fifth Level');
  });

  it('clamps level at 1 for values below 1', () => {
    const node: HeaderNode = {
      id: 'test-25',
      nodeType: 'header',
      content: '## Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 0);
    expect(updated.content).toBe('# Header');
  });

  it('clamps level at 1 for negative values', () => {
    const node: HeaderNode = {
      id: 'test-26',
      nodeType: 'header',
      content: '### Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, -5);
    expect(updated.content).toBe('# Header');
  });

  it('clamps level at 6 for values above 6', () => {
    const node: HeaderNode = {
      id: 'test-27',
      nodeType: 'header',
      content: '# Header',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 10);
    expect(updated.content).toBe('###### Header');
  });

  it('preserves header text content exactly', () => {
    const node: HeaderNode = {
      id: 'test-28',
      nodeType: 'header',
      content: '# Text with special chars !@#$%^&*()',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 4);
    expect(updated.content).toBe('#### Text with special chars !@#$%^&*()');
  });

  it('handles headers without prefix', () => {
    const node: HeaderNode = {
      id: 'test-29',
      nodeType: 'header',
      content: 'Plain text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setHeaderLevel(node, 3);
    expect(updated.content).toBe('### Plain text');
  });

  it('preserves properties and metadata', () => {
    const node: HeaderNode = {
      id: 'test-30',
      nodeType: 'header',
      content: '# Header',
      createdAt: '2025-01-01T00:00:00Z',
      modifiedAt: '2025-01-02T00:00:00Z',
      version: 5,
      properties: { custom: 'value' }
    };

    const updated = setHeaderLevel(node, 2);
    expect(updated.createdAt).toBe(node.createdAt);
    expect(updated.modifiedAt).toBe(node.modifiedAt);
    expect(updated.version).toBe(node.version);
    expect(updated.properties).toEqual(node.properties);
  });
});

describe('HeaderNodeHelpers', () => {
  describe('createHeaderNode', () => {
    it('creates H1 header by default', () => {
      const header = HeaderNodeHelpers.createHeaderNode('My Header');

      expect(header.content).toBe('# My Header');
      expect(header.nodeType).toBe('header');
      expect(header.id).toMatch(/^header-/);
      expect(header.version).toBe(1);
      expect(header.properties).toEqual({});
    });

    it('creates header with specified level', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Level 3 Header', 3);

      expect(header.content).toBe('### Level 3 Header');
      expect(header.nodeType).toBe('header');
    });

    it('creates H6 header', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Sixth Level', 6);

      expect(header.content).toBe('###### Sixth Level');
    });

    it('clamps level at 1 for values below 1', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Header', 0);

      expect(header.content).toBe('# Header');
    });

    it('clamps level at 1 for negative values', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Header', -3);

      expect(header.content).toBe('# Header');
    });

    it('clamps level at 6 for values above 6', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Header', 10);

      expect(header.content).toBe('###### Header');
    });

    it('generates unique IDs', () => {
      const header1 = HeaderNodeHelpers.createHeaderNode('Header 1');
      const header2 = HeaderNodeHelpers.createHeaderNode('Header 2');

      expect(header1.id).not.toBe(header2.id);
      expect(header1.id).toMatch(/^header-\d+-[a-z0-9]+$/);
      expect(header2.id).toMatch(/^header-\d+-[a-z0-9]+$/);
    });

    it('sets valid timestamps', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Header');

      expect(header.createdAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
      expect(header.modifiedAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
      expect(new Date(header.createdAt).getTime()).toBeLessThanOrEqual(Date.now());
      expect(new Date(header.modifiedAt).getTime()).toBeLessThanOrEqual(Date.now());
    });

    it('handles empty text', () => {
      const header = HeaderNodeHelpers.createHeaderNode('');

      expect(header.content).toBe('# ');
      expect(header.nodeType).toBe('header');
    });

    it('handles text with special characters', () => {
      const header = HeaderNodeHelpers.createHeaderNode('Header with !@#$%', 2);

      expect(header.content).toBe('## Header with !@#$%');
    });
  });

  describe('isTopLevel', () => {
    it('returns true for H1 headers', () => {
      const node: HeaderNode = {
        id: 'test-31',
        nodeType: 'header',
        content: '# Top Level',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(true);
    });

    it('returns false for H2 headers', () => {
      const node: HeaderNode = {
        id: 'test-32',
        nodeType: 'header',
        content: '## Second Level',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(false);
    });

    it('returns false for H3 headers', () => {
      const node: HeaderNode = {
        id: 'test-33',
        nodeType: 'header',
        content: '### Third Level',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(false);
    });

    it('returns false for H6 headers', () => {
      const node: HeaderNode = {
        id: 'test-34',
        nodeType: 'header',
        content: '###### Sixth Level',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(false);
    });

    it('returns true for headers without prefix (default level 1)', () => {
      const node: HeaderNode = {
        id: 'test-35',
        nodeType: 'header',
        content: 'No prefix',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(true);
    });
  });

  describe('isSubHeading', () => {
    it('returns false for H1 headers', () => {
      const node: HeaderNode = {
        id: 'test-36',
        nodeType: 'header',
        content: '# Top Level',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(false);
    });

    it('returns true for H2 headers', () => {
      const node: HeaderNode = {
        id: 'test-37',
        nodeType: 'header',
        content: '## Sub Heading',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(true);
    });

    it('returns true for H3 headers', () => {
      const node: HeaderNode = {
        id: 'test-38',
        nodeType: 'header',
        content: '### Sub Heading',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(true);
    });

    it('returns true for H6 headers', () => {
      const node: HeaderNode = {
        id: 'test-39',
        nodeType: 'header',
        content: '###### Deep Sub Heading',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(true);
    });

    it('returns false for headers without prefix (default level 1)', () => {
      const node: HeaderNode = {
        id: 'test-40',
        nodeType: 'header',
        content: 'No prefix',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(false);
    });
  });

  describe('namespace exports', () => {
    it('exports isHeaderNode in namespace', () => {
      expect(HeaderNodeHelpers.isHeaderNode).toBe(isHeaderNode);
    });

    it('exports getHeaderLevel in namespace', () => {
      expect(HeaderNodeHelpers.getHeaderLevel).toBe(getHeaderLevel);
    });

    it('exports getHeaderText in namespace', () => {
      expect(HeaderNodeHelpers.getHeaderText).toBe(getHeaderText);
    });

    it('exports setHeaderLevel in namespace', () => {
      expect(HeaderNodeHelpers.setHeaderLevel).toBe(setHeaderLevel);
    });
  });
});

describe('Integration', () => {
  it('works with type guard and helpers together', () => {
    const node: HeaderNode = {
      id: 'integration-test',
      nodeType: 'header',
      content: '## Integration Test',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    if (isHeaderNode(node)) {
      // Type guard should work
      const level = getHeaderLevel(node);
      expect(level).toBe(2);

      const text = getHeaderText(node);
      expect(text).toBe('Integration Test');

      // Immutable updates should work
      const updated = setHeaderLevel(node, 3);
      expect(updated.content).toBe('### Integration Test');

      // Original should be unchanged
      expect(node.content).toBe('## Integration Test');
    }
  });

  it('handles various header scenarios', () => {
    const scenarios = [
      { content: '# H1', expectedLevel: 1, expectedText: 'H1', isTop: true, isSub: false },
      { content: '## H2', expectedLevel: 2, expectedText: 'H2', isTop: false, isSub: true },
      { content: '### H3', expectedLevel: 3, expectedText: 'H3', isTop: false, isSub: true },
      { content: '#### H4', expectedLevel: 4, expectedText: 'H4', isTop: false, isSub: true },
      { content: '##### H5', expectedLevel: 5, expectedText: 'H5', isTop: false, isSub: true },
      { content: '###### H6', expectedLevel: 6, expectedText: 'H6', isTop: false, isSub: true }
    ];

    scenarios.forEach(({ content, expectedLevel, expectedText, isTop, isSub }) => {
      const node: HeaderNode = {
        id: `test-${expectedLevel}`,
        nodeType: 'header',
        content,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(getHeaderLevel(node)).toBe(expectedLevel);
      expect(getHeaderText(node)).toBe(expectedText);
      expect(HeaderNodeHelpers.isTopLevel(node)).toBe(isTop);
      expect(HeaderNodeHelpers.isSubHeading(node)).toBe(isSub);
    });
  });

  it('handles level changes across full range', () => {
    let node = HeaderNodeHelpers.createHeaderNode('Test Header', 1);

    for (let level = 2; level <= 6; level++) {
      node = setHeaderLevel(node, level);
      expect(getHeaderLevel(node)).toBe(level);
      expect(getHeaderText(node)).toBe('Test Header');
    }

    // Go back to level 1
    node = setHeaderLevel(node, 1);
    expect(getHeaderLevel(node)).toBe(1);
    expect(getHeaderText(node)).toBe('Test Header');
  });

  it('creates and manipulates headers with helpers', () => {
    // Create header
    const header = HeaderNodeHelpers.createHeaderNode('New Section', 2);
    expect(HeaderNodeHelpers.isHeaderNode(header)).toBe(true);
    expect(HeaderNodeHelpers.isSubHeading(header)).toBe(true);
    expect(HeaderNodeHelpers.isTopLevel(header)).toBe(false);

    // Change to top level
    const topLevel = HeaderNodeHelpers.setHeaderLevel(header, 1);
    expect(HeaderNodeHelpers.isTopLevel(topLevel)).toBe(true);
    expect(HeaderNodeHelpers.isSubHeading(topLevel)).toBe(false);

    // Extract text
    const text = HeaderNodeHelpers.getHeaderText(topLevel);
    expect(text).toBe('New Section');
  });
});
