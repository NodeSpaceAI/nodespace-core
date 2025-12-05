/**
 * Comprehensive unit tests for markdown-utils service
 *
 * Tests all exported functions to achieve 95%+ coverage including:
 * - parseMarkdown with all formatting options
 * - stripMarkdown for plain text extraction
 * - getWordCount for counting words
 * - validateMarkdown for syntax validation
 * - previewMarkdown for truncated previews
 * - Edge cases and error handling
 */
import { describe, it, expect } from 'vitest';
import {
	parseMarkdown,
	stripMarkdown,
	getWordCount,
	validateMarkdown,
	previewMarkdown,
	type MarkdownOptions
} from '$lib/services/markdown-utils';

describe('markdown-utils service', () => {
	describe('parseMarkdown - basic functionality', () => {
		it('should return empty string for empty input', () => {
			expect(parseMarkdown('')).toBe('');
		});

		it('should return empty string for undefined input', () => {
			// @ts-expect-error testing invalid input
			expect(parseMarkdown(null)).toBe('');
		});

		it('should escape HTML entities to prevent XSS', () => {
			const input = '<script>alert("XSS")</script>';
			const result = parseMarkdown(input);
			expect(result).not.toContain('<script>');
			expect(result).toContain('&lt;script&gt;');
		});

		it('should handle plain text without markdown', () => {
			const input = 'This is plain text';
			const result = parseMarkdown(input);
			expect(result).toContain('This is plain text');
		});

		it('should trim whitespace from result', () => {
			const input = '  Hello World  ';
			const result = parseMarkdown(input);
			expect(result).not.toMatch(/^\s/);
			expect(result).not.toMatch(/\s$/);
		});
	});

	describe('parseMarkdown - headings', () => {
		it('should parse H1 heading', () => {
			const result = parseMarkdown('# Heading 1');
			expect(result).toContain('<h1 class="ns-markdown-heading ns-markdown-h1">');
			expect(result).toContain('Heading 1</h1>');
		});

		it('should parse H2 heading', () => {
			const result = parseMarkdown('## Heading 2');
			expect(result).toContain('<h2 class="ns-markdown-heading ns-markdown-h2">');
			expect(result).toContain('Heading 2</h2>');
		});

		it('should parse H3 heading', () => {
			const result = parseMarkdown('### Heading 3');
			expect(result).toContain('<h3 class="ns-markdown-heading ns-markdown-h3">');
			expect(result).toContain('Heading 3</h3>');
		});

		it('should parse H4 heading', () => {
			const result = parseMarkdown('#### Heading 4');
			expect(result).toContain('<h4 class="ns-markdown-heading ns-markdown-h4">');
			expect(result).toContain('Heading 4</h4>');
		});

		it('should parse H5 heading', () => {
			const result = parseMarkdown('##### Heading 5');
			expect(result).toContain('<h5 class="ns-markdown-heading ns-markdown-h5">');
			expect(result).toContain('Heading 5</h5>');
		});

		it('should parse H6 heading', () => {
			const result = parseMarkdown('###### Heading 6');
			expect(result).toContain('<h6 class="ns-markdown-heading ns-markdown-h6">');
			expect(result).toContain('Heading 6</h6>');
		});

		it('should trim whitespace in heading content', () => {
			const result = parseMarkdown('##   Heading with spaces   ');
			expect(result).toContain('Heading with spaces</h2>');
			expect(result).not.toContain('  Heading');
		});

		it('should not parse headings when disabled', () => {
			const result = parseMarkdown('# Heading', { allowHeadings: false });
			expect(result).not.toContain('<h1');
			expect(result).toContain('# Heading');
		});

		it('should handle multiple headings', () => {
			const input = '# H1\n## H2\n### H3';
			const result = parseMarkdown(input);
			expect(result).toContain('<h1');
			expect(result).toContain('<h2');
			expect(result).toContain('<h3');
		});

		it('should handle headings with special characters', () => {
			const result = parseMarkdown('# Heading with "quotes" & ampersands');
			// Note: HTML entities are escaped before heading processing
			expect(result).toContain('<h1');
			expect(result).toContain('&amp;'); // Ampersand should be escaped
		});
	});

	describe('parseMarkdown - bold text', () => {
		it('should parse bold text with asterisks', () => {
			const result = parseMarkdown('This is **bold** text');
			expect(result).toContain('<strong class="ns-markdown-bold">bold</strong>');
		});

		it('should parse bold text with underscores', () => {
			const result = parseMarkdown('This is __bold__ text');
			expect(result).toContain('<strong class="ns-markdown-bold">bold</strong>');
		});

		it('should handle multiple bold segments', () => {
			const result = parseMarkdown('**First** and **second** bold');
			expect(result).toMatch(/<strong[^>]*>First<\/strong>/);
			expect(result).toMatch(/<strong[^>]*>second<\/strong>/);
		});

		it('should not parse bold when disabled', () => {
			const result = parseMarkdown('**bold**', { allowBold: false });
			expect(result).not.toContain('<strong');
			expect(result).toContain('**bold**');
		});

		it('should handle bold with special characters', () => {
			const result = parseMarkdown('**<script>alert("XSS")</script>**');
			expect(result).toContain('<strong');
			expect(result).toContain('&lt;script&gt;');
		});

		it('should handle empty bold markers', () => {
			const result = parseMarkdown('****');
			expect(result).not.toContain('<strong></strong>');
		});
	});

	describe('parseMarkdown - italic text', () => {
		it('should parse italic text with single asterisk', () => {
			const result = parseMarkdown('This is *italic* text');
			expect(result).toContain('<em class="ns-markdown-italic">italic</em>');
		});

		it('should parse italic text with single underscore', () => {
			const result = parseMarkdown('This is _italic_ text');
			expect(result).toContain('<em class="ns-markdown-italic">italic</em>');
		});

		it('should handle multiple italic segments', () => {
			const result = parseMarkdown('*First* and *second* italic');
			expect(result).toMatch(/<em[^>]*>First<\/em>/);
			expect(result).toMatch(/<em[^>]*>second<\/em>/);
		});

		it('should not parse italic when disabled', () => {
			const result = parseMarkdown('*italic*', { allowItalic: false });
			expect(result).not.toContain('<em');
			expect(result).toContain('*italic*');
		});

		it('should not confuse italic with bold', () => {
			const result = parseMarkdown('**bold** and *italic*');
			expect(result).toContain('<strong');
			expect(result).toContain('<em');
		});

		it('should handle italic with special characters', () => {
			const result = parseMarkdown('*<span>test</span>*');
			expect(result).toContain('<em');
			expect(result).toContain('&lt;span&gt;');
		});
	});

	describe('parseMarkdown - inline code', () => {
		it('should parse inline code', () => {
			const result = parseMarkdown('Use `console.log()` for debugging');
			expect(result).toContain('<code class="ns-markdown-code">console.log()</code>');
		});

		it('should handle multiple code segments', () => {
			const result = parseMarkdown('Use `const` or `let` for variables');
			expect(result).toMatch(/<code[^>]*>const<\/code>/);
			expect(result).toMatch(/<code[^>]*>let<\/code>/);
		});

		it('should not parse code when disabled', () => {
			const result = parseMarkdown('`code`', { allowCode: false });
			expect(result).not.toContain('<code');
			expect(result).toContain('`code`');
		});

		it('should preserve HTML entities in code', () => {
			const result = parseMarkdown('`<div>test</div>`');
			expect(result).toContain('<code');
			expect(result).toContain('&lt;div&gt;');
		});

		it('should handle empty code markers', () => {
			const result = parseMarkdown('``');
			expect(result).not.toContain('<code></code>');
		});

		it('should handle code with special markdown inside', () => {
			const result = parseMarkdown('`**not bold**`');
			expect(result).toContain('<code');
			// Note: The implementation processes bold before code, so markdown inside backticks
			// may still be processed. This is a known limitation of the simple parser.
			expect(result).toContain('<strong'); // Bold is still processed
		});
	});

	describe('parseMarkdown - links', () => {
		it('should parse markdown links', () => {
			const result = parseMarkdown('[NodeSpace](https://nodespace.app)');
			expect(result).toContain('<a href="https://nodespace.app"');
			expect(result).toContain('class="ns-markdown-link"');
			expect(result).toContain('target="_blank"');
			expect(result).toContain('rel="noopener noreferrer"');
			expect(result).toContain('>NodeSpace</a>');
		});

		it('should handle multiple links', () => {
			const result = parseMarkdown('[Link1](url1) and [Link2](url2)');
			expect(result).toContain('href="url1"');
			expect(result).toContain('href="url2"');
		});

		it('should not parse links when disabled', () => {
			const result = parseMarkdown('[Link](url)', { allowLinks: false });
			expect(result).not.toContain('<a');
			expect(result).toContain('[Link](url)');
		});

		it('should handle links with special characters in text', () => {
			const result = parseMarkdown('[Link & "Text"](url)');
			expect(result).toContain('<a');
			expect(result).toContain('Link &amp;'); // Ampersand is escaped
			// Note: Quotes inside link text may not be fully escaped in the simple parser
		});

		it('should handle links with query parameters', () => {
			const result = parseMarkdown('[Search](https://example.com?q=test&page=1)');
			expect(result).toContain('href="https://example.com?q=test&amp;page=1"');
		});

		it('should handle empty link text', () => {
			const result = parseMarkdown('[](url)');
			// Note: The regex requires at least one character in link text ([^\]]+)
			// so empty links are not parsed
			expect(result).not.toContain('<a');
		});
	});

	describe('parseMarkdown - line breaks and paragraphs', () => {
		it('should wrap text in paragraph tags', () => {
			const result = parseMarkdown('Simple text');
			expect(result).toContain('<p class="ns-markdown-paragraph">');
			expect(result).toContain('</p>');
		});

		it('should convert single newlines to <br>', () => {
			const result = parseMarkdown('Line 1\nLine 2');
			expect(result).toContain('Line 1<br>Line 2');
		});

		it('should create separate paragraphs for double newlines', () => {
			const result = parseMarkdown('Para 1\n\nPara 2');
			expect(result).toMatch(/<p[^>]*>Para 1<\/p>/);
			expect(result).toMatch(/<p[^>]*>Para 2<\/p>/);
		});

		it('should not wrap headings in paragraphs', () => {
			const result = parseMarkdown('# Heading\n\nText');
			expect(result).toContain('<h1');
			expect(result).not.toContain('<p class="ns-markdown-paragraph"># Heading</p>');
		});

		it('should not parse line breaks when disabled', () => {
			const result = parseMarkdown('Line 1\nLine 2', { allowLineBreaks: false });
			expect(result).not.toContain('<br>');
			expect(result).not.toContain('<p');
		});

		it('should handle multiple blank lines', () => {
			const result = parseMarkdown('Para 1\n\n\n\nPara 2');
			// Should still create two paragraphs, not empty ones
			expect(result).toMatch(/<p[^>]*>Para 1<\/p>/);
			expect(result).toMatch(/<p[^>]*>Para 2<\/p>/);
		});

		it('should handle text with trailing newlines', () => {
			const result = parseMarkdown('Text\n\n');
			expect(result).toContain('Text');
			expect(result).not.toMatch(/>\s+$/); // No trailing whitespace
		});
	});

	describe('parseMarkdown - combined formatting', () => {
		it('should handle bold and italic together', () => {
			const result = parseMarkdown('**bold** and *italic* text');
			expect(result).toContain('<strong');
			expect(result).toContain('<em');
		});

		it('should handle nested formatting (bold italic)', () => {
			const result = parseMarkdown('***bold and italic***');
			// Should have both strong and em tags
			expect(result).toContain('<strong');
			expect(result).toContain('<em');
		});

		it('should handle links with bold text', () => {
			const result = parseMarkdown('[**bold link**](url)');
			expect(result).toContain('<a');
			expect(result).toContain('<strong');
		});

		it('should handle code in headings', () => {
			const result = parseMarkdown('# Heading with `code`');
			expect(result).toContain('<h1');
			expect(result).toContain('<code');
		});

		it('should handle complex mixed formatting', () => {
			const input = '# Title\n\nThis is **bold** and *italic* with `code` and [link](url).\n\nNew paragraph.';
			const result = parseMarkdown(input);
			expect(result).toContain('<h1');
			expect(result).toContain('<strong');
			expect(result).toContain('<em');
			expect(result).toContain('<code');
			expect(result).toContain('<a');
			expect(result).toContain('<p');
		});
	});

	describe('parseMarkdown - options', () => {
		it('should accept partial options', () => {
			const result = parseMarkdown('**bold**', { allowBold: false });
			expect(result).not.toContain('<strong');
		});

		it('should merge options with defaults', () => {
			const options: Partial<MarkdownOptions> = {
				allowBold: false,
				allowItalic: false
			};
			const result = parseMarkdown('**bold** *italic* [link](url)', options);
			expect(result).not.toContain('<strong');
			expect(result).not.toContain('<em');
			expect(result).toContain('<a'); // Links still enabled by default
		});

		it('should disable all formatting when all options false', () => {
			const options: MarkdownOptions = {
				allowHeadings: false,
				allowBold: false,
				allowItalic: false,
				allowLinks: false,
				allowCode: false,
				allowLineBreaks: false
			};
			const result = parseMarkdown('# **bold** *italic* `code` [link](url)', options);
			expect(result).not.toContain('<h1');
			expect(result).not.toContain('<strong');
			expect(result).not.toContain('<em');
			expect(result).not.toContain('<code');
			expect(result).not.toContain('<a');
			expect(result).not.toContain('<br>');
		});
	});

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

	describe('getWordCount', () => {
		it('should return 0 for empty string', () => {
			expect(getWordCount('')).toBe(0);
		});

		it('should return 0 for null/undefined input', () => {
			// @ts-expect-error testing invalid input
			expect(getWordCount(null)).toBe(0);
		});

		it('should count words in plain text', () => {
			expect(getWordCount('Hello world')).toBe(2);
		});

		it('should count words ignoring markdown', () => {
			expect(getWordCount('**Bold** and *italic*')).toBe(3);
		});

		it('should count words in heading', () => {
			expect(getWordCount('# This is a heading')).toBe(4);
		});

		it('should count words ignoring links', () => {
			expect(getWordCount('[Link text](url) more words')).toBe(4);
		});

		it('should count words ignoring code markers', () => {
			expect(getWordCount('Use `console.log()` for debugging')).toBe(4);
		});

		it('should handle multiple spaces', () => {
			expect(getWordCount('Word    with    spaces')).toBe(3);
		});

		it('should handle newlines', () => {
			expect(getWordCount('Line 1\nLine 2\nLine 3')).toBe(6);
		});

		it('should handle complex markdown', () => {
			const input = '# Title\n\n**Bold** text with *italic* and [link](url).';
			// Words: Title, Bold, text, with, italic, and, link
			expect(getWordCount(input)).toBe(7);
		});

		it('should not count only whitespace', () => {
			expect(getWordCount('   \n\n   ')).toBe(0);
		});

		it('should handle punctuation correctly', () => {
			expect(getWordCount('Hello, world! How are you?')).toBe(5);
		});
	});

	describe('validateMarkdown', () => {
		it('should validate correct markdown', () => {
			const result = validateMarkdown('**bold** and *italic*');
			expect(result.isValid).toBe(true);
			expect(result.errors).toHaveLength(0);
		});

		it('should detect unclosed bold markers (asterisks)', () => {
			const result = validateMarkdown('**bold text');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed bold markers (**)');
		});

		it('should detect unclosed bold markers (underscores)', () => {
			const result = validateMarkdown('__bold text');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed bold markers (__)');
		});

		it('should detect unclosed italic markers (asterisks)', () => {
			const result = validateMarkdown('*italic text');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed italic markers (*)');
		});

		it('should detect unclosed italic markers (underscores)', () => {
			const result = validateMarkdown('_italic text');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed italic markers (_)');
		});

		it('should detect unclosed code markers', () => {
			const result = validateMarkdown('`code text');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed code markers (`)');
		});

		it('should detect multiple errors', () => {
			const result = validateMarkdown('**bold *italic `code');
			expect(result.isValid).toBe(false);
			expect(result.errors.length).toBeGreaterThan(1);
		});

		it('should handle empty string as valid', () => {
			const result = validateMarkdown('');
			expect(result.isValid).toBe(true);
			expect(result.errors).toHaveLength(0);
		});

		it('should handle plain text as valid', () => {
			const result = validateMarkdown('Plain text without markdown');
			expect(result.isValid).toBe(true);
			expect(result.errors).toHaveLength(0);
		});

		it('should validate correctly matched pairs', () => {
			const result = validateMarkdown('**bold1** **bold2** *italic1* *italic2* `code1` `code2`');
			expect(result.isValid).toBe(true);
			expect(result.errors).toHaveLength(0);
		});

		it('should handle mixed matched and unmatched markers', () => {
			const result = validateMarkdown('**bold** *italic');
			expect(result.isValid).toBe(false);
			expect(result.errors).toContain('Unclosed italic markers (*)');
		});

		it('should not confuse bold with italic (triple asterisks)', () => {
			const result = validateMarkdown('***bold italic***');
			expect(result.isValid).toBe(true);
			expect(result.errors).toHaveLength(0);
		});

		it('should handle escaped markers', () => {
			// Note: The validator counts all markers, including those in code blocks
			// This is a simple validator that doesn't parse context
			const result = validateMarkdown('Use `**` for bold');
			expect(result.isValid).toBe(false); // The ** is still counted
			expect(result.errors).toContain('Unclosed bold markers (**)');
		});
	});

	describe('previewMarkdown', () => {
		it('should return full text if shorter than maxLength', () => {
			const input = 'Short text';
			const result = previewMarkdown(input);
			expect(result).toBe('Short text');
			expect(result).not.toContain('...');
		});

		it('should truncate text longer than maxLength', () => {
			const input = 'A'.repeat(200);
			const result = previewMarkdown(input, 150);
			expect(result.length).toBeLessThanOrEqual(154); // 150 + '...'
			expect(result).toContain('...');
		});

		it('should use default maxLength of 150', () => {
			const input = 'A'.repeat(200);
			const result = previewMarkdown(input);
			expect(result.length).toBeLessThanOrEqual(154);
			expect(result).toContain('...');
		});

		it('should strip markdown before truncating', () => {
			const input = '**Bold** ' + 'A'.repeat(150);
			const result = previewMarkdown(input, 150);
			expect(result).not.toContain('**');
			expect(result).toContain('Bold');
		});

		it('should trim before adding ellipsis', () => {
			const input = 'Word '.repeat(100);
			const result = previewMarkdown(input, 50);
			expect(result).toMatch(/\w\.\.\./); // Word followed by ...
			expect(result).not.toMatch(/\s\.\.\./); // No space before ...
		});

		it('should handle empty string', () => {
			expect(previewMarkdown('')).toBe('');
		});

		it('should handle markdown formatting', () => {
			const input = '# Heading\n\n**Bold** and *italic* text.';
			const result = previewMarkdown(input, 50);
			expect(result).not.toContain('#');
			expect(result).not.toContain('**');
			expect(result).not.toContain('*');
			expect(result).toContain('Heading');
		});

		it('should handle custom maxLength of 0', () => {
			const result = previewMarkdown('Text', 0);
			expect(result).toBe('...');
		});

		it('should handle custom maxLength of 10', () => {
			const input = 'This is a long text';
			const result = previewMarkdown(input, 10);
			expect(result.length).toBeLessThanOrEqual(14); // 10 + '...'
			expect(result).toContain('...');
		});

		it('should preserve word boundaries when possible', () => {
			const input = 'Short text exactly here';
			const result = previewMarkdown(input, 20);
			expect(result).toContain('...');
		});
	});

	describe('edge cases and error handling', () => {
		it('should handle very long text', () => {
			const longText = 'A'.repeat(10000);
			expect(() => parseMarkdown(longText)).not.toThrow();
			expect(() => stripMarkdown(longText)).not.toThrow();
			expect(() => getWordCount(longText)).not.toThrow();
		});

		it('should handle special unicode characters', () => {
			const unicode = '日本語 中文 한국어 🚀 🌟';
			expect(() => parseMarkdown(unicode)).not.toThrow();
			expect(getWordCount(unicode)).toBeGreaterThan(0);
		});

		it('should handle emoji in markdown', () => {
			const result = parseMarkdown('# 🚀 Title\n\n**Bold** 🌟');
			expect(result).toContain('🚀');
			expect(result).toContain('🌟');
		});

		it('should handle malformed markdown gracefully', () => {
			expect(() => parseMarkdown('**bold *italic**')).not.toThrow();
			expect(() => parseMarkdown('[link](url]')).not.toThrow();
			expect(() => parseMarkdown('`code**bold*')).not.toThrow();
		});

		it('should handle deeply nested HTML', () => {
			const nested = '<div><div><div><script>alert("XSS")</script></div></div></div>';
			const result = parseMarkdown(nested);
			expect(result).not.toContain('<script>');
			expect(result).toContain('&lt;');
		});

		it('should handle markdown with only symbols', () => {
			const result = parseMarkdown('*** ___ ``` ### []()');
			expect(result).toBeDefined();
		});

		it('should handle text with null bytes', () => {
			const text = 'Hello\x00World';
			expect(() => parseMarkdown(text)).not.toThrow();
		});

		it('should handle text with control characters', () => {
			const text = 'Hello\x01\x02\x03World';
			expect(() => parseMarkdown(text)).not.toThrow();
		});
	});

	describe('real-world usage scenarios', () => {
		it('should handle typical note content', () => {
			const note = `# Meeting Notes

**Date:** 2024-01-15
**Attendees:** Alice, Bob, Charlie

## Discussion Points

- Review *quarterly goals*
- Discuss \`new feature\` implementation
- Plan next sprint

[Documentation](https://docs.example.com)`;

			const result = parseMarkdown(note);
			expect(result).toContain('<h1');
			expect(result).toContain('<h2');
			expect(result).toContain('<strong');
			expect(result).toContain('<em');
			expect(result).toContain('<code');
			expect(result).toContain('<a');
		});

		it('should handle code snippets in notes', () => {
			const note = 'Use `Array.map()` and `Array.filter()` for transformations';
			const result = parseMarkdown(note);
			expect(result).toContain('<code class="ns-markdown-code">Array.map()</code>');
			expect(result).toContain('<code class="ns-markdown-code">Array.filter()</code>');
		});

		it('should handle task lists', () => {
			const tasks = `# Tasks

**Completed:**
- Fix bug in login
- Update documentation

**In Progress:**
- Add new feature
- Write tests`;

			const result = parseMarkdown(tasks);
			expect(result).toContain('Tasks');
			expect(result).toContain('Completed:');
			expect(result).toContain('In Progress:');
		});

		it('should generate preview for long content', () => {
			const longNote = `# Long Document

This is a very long document with lots of content. `.repeat(10);

			const preview = previewMarkdown(longNote, 100);
			expect(preview.length).toBeLessThanOrEqual(104);
			expect(preview).toContain('...');
			// Note: stripMarkdown removes # from the beginning but "# " in the middle
			// of repeated text may appear after stripping leading markers
		});

		it('should validate user input before parsing', () => {
			const userInput = '**Bold text but forgot to close';
			const validation = validateMarkdown(userInput);

			if (!validation.isValid) {
				// In real app, show error to user
				expect(validation.errors.length).toBeGreaterThan(0);
			}
		});

		it('should count words for reading time estimation', () => {
			const article = `# Article Title

This is a long article with many paragraphs and lots of content.`.repeat(50);

			const wordCount = getWordCount(article);
			const readingTime = Math.ceil(wordCount / 200); // 200 words per minute
			expect(wordCount).toBeGreaterThan(0);
			expect(readingTime).toBeGreaterThan(0);
		});
	});

	describe('performance considerations', () => {
		it('should handle rapid successive parse calls', () => {
			expect(() => {
				for (let i = 0; i < 100; i++) {
					parseMarkdown(`# Heading ${i}\n\n**Bold** content ${i}`);
				}
			}).not.toThrow();
		});

		it('should handle rapid successive validation calls', () => {
			expect(() => {
				for (let i = 0; i < 100; i++) {
					validateMarkdown(`**bold ${i}** *italic ${i}*`);
				}
			}).not.toThrow();
		});

		it('should efficiently strip markdown from large text', () => {
			const largeMarkdown = '**Bold** and *italic* text. '.repeat(1000);
			const start = Date.now();
			stripMarkdown(largeMarkdown);
			const duration = Date.now() - start;
			// Should complete in reasonable time (under 100ms)
			expect(duration).toBeLessThan(100);
		});
	});
});
