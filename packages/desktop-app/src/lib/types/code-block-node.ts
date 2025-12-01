/**
 * Type-Safe CodeBlock Node Wrapper
 *
 * Provides ergonomic, type-safe access to code block node properties
 * while maintaining the universal Node storage model.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { CodeBlockNode, isCodeBlockNode, getLanguage, setLanguage } from '$lib/types/code-block-node';
 *
 * // Type guard
 * if (isCodeBlockNode(node)) {
 *   const language = getLanguage(node); // Type-safe access
 *   console.log(`Code is in ${language}`);
 * }
 *
 * // Immutable update
 * const updated = setLanguage(node, 'typescript');
 * ```
 */

import type { Node } from './node';

/**
 * CodeBlock node interface extending base Node
 *
 * Represents a code block with syntax highlighting support.
 * Note: language is derived from content (code fence syntax), not stored in properties.
 */
export interface CodeBlockNode extends Node {
  nodeType: 'code-block';
}

/**
 * Type guard to check if a node is a code block node
 *
 * @param node - Node to check
 * @returns True if node is a code block node
 *
 * @example
 * ```typescript
 * if (isCodeBlockNode(node)) {
 *   // TypeScript knows node is CodeBlockNode here
 *   const language = getLanguage(node);
 * }
 * ```
 */
export function isCodeBlockNode(node: Node): node is CodeBlockNode {
  return node.nodeType === 'code-block';
}

/**
 * Get the programming language for syntax highlighting
 *
 * Parses the language from the code fence syntax in content (e.g., ```typescript).
 * The language is derived from content, not stored as a property.
 *
 * @param node - Code block node
 * @returns Language identifier (defaults to "plaintext")
 *
 * @example
 * ```typescript
 * const language = getLanguage(codeBlockNode);
 * console.log(language); // "javascript", "python", etc.
 * ```
 */
export function getLanguage(node: CodeBlockNode): string {
  // Parse language from code fence syntax: ```language
  // Supports c++, c#, objective-c, and other languages with special characters
  const match = node.content.match(/^```([\w\-+#]+)?/);
  if (match && match[1]) {
    return match[1].toLowerCase();
  }
  return 'plaintext';
}

/**
 * Set the programming language for syntax highlighting (immutable)
 *
 * Returns a new node with the updated language in the code fence syntax.
 * Original node is not modified.
 *
 * @param node - Code block node
 * @param language - Language identifier (e.g., "rust", "typescript", "python")
 * @returns New node with updated language in content
 *
 * @example
 * ```typescript
 * const updated = setLanguage(codeBlockNode, 'typescript');
 * // original node unchanged, updated has new language in code fence
 * ```
 */
export function setLanguage(node: CodeBlockNode, language: string): CodeBlockNode {
  // Update language in code fence syntax: ```language
  const newContent = node.content.replace(/^```\w*/, '```' + language);
  return {
    ...node,
    content: newContent
  };
}

/**
 * Helper namespace for code block node operations
 *
 * Provides utility functions for working with code block nodes.
 */
export const CodeBlockNodeHelpers = {
  /**
   * Check if node is a code block
   */
  isCodeBlockNode,

  /**
   * Get language with fallback to plaintext
   */
  getLanguage,

  /**
   * Set language (immutable update)
   */
  setLanguage,

  /**
   * Check if language is a common programming language
   *
   * @param language - Language identifier to check
   * @returns True if language is in the common list
   *
   * @example
   * ```typescript
   * CodeBlockNodeHelpers.isCommonLanguage('typescript'); // true
   * CodeBlockNodeHelpers.isCommonLanguage('brainfuck'); // false
   * ```
   */
  isCommonLanguage(language: string): boolean {
    const commonLanguages = new Set([
      'javascript',
      'typescript',
      'python',
      'rust',
      'go',
      'java',
      'c',
      'cpp',
      'c++',
      'csharp',
      'c#',
      'ruby',
      'php',
      'swift',
      'kotlin',
      'html',
      'css',
      'scss',
      'sass',
      'sql',
      'bash',
      'shell',
      'json',
      'yaml',
      'xml',
      'markdown',
      'plaintext'
    ]);
    return commonLanguages.has(language.toLowerCase());
  },

  /**
   * Get language display name (handles special cases)
   *
   * @param language - Language identifier
   * @returns Display-friendly language name
   *
   * @example
   * ```typescript
   * getLanguageDisplayName('cpp'); // "C++"
   * getLanguageDisplayName('csharp'); // "C#"
   * getLanguageDisplayName('javascript'); // "JavaScript"
   * ```
   */
  getLanguageDisplayName(language: string): string {
    const displayNames: Record<string, string> = {
      javascript: 'JavaScript',
      typescript: 'TypeScript',
      cpp: 'C++',
      'c++': 'C++',
      csharp: 'C#',
      'c#': 'C#',
      python: 'Python',
      rust: 'Rust',
      go: 'Go',
      java: 'Java',
      ruby: 'Ruby',
      php: 'PHP',
      swift: 'Swift',
      kotlin: 'Kotlin',
      html: 'HTML',
      css: 'CSS',
      scss: 'SCSS',
      sass: 'Sass',
      sql: 'SQL',
      bash: 'Bash',
      shell: 'Shell',
      json: 'JSON',
      yaml: 'YAML',
      xml: 'XML',
      markdown: 'Markdown',
      plaintext: 'Plain Text'
    };

    return displayNames[language.toLowerCase()] || language;
  }
};
