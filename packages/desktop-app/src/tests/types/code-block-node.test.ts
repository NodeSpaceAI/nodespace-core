/**
 * Tests for CodeBlock Node Type-Safe Wrapper
 *
 * Note: Language is now derived from code fence syntax in content (e.g., ```python)
 * rather than stored as a property. This matches the Rust backend behavior.
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import {
  type CodeBlockNode,
  isCodeBlockNode,
  getLanguage,
  setLanguage,
  CodeBlockNodeHelpers
} from '$lib/types/code-block-node';

describe('CodeBlockNode Type Guard', () => {
  it('identifies code block nodes correctly', () => {
    const codeBlockNode: Node = {
      id: 'test-1',
      nodeType: 'code-block',
      content: '```javascript\nconst x = 1;\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isCodeBlockNode(codeBlockNode)).toBe(true);
  });

  it('rejects non-code-block nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isCodeBlockNode(textNode)).toBe(false);
  });
});

describe('getLanguage', () => {
  it('returns language from code fence syntax in content', () => {
    const node: CodeBlockNode = {
      id: 'test-3',
      nodeType: 'code-block',
      content: '```python\nprint("hello")\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('python');
  });

  it('returns plaintext when language is missing in code fence', () => {
    const node: CodeBlockNode = {
      id: 'test-4',
      nodeType: 'code-block',
      content: '```\nsome code\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('plaintext');
  });

  it('returns plaintext when no code fence exists', () => {
    const node: CodeBlockNode = {
      id: 'test-5',
      nodeType: 'code-block',
      content: 'some code without fences',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('plaintext');
  });

  it('handles empty content', () => {
    const node: CodeBlockNode = {
      id: 'test-6',
      nodeType: 'code-block',
      content: '',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('plaintext');
  });

  it('extracts language case-insensitively and normalizes to lowercase', () => {
    const node: CodeBlockNode = {
      id: 'test-7',
      nodeType: 'code-block',
      content: '```JavaScript\ncode here\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('javascript');
  });
});

describe('setLanguage', () => {
  it('sets language in code fence immutably', () => {
    const original: CodeBlockNode = {
      id: 'test-7',
      nodeType: 'code-block',
      content: '```\nfn main() {}\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setLanguage(original, 'rust');

    // Original unchanged
    expect(getLanguage(original)).toBe('plaintext');
    expect(original.content).toBe('```\nfn main() {}\n```');

    // Updated has new language
    expect(getLanguage(updated)).toBe('rust');
    expect(updated.content).toBe('```rust\nfn main() {}\n```');
    expect(updated.id).toBe(original.id);
  });

  it('overwrites existing language', () => {
    const original: CodeBlockNode = {
      id: 'test-8',
      nodeType: 'code-block',
      content: '```javascript\ncode\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setLanguage(original, 'typescript');

    expect(getLanguage(original)).toBe('javascript');
    expect(getLanguage(updated)).toBe('typescript');
    expect(updated.content).toBe('```typescript\ncode\n```');
  });

  it('preserves other properties', () => {
    const original: CodeBlockNode = {
      id: 'test-9',
      nodeType: 'code-block',
      content: '```javascript\ncode\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { customProp: 'value' }
    };

    const updated = setLanguage(original, 'typescript');

    expect(updated.properties.customProp).toBe('value');
    expect(getLanguage(updated)).toBe('typescript');
  });
});

describe('CodeBlockNodeHelpers', () => {
  describe('isCommonLanguage', () => {
    it('identifies common programming languages', () => {
      const commonLanguages = [
        'javascript',
        'typescript',
        'python',
        'rust',
        'go',
        'java',
        'html',
        'css',
        'sql'
      ];

      commonLanguages.forEach((lang) => {
        expect(CodeBlockNodeHelpers.isCommonLanguage(lang)).toBe(true);
      });
    });

    it('rejects uncommon languages', () => {
      expect(CodeBlockNodeHelpers.isCommonLanguage('brainfuck')).toBe(false);
      expect(CodeBlockNodeHelpers.isCommonLanguage('whitespace')).toBe(false);
      expect(CodeBlockNodeHelpers.isCommonLanguage('xyz123')).toBe(false);
    });

    it('is case insensitive', () => {
      expect(CodeBlockNodeHelpers.isCommonLanguage('JavaScript')).toBe(true);
      expect(CodeBlockNodeHelpers.isCommonLanguage('PYTHON')).toBe(true);
      expect(CodeBlockNodeHelpers.isCommonLanguage('RuSt')).toBe(true);
    });
  });

  describe('getLanguageDisplayName', () => {
    it('returns proper display names for common languages', () => {
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('javascript')).toBe('JavaScript');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('typescript')).toBe('TypeScript');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('python')).toBe('Python');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('rust')).toBe('Rust');
    });

    it('handles special cases', () => {
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('cpp')).toBe('C++');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('c++')).toBe('C++');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('csharp')).toBe('C#');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('c#')).toBe('C#');
    });

    it('returns original string for unknown languages', () => {
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('customlang')).toBe('customlang');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('xyz')).toBe('xyz');
    });

    it('is case insensitive', () => {
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('JAVASCRIPT')).toBe('JavaScript');
      expect(CodeBlockNodeHelpers.getLanguageDisplayName('Python')).toBe('Python');
    });
  });
});

describe('Integration', () => {
  it('works with type guard and helpers', () => {
    const node: Node = {
      id: 'test-10',
      nodeType: 'code-block',
      content: '```sql\nSELECT * FROM users;\n```',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    if (isCodeBlockNode(node)) {
      const language = getLanguage(node);
      expect(language).toBe('sql');

      const displayName = CodeBlockNodeHelpers.getLanguageDisplayName(language);
      expect(displayName).toBe('SQL');

      const isCommon = CodeBlockNodeHelpers.isCommonLanguage(language);
      expect(isCommon).toBe(true);

      const updated = setLanguage(node, 'postgresql');
      expect(getLanguage(updated)).toBe('postgresql');
      expect(getLanguage(node)).toBe('sql'); // Original unchanged
    }
  });

  it('handles various code block scenarios', () => {
    const scenarios = [
      { language: 'rust', content: '```rust\nfn main() {}\n```', expected: 'Rust' },
      { language: 'python', content: '```python\nprint("hello")\n```', expected: 'Python' },
      { language: 'bash', content: '```bash\n#!/bin/bash\necho "test"\n```', expected: 'Bash' },
      { language: 'json', content: '```json\n{"key": "value"}\n```', expected: 'JSON' }
    ];

    scenarios.forEach(({ language, content, expected }) => {
      const node: CodeBlockNode = {
        id: `test-${language}`,
        nodeType: 'code-block',
        content,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      expect(getLanguage(node)).toBe(language);
      expect(CodeBlockNodeHelpers.getLanguageDisplayName(language)).toBe(expected);
    });
  });
});
