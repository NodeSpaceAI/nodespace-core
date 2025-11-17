/**
 * Tests for CodeBlock Node Type-Safe Wrapper
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
      content: 'const x = 1;',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 'javascript' }
    };

    expect(isCodeBlockNode(codeBlockNode)).toBe(true);
  });

  it('rejects non-code-block nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isCodeBlockNode(textNode)).toBe(false);
  });
});

describe('getLanguage', () => {
  it('returns language from properties', () => {
    const node: CodeBlockNode = {
      id: 'test-3',
      nodeType: 'code-block',
      content: 'print("hello")',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 'python' }
    };

    expect(getLanguage(node)).toBe('python');
  });

  it('returns plaintext when language is missing', () => {
    const node: CodeBlockNode = {
      id: 'test-4',
      nodeType: 'code-block',
      content: 'some code',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(getLanguage(node)).toBe('plaintext');
  });

  it('returns plaintext when language is not a string', () => {
    const node: CodeBlockNode = {
      id: 'test-5',
      nodeType: 'code-block',
      content: 'some code',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 123 } as unknown as CodeBlockNode['properties']
    };

    expect(getLanguage(node)).toBe('plaintext');
  });

  it('returns plaintext when language is empty string', () => {
    const node: CodeBlockNode = {
      id: 'test-6',
      nodeType: 'code-block',
      content: 'some code',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: '' }
    };

    expect(getLanguage(node)).toBe('plaintext');
  });
});

describe('setLanguage', () => {
  it('sets language property immutably', () => {
    const original: CodeBlockNode = {
      id: 'test-7',
      nodeType: 'code-block',
      content: 'fn main() {}',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    const updated = setLanguage(original, 'rust');

    // Original unchanged
    expect(getLanguage(original)).toBe('plaintext');

    // Updated has new language
    expect(getLanguage(updated)).toBe('rust');
    expect(updated.id).toBe(original.id);
    expect(updated.content).toBe(original.content);
  });

  it('overwrites existing language', () => {
    const original: CodeBlockNode = {
      id: 'test-8',
      nodeType: 'code-block',
      content: 'code',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 'javascript' }
    };

    const updated = setLanguage(original, 'typescript');

    expect(getLanguage(original)).toBe('javascript');
    expect(getLanguage(updated)).toBe('typescript');
  });

  it('preserves other properties', () => {
    const original: CodeBlockNode = {
      id: 'test-9',
      nodeType: 'code-block',
      content: 'code',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 'javascript', customProp: 'value' }
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
      content: 'SELECT * FROM users;',
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: { language: 'sql' }
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
      { language: 'rust', content: 'fn main() {}', expected: 'Rust' },
      { language: 'python', content: 'print("hello")', expected: 'Python' },
      { language: 'bash', content: '#!/bin/bash\necho "test"', expected: 'Bash' },
      { language: 'json', content: '{"key": "value"}', expected: 'JSON' }
    ];

    scenarios.forEach(({ language, content, expected }) => {
      const node: CodeBlockNode = {
        id: `test-${language}`,
        nodeType: 'code-block',
        content,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: { language }
      };

      expect(getLanguage(node)).toBe(language);
      expect(CodeBlockNodeHelpers.getLanguageDisplayName(language)).toBe(expected);
    });
  });
});
