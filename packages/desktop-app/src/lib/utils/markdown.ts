/**
 * Markdown conversion utilities
 * Shared functions for converting HTML content to markdown format
 */

/**
 * Convert HTML content to markdown syntax
 * Handles header tags and inline formatting spans
 */
export function htmlToMarkdown(htmlContent: string): string {
  let markdown = htmlContent;

  // Convert header tags to markdown syntax (check first to preserve header level)
  markdown = markdown.replace(/<h1[^>]*>(.*?)<\/h1>/g, '# $1');
  markdown = markdown.replace(/<h2[^>]*>(.*?)<\/h2>/g, '## $1');
  markdown = markdown.replace(/<h3[^>]*>(.*?)<\/h3>/g, '### $1');
  markdown = markdown.replace(/<h4[^>]*>(.*?)<\/h4>/g, '#### $1');
  markdown = markdown.replace(/<h5[^>]*>(.*?)<\/h5>/g, '##### $1');
  markdown = markdown.replace(/<h6[^>]*>(.*?)<\/h6>/g, '###### $1');

  // Convert HTML spans to markdown syntax
  markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
  markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');
  markdown = markdown.replace(/<span class="markdown-underline">(.*?)<\/span>/g, '__$1__');

  // Clean up any remaining HTML tags
  markdown = markdown.replace(/<[^>]*>/g, '');

  return markdown;
}
