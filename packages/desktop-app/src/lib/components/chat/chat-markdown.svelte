<!--
  ChatMarkdown Component

  Renders markdown content in AI chat messages using marked.js.
  Unlike the node MarkdownRenderer, this supports full markdown:
  headings, lists, code blocks, tables, etc.

  nodespace:// URIs are rendered as special node links that can be
  clicked to navigate and styled with type-specific decorations.
-->

<script lang="ts">
  import { marked, Renderer, type Tokens } from 'marked';
  import DOMPurify from 'dompurify';
  import { mount, unmount } from 'svelte';
  import NodeCardInline from './node-card-inline.svelte';

  let { content = '' }: { content: string } = $props();

  // Custom renderer that handles nodespace:// URIs
  const chatRenderer = new Renderer();
  chatRenderer.link = function (token: Tokens.Link): string {
    const href = token.href ?? '';
    const text = this.parser.parseInline(token.tokens);

    // Detect nodespace:// URIs and render as placeholders for rich node cards
    const nsMatch = href.match(/^nodespace:\/\/(.+)$/);
    if (nsMatch) {
      const nodeId = nsMatch[1];
      const safeText = text.replace(/"/g, '&quot;');
      return `<span class="ns-node-card-placeholder" data-node-id="${nodeId}" data-display-text="${safeText}"></span>`;
    }

    return `<a href="${href}" target="_blank" rel="noopener noreferrer">${text}</a>`;
  };

  const rendered = $derived(renderMarkdown(content));

  /** Convert bare nodespace:// URIs into markdown links before parsing */
  function autolinkNodespaceUris(md: string): string {
    // Match nodespace://uuid that isn't already inside a markdown link syntax ](url)
    // Only skip when preceded by "](" — regular parentheses are fine
    return md.replace(
      /(?<!\]\()(nodespace:\/\/[a-f0-9-]+)/gi,
      '[$1]($1)'
    );
  }

  function renderMarkdown(md: string): string {
    if (!md) return '';
    try {
      const raw = marked(autolinkNodespaceUris(md), {
        renderer: chatRenderer,
        breaks: true,
        gfm: true,
      });
      if (typeof raw !== 'string') return md;
      // Allow nodespace:// protocol and data attributes for node card placeholders
      return DOMPurify.sanitize(raw, {
        ADD_ATTR: ['data-node-id', 'data-display-text'],
        ALLOW_UNKNOWN_PROTOCOLS: true,
      });
    } catch {
      return md;
    }
  }

  // nodespace:// link clicks are handled by the global click handler
  // in app-shell.svelte — no local handler needed.

  // Inject DOMPurify-sanitized HTML via DOM, then hydrate node card placeholders
  let containerEl: HTMLDivElement;
  let mountedComponents: ReturnType<typeof mount>[] = [];
  let lastContent = '';

  $effect(() => {
    if (!containerEl || content === lastContent) return;
    lastContent = content;

    // Clean up previously mounted components
    for (const comp of mountedComponents) {
      unmount(comp);
    }
    mountedComponents = [];

    containerEl.innerHTML = rendered;

    // Hydrate node card placeholders with Svelte components
    const placeholders = containerEl.querySelectorAll('.ns-node-card-placeholder');
    for (const el of placeholders) {
      const nodeId = el.getAttribute('data-node-id');
      const displayText = el.getAttribute('data-display-text') || undefined;
      if (nodeId) {
        const comp = mount(NodeCardInline, {
          target: el,
          props: { nodeId, displayText }
        });
        mountedComponents.push(comp);
      }
    }
  });

  // Cleanup on component destroy
  $effect(() => {
    return () => {
      for (const comp of mountedComponents) {
        unmount(comp);
      }
      mountedComponents = [];
    };
  });
</script>

<div class="chat-markdown" bind:this={containerEl}></div>

<style>
  .chat-markdown {
    line-height: 1.6;
    word-break: break-word;
  }

  .chat-markdown :global(p) {
    margin: 0 0 0.5em 0;
  }

  .chat-markdown :global(p:last-child) {
    margin-bottom: 0;
  }

  .chat-markdown :global(h1),
  .chat-markdown :global(h2),
  .chat-markdown :global(h3),
  .chat-markdown :global(h4) {
    margin: 0.75em 0 0.25em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  .chat-markdown :global(h1) { font-size: 1.25em; }
  .chat-markdown :global(h2) { font-size: 1.125em; }
  .chat-markdown :global(h3) { font-size: 1em; }

  .chat-markdown :global(strong) {
    font-weight: 600;
  }

  .chat-markdown :global(em) {
    font-style: italic;
  }

  .chat-markdown :global(code) {
    background: hsl(var(--background) / 0.5);
    padding: 0.125em 0.375em;
    border-radius: 0.25rem;
    font-size: 0.85em;
    font-family: 'SF Mono', 'Fira Code', monospace;
  }

  .chat-markdown :global(pre) {
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 0.75rem;
    margin: 0.5em 0;
    overflow-x: auto;
  }

  .chat-markdown :global(pre code) {
    background: none;
    padding: 0;
    font-size: 0.8em;
  }

  .chat-markdown :global(ul),
  .chat-markdown :global(ol) {
    margin: 0.25em 0;
    padding-left: 1.5em;
  }

  .chat-markdown :global(li) {
    margin: 0.125em 0;
  }

  .chat-markdown :global(blockquote) {
    border-left: 3px solid hsl(var(--border));
    margin: 0.5em 0;
    padding: 0.25em 0.75em;
    color: hsl(var(--muted-foreground));
  }

  .chat-markdown :global(a) {
    color: hsl(var(--primary));
    text-decoration: underline;
  }

  .chat-markdown :global(.ns-node-card-placeholder) {
    display: inline;
  }

  .chat-markdown :global(hr) {
    border: none;
    border-top: 1px solid hsl(var(--border));
    margin: 0.75em 0;
  }

  .chat-markdown :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.5em 0;
    font-size: 0.85em;
  }

  .chat-markdown :global(th),
  .chat-markdown :global(td) {
    border: 1px solid hsl(var(--border));
    padding: 0.375em 0.625em;
    text-align: left;
  }

  .chat-markdown :global(th) {
    background: hsl(var(--muted));
    font-weight: 600;
  }
</style>
