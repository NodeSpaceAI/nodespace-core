/**
 * Unit tests for markdown utility
 *
 * Tests the htmlToMarkdown function which converts HTML content to markdown syntax.
 * Covers header tags (h1-h6), inline formatting spans (bold, italic, underline),
 * and HTML tag cleanup.
 */
import { describe, it, expect } from 'vitest';
import { htmlToMarkdown } from '$lib/utils/markdown';

describe('markdown utils', () => {
  describe('htmlToMarkdown', () => {
    describe('header conversions', () => {
      it('should convert h1 tag to markdown heading level 1', () => {
        const result = htmlToMarkdown('<h1>Heading 1</h1>');
        expect(result).toBe('# Heading 1');
      });

      it('should convert h2 tag to markdown heading level 2', () => {
        const result = htmlToMarkdown('<h2>Heading 2</h2>');
        expect(result).toBe('## Heading 2');
      });

      it('should convert h3 tag to markdown heading level 3', () => {
        const result = htmlToMarkdown('<h3>Heading 3</h3>');
        expect(result).toBe('### Heading 3');
      });

      it('should convert h4 tag to markdown heading level 4', () => {
        const result = htmlToMarkdown('<h4>Heading 4</h4>');
        expect(result).toBe('#### Heading 4');
      });

      it('should convert h5 tag to markdown heading level 5', () => {
        const result = htmlToMarkdown('<h5>Heading 5</h5>');
        expect(result).toBe('##### Heading 5');
      });

      it('should convert h6 tag to markdown heading level 6', () => {
        const result = htmlToMarkdown('<h6>Heading 6</h6>');
        expect(result).toBe('###### Heading 6');
      });

      it('should handle headers with attributes', () => {
        const result = htmlToMarkdown('<h1 class="title" id="main">Title</h1>');
        expect(result).toBe('# Title');
      });

      it('should handle headers with inline styles', () => {
        const result = htmlToMarkdown('<h2 style="color: blue;">Styled</h2>');
        expect(result).toBe('## Styled');
      });

      it('should convert multiple headers of different levels', () => {
        const result = htmlToMarkdown('<h1>Level 1</h1><h2>Level 2</h2><h3>Level 3</h3>');
        expect(result).toBe('# Level 1## Level 2### Level 3');
      });

      it('should preserve header level order in conversion', () => {
        const input = '<h6>Small</h6><h1>Large</h1>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('###### Small# Large');
      });

      it('should handle empty header tags', () => {
        const result = htmlToMarkdown('<h1></h1>');
        expect(result).toBe('# ');
      });

      it('should handle headers with only whitespace', () => {
        const result = htmlToMarkdown('<h1>   </h1>');
        expect(result).toBe('#    ');
      });
    });

    describe('inline formatting conversions', () => {
      it('should convert markdown-bold span to markdown bold syntax', () => {
        const result = htmlToMarkdown('<span class="markdown-bold">Bold text</span>');
        expect(result).toBe('**Bold text**');
      });

      it('should convert markdown-italic span to markdown italic syntax', () => {
        const result = htmlToMarkdown('<span class="markdown-italic">Italic text</span>');
        expect(result).toBe('*Italic text*');
      });

      it('should convert markdown-underline span to markdown underline syntax', () => {
        const result = htmlToMarkdown('<span class="markdown-underline">Underlined text</span>');
        expect(result).toBe('__Underlined text__');
      });

      it('should handle multiple bold spans', () => {
        const input =
          '<span class="markdown-bold">Bold 1</span> normal <span class="markdown-bold">Bold 2</span>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('**Bold 1** normal **Bold 2**');
      });

      it('should handle multiple italic spans', () => {
        const input =
          '<span class="markdown-italic">Italic 1</span> normal <span class="markdown-italic">Italic 2</span>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('*Italic 1* normal *Italic 2*');
      });

      it('should handle multiple underline spans', () => {
        const input =
          '<span class="markdown-underline">Underline 1</span> normal <span class="markdown-underline">Underline 2</span>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('__Underline 1__ normal __Underline 2__');
      });

      it('should handle mixed inline formatting', () => {
        const input =
          '<span class="markdown-bold">Bold</span> and <span class="markdown-italic">Italic</span> and <span class="markdown-underline">Underline</span>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('**Bold** and *Italic* and __Underline__');
      });

      it('should handle empty formatting spans', () => {
        const result = htmlToMarkdown('<span class="markdown-bold"></span>');
        expect(result).toBe('****');
      });

      it('should handle formatting spans with only whitespace', () => {
        const result = htmlToMarkdown('<span class="markdown-italic">   </span>');
        expect(result).toBe('*   *');
      });
    });

    describe('HTML tag cleanup', () => {
      it('should remove simple HTML tags', () => {
        const result = htmlToMarkdown('<div>Content</div>');
        expect(result).toBe('Content');
      });

      it('should remove paragraph tags', () => {
        const result = htmlToMarkdown('<p>Paragraph text</p>');
        expect(result).toBe('Paragraph text');
      });

      it('should remove nested HTML tags', () => {
        const result = htmlToMarkdown('<div><p><span>Nested</span></p></div>');
        expect(result).toBe('Nested');
      });

      it('should remove tags with attributes', () => {
        const result = htmlToMarkdown('<div class="container" id="main">Text</div>');
        expect(result).toBe('Text');
      });

      it('should remove self-closing tags', () => {
        const result = htmlToMarkdown('Text<br/>More text');
        expect(result).toBe('TextMore text');
      });

      it('should remove multiple different HTML tags', () => {
        const result = htmlToMarkdown('<div><p>Text</p><span>More</span><a href="#">Link</a></div>');
        expect(result).toBe('TextMoreLink');
      });

      it('should handle tags with inline styles', () => {
        const result = htmlToMarkdown('<div style="color: red; font-size: 14px;">Styled</div>');
        expect(result).toBe('Styled');
      });
    });

    describe('complex conversions', () => {
      it('should convert headers with bold formatting', () => {
        const result = htmlToMarkdown(
          '<h1><span class="markdown-bold">Bold Heading</span></h1>'
        );
        expect(result).toBe('# **Bold Heading**');
      });

      it('should convert headers with italic formatting', () => {
        const result = htmlToMarkdown(
          '<h2><span class="markdown-italic">Italic Heading</span></h2>'
        );
        expect(result).toBe('## *Italic Heading*');
      });

      it('should convert headers with mixed formatting', () => {
        const result = htmlToMarkdown(
          '<h1><span class="markdown-bold">Bold</span> and <span class="markdown-italic">Italic</span></h1>'
        );
        expect(result).toBe('# **Bold** and *Italic*');
      });

      it('should handle content with headers and inline formatting', () => {
        const input = '<h1>Title</h1><p><span class="markdown-bold">Bold paragraph</span></p>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# Title**Bold paragraph**');
      });

      it('should handle complex nested structures', () => {
        const input =
          '<div><h2>Section</h2><p>Text with <span class="markdown-bold">bold</span> and <span class="markdown-italic">italic</span></p></div>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('## SectionText with **bold** and *italic*');
      });

      it('should convert multiple sections with headers and formatting', () => {
        const input =
          '<h1>Main Title</h1><p><span class="markdown-bold">Intro</span></p><h2>Subtitle</h2><p><span class="markdown-italic">Details</span></p>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# Main Title**Intro**## Subtitle*Details*');
      });
    });

    describe('edge cases', () => {
      it('should handle empty string', () => {
        const result = htmlToMarkdown('');
        expect(result).toBe('');
      });

      it('should handle plain text without HTML', () => {
        const result = htmlToMarkdown('Plain text');
        expect(result).toBe('Plain text');
      });

      it('should handle whitespace-only content', () => {
        const result = htmlToMarkdown('   ');
        expect(result).toBe('   ');
      });

      it('should handle malformed HTML tags', () => {
        const result = htmlToMarkdown('<div>Unclosed tag');
        expect(result).toBe('Unclosed tag');
      });

      it('should handle tags without closing brackets', () => {
        const result = htmlToMarkdown('<div Text >more</div>');
        // The regex /<[^>]*>/g matches '<div Text >' and '</div>', removing them entirely
        expect(result).toBe('more');
      });

      it('should handle spans with other class names', () => {
        const result = htmlToMarkdown('<span class="other-class">Text</span>');
        expect(result).toBe('Text');
      });

      it('should handle spans without markdown classes', () => {
        const result = htmlToMarkdown('<span>Plain span</span>');
        expect(result).toBe('Plain span');
      });

      it('should handle unicode characters', () => {
        const result = htmlToMarkdown('<h1>日本語のタイトル</h1>');
        expect(result).toBe('# 日本語のタイトル');
      });

      it('should handle emojis', () => {
        const result = htmlToMarkdown(
          '<span class="markdown-bold">📝 Notes 📋</span>'
        );
        expect(result).toBe('**📝 Notes 📋**');
      });

      it('should handle special characters', () => {
        const result = htmlToMarkdown('<h1>Title with & < > " \' characters</h1>');
        // The < and > are part of the HTML tag cleanup regex, so they get removed
        expect(result).toBe('# Title with &  " \' characters');
      });

      it('should handle multiple consecutive spaces', () => {
        const result = htmlToMarkdown('<span class="markdown-bold">Text    with    spaces</span>');
        expect(result).toBe('**Text    with    spaces**');
      });

      it('should handle newlines in content', () => {
        const result = htmlToMarkdown('<div>Line 1\nLine 2</div>');
        expect(result).toBe('Line 1\nLine 2');
      });

      it('should handle tabs in content', () => {
        const result = htmlToMarkdown('<div>Text\twith\ttabs</div>');
        expect(result).toBe('Text\twith\ttabs');
      });
    });

    describe('real-world scenarios', () => {
      it('should convert typical rich text note content', () => {
        const input =
          '<h1>Meeting Notes</h1><p><span class="markdown-bold">Attendees:</span> John, Jane</p><p><span class="markdown-italic">Date:</span> Oct 31, 2025</p>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# Meeting Notes**Attendees:** John, Jane*Date:* Oct 31, 2025');
      });

      it('should convert document with multiple formatting styles', () => {
        const input =
          '<h2>Project Overview</h2><p>The project aims to <span class="markdown-bold">improve</span> user experience by <span class="markdown-italic">streamlining</span> the interface.</p>';
        const result = htmlToMarkdown(input);
        expect(result).toBe(
          '## Project OverviewThe project aims to **improve** user experience by *streamlining* the interface.'
        );
      });

      it('should handle content with all header levels', () => {
        const input =
          '<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# H1## H2### H3#### H4##### H5###### H6');
      });

      it('should convert emphasized list items', () => {
        const input =
          '<ul><li><span class="markdown-bold">Task 1:</span> Complete</li><li><span class="markdown-italic">Task 2:</span> In progress</li></ul>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('**Task 1:** Complete*Task 2:* In progress');
      });

      it('should handle mixed content with cleanup', () => {
        const input =
          '<div class="container"><h1>Title</h1><p>Paragraph with <span class="markdown-bold">bold</span> text.</p></div>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# TitleParagraph with **bold** text.');
      });

      it('should convert header hierarchy in documentation', () => {
        const input =
          '<h1>Documentation</h1><h2>Introduction</h2><p>Overview text</p><h3>Getting Started</h3><p>Setup instructions</p>';
        const result = htmlToMarkdown(input);
        expect(result).toBe(
          '# Documentation## IntroductionOverview text### Getting StartedSetup instructions'
        );
      });
    });

    describe('order of operations', () => {
      it('should process headers before generic tag cleanup', () => {
        // Headers should be converted to markdown before being cleaned as generic tags
        const result = htmlToMarkdown('<h1>Title</h1>');
        expect(result).toBe('# Title');
      });

      it('should process inline formatting before generic tag cleanup', () => {
        // Formatting spans should be converted before being cleaned as generic tags
        const result = htmlToMarkdown('<span class="markdown-bold">Bold</span>');
        expect(result).toBe('**Bold**');
      });

      it('should clean up remaining tags after conversions', () => {
        // After headers and formatting are converted, other tags should be removed
        const input =
          '<div><h1>Title</h1><span class="other">Text</span></div>';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# TitleText');
      });
    });

    describe('preservation of content', () => {
      it('should preserve text content throughout conversion', () => {
        const input =
          '<h1>Title</h1>Text between<span class="markdown-bold">Bold</span>More text';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# TitleText between**Bold**More text');
      });

      it('should preserve spacing between elements', () => {
        const input = '<h1>Title</h1> <span class="markdown-bold">Bold</span> text';
        const result = htmlToMarkdown(input);
        expect(result).toBe('# Title **Bold** text');
      });

      it('should not add or remove content', () => {
        const input = 'a<h1>b</h1>c<span class="markdown-bold">d</span>e';
        const result = htmlToMarkdown(input);
        // Headers get converted to markdown, bold spans get converted to **text**
        expect(result).toBe('a# bc**d**e');
      });
    });
  });
});
