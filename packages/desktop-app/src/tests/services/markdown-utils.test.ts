/**
 * Unit tests for markdown-utils service
 *
 * - stripMarkdown for plain text extraction
 * - parseHeaderLevel for header detection
 */
import { describe, it, expect } from 'vitest';
import { stripMarkdown, parseHeaderLevel } from '$lib/services/markdown-utils';

describe('markdown-utils service', () => {
	describe('stripMarkdown', () => {
		it('should return empty string for empty input', () => {
			expect(stripMarkdown('')).toBe('');
		});

		it('should return empty string for null/undefined input', () => {
			// @ts-expect-error testing invalid input
			expect(stripMarkdown(null)).toBe('');
		});

		it('should strip heading markers', () => {
			expect(stripMarkdown('# Heading')).toBe('Heading');
			expect(stripMarkdown('## Heading 2')).toBe('Heading 2');
			expect(stripMarkdown('###### Heading 6')).toBe('Heading 6');
		});

		it('should strip bold markers (asterisks)', () => {
			expect(stripMarkdown('**bold**')).toBe('bold');
		});

		it('should keep intra-word underscores', () => {
			expect(stripMarkdown('Review my_config_file.rs')).toBe('Review my_config_file.rs');
			expect(stripMarkdown('Fix user_id and order_id')).toBe('Fix user_id and order_id');
			expect(stripMarkdown('__init__.py')).toBe('__init__.py');
		});

		it('should strip underscore emphasis at word boundaries', () => {
			expect(stripMarkdown('_a_ _b_ _c_')).toBe('a b c');
			expect(stripMarkdown('see __this__, now')).toBe('see this, now');
			expect(stripMarkdown('use my_var in _italic_ text')).toBe('use my_var in italic text');
		});

		it('should strip bold markers (underscores)', () => {
			expect(stripMarkdown('__bold__')).toBe('bold');
		});

		it('should strip italic markers (asterisks)', () => {
			expect(stripMarkdown('*italic*')).toBe('italic');
		});

		it('should strip italic markers (underscores)', () => {
			expect(stripMarkdown('_italic_')).toBe('italic');
		});

		it('should strip inline code markers', () => {
			expect(stripMarkdown('`code`')).toBe('code');
		});

		it('should strip link markers and keep text', () => {
			expect(stripMarkdown('[Link Text](url)')).toBe('Link Text');
		});

		it('should normalize whitespace', () => {
			expect(stripMarkdown('Text   with    multiple     spaces')).toBe('Text with multiple spaces');
		});

		it('should handle complex markdown', () => {
			const input = '# Title\n\nThis is **bold** and *italic* with `code` and [link](url).';
			const result = stripMarkdown(input);
			expect(result).toBe('Title This is bold and italic with code and link.');
		});

		it('should handle multiple formatting on same text', () => {
			const result = stripMarkdown('***bold italic***');
			expect(result).toBe('bold italic');
			expect(result).not.toContain('*');
		});

		it('should trim result', () => {
			const result = stripMarkdown('  # Heading  ');
			expect(result).not.toMatch(/^\s/);
			expect(result).not.toMatch(/\s$/);
		});
	});


	describe('parseHeaderLevel', () => {
		it('should return the number of leading hashes for headers', () => {
			expect(parseHeaderLevel('# Title')).toBe(1);
			expect(parseHeaderLevel('### Title')).toBe(3);
			expect(parseHeaderLevel('###### Title')).toBe(6);
		});

		it('should detect a header while typing, before any text follows the space', () => {
			expect(parseHeaderLevel('## ')).toBe(2);
		});

		it('should return 0 when the hashes are not followed by a space', () => {
			expect(parseHeaderLevel('#')).toBe(0);
			expect(parseHeaderLevel('#hashtag')).toBe(0);
		});

		it('should return 0 for more than six hashes', () => {
			expect(parseHeaderLevel('####### Title')).toBe(0);
		});

		it('should return 0 for non-header content', () => {
			expect(parseHeaderLevel('')).toBe(0);
			expect(parseHeaderLevel('Plain text')).toBe(0);
			expect(parseHeaderLevel(' # Indented')).toBe(0);
		});
	});

	describe('stripMarkdown on large input', () => {
		it('should strip markdown from large text', () => {
			const largeMarkdown = '**Bold** and *italic* text. '.repeat(1000);
			expect(stripMarkdown(largeMarkdown)).toBe('Bold and italic text. '.repeat(1000).trim());
		});

		it('should not throw on very long text', () => {
			expect(() => stripMarkdown('A'.repeat(10000))).not.toThrow();
		});
	});
});
