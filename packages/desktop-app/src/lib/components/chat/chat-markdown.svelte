<!--
  ChatMarkdown Component

  Renders markdown content in AI chat messages using marked.js.
  Unlike the node MarkdownRenderer, this supports full markdown:
  headings, lists, code blocks, tables, etc.

  nodespace:// URIs are rendered as node links that can be clicked to
  navigate and that show the node's live title.
-->

<script lang="ts">
  import { Marked, Renderer, type Tokens } from 'marked';
  import createDOMPurify from 'dompurify';
  import { mount, unmount } from 'svelte';
  import NodeCardInline from './node-card-inline.svelte';

  let { content = '' }: { content: string } = $props();

  // A private Marked instance, NOT the shared `marked` singleton — the
  // singleton is mutated process-wide by marked-config.ts's marked.use()
  // (for the node editor's inline-only rendering), which would otherwise
  // leak into and corrupt this component's full-markdown chat rendering.
  const chatMarked = new Marked();

  /**
   * DOMPurify's default export is a singleton bound to whatever `window`
   * existed the first time the module was imported anywhere in the process
   * (see dompurify's createDOMPurify()). Binding explicitly to the live
   * `window` on every call avoids depending on that import-order-sensitive
   * global state.
   */
  function getDOMPurify() {
    return createDOMPurify(document.defaultView ?? undefined);
  }

  /** Escape text for use inside a double-quoted HTML attribute value. */
  function escapeAttribute(value: string): string {
    return value
      .replace(/&/g, '&amp;')
      .replace(/"/g, '&quot;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
  }

  // Custom renderer that handles nodespace:// URIs
  const chatRenderer = new Renderer();
  chatRenderer.link = function (token: Tokens.Link): string {
    const href = token.href ?? '';

    // Detect nodespace:// URIs and render as placeholders for node links
    const nsMatch = href.match(/^nodespace:\/\/(.+)$/);
    if (nsMatch) {
      // The label is shown as plain text, so it is rendered without inline
      // markup: `[**Data** Layer](…)` reads "Data Layer", not its HTML.
      const label = this.parser.parseInline(token.tokens, this.parser.textRenderer);
      return `<span class="ns-node-card-placeholder" data-node-id="${escapeAttribute(nsMatch[1])}" data-display-text="${escapeAttribute(label)}"></span>`;
    }

    const text = this.parser.parseInline(token.tokens);
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
      const raw = chatMarked.parse(autolinkNodespaceUris(md), {
        renderer: chatRenderer,
        breaks: true,
        gfm: true,
      });
      if (typeof raw !== 'string') return md;
      // Allow nodespace:// protocol and data attributes for node card placeholders
      return getDOMPurify().sanitize(raw, {
        ADD_ATTR: ['data-node-id', 'data-display-text'],
        ALLOW_UNKNOWN_PROTOCOLS: true,
      });
    } catch {
      return md;
    }
  }

  // nodespace:// link clicks are handled by the global click handler
  // in app-shell.svelte — no local handler needed.

  let containerEl: HTMLDivElement;
  const mountedByEl = new Map<Element, ReturnType<typeof mount>>();
  // Pre-hydration snapshot of each top-level node's outerHTML/textContent, so
  // diffing compares against what was actually rendered rather than the live
  // DOM — which NodeCardInline mutates in place once mounted (it injects
  // child elements into the placeholder span), making the live node never
  // equal a freshly-parsed placeholder even when nothing changed.
  let lastRenderedNodes: Node[] = [];

  function isPlaceholder(el: Element): boolean {
    return el.classList.contains('ns-node-card-placeholder');
  }

  function hydratePlaceholders(root: Element): void {
    const placeholders: Element[] = isPlaceholder(root) ? [root] : [];
    placeholders.push(...root.querySelectorAll('.ns-node-card-placeholder'));
    for (const el of placeholders) {
      if (mountedByEl.has(el)) continue;
      const nodeId = el.getAttribute('data-node-id');
      const displayText = el.getAttribute('data-display-text') || undefined;
      if (nodeId) {
        const comp = mount(NodeCardInline, { target: el, props: { nodeId, displayText } });
        mountedByEl.set(el, comp);
      }
    }
  }

  function unmountPlaceholders(root: Element): void {
    const placeholders: Element[] = isPlaceholder(root) ? [root] : [];
    placeholders.push(...root.querySelectorAll('.ns-node-card-placeholder'));
    for (const el of placeholders) {
      const comp = mountedByEl.get(el);
      if (comp) {
        unmount(comp);
        mountedByEl.delete(el);
      }
    }
  }

  /**
   * Patch containerEl's top-level children to match newHtml, replacing only
   * the top-level nodes that actually changed. Streaming updates typically
   * only touch the trailing block, so unchanged siblings — and any
   * NodeCardInline components mounted inside them — are left untouched
   * instead of being torn down and remounted on every content change.
   *
   * This is a positional index diff, not a keyed/LCS diff: block N is only
   * ever compared against the previous render's block N. That's correct for
   * the append-only way chat content grows (streaming appends to the last
   * block or adds new trailing blocks) but would cause needless remounts of
   * every following block if a block were ever reordered or inserted before
   * the end — there's no such path today. Node-cards are also reconciled at
   * block granularity, not individually: if two node-cards share one
   * top-level block (e.g. the same paragraph) and any part of that block's
   * text changes, both remount together.
   */
  function patchContent(newHtml: string): void {
    const template = document.createElement('template');
    template.innerHTML = newHtml;
    const newNodes = Array.from(template.content.childNodes);
    const oldNodes = lastRenderedNodes;
    const liveNodes = Array.from(containerEl.childNodes);

    const max = Math.max(newNodes.length, oldNodes.length);
    for (let i = 0; i < max; i++) {
      const oldNode = oldNodes[i];
      const newNode = newNodes[i];
      const liveNode = liveNodes[i];

      if (oldNode && newNode && oldNode.isEqualNode(newNode)) {
        continue;
      }

      if (liveNode instanceof Element) unmountPlaceholders(liveNode);

      if (liveNode && newNode) {
        containerEl.replaceChild(newNode, liveNode);
      } else if (liveNode) {
        containerEl.removeChild(liveNode);
      } else if (newNode) {
        containerEl.appendChild(newNode);
      }
    }

    // Snapshot pre-hydration nodes for the next diff, then hydrate the live tree.
    lastRenderedNodes = Array.from(containerEl.childNodes).map((n) => n.cloneNode(true));
    for (const node of Array.from(containerEl.childNodes)) {
      if (node instanceof Element) hydratePlaceholders(node);
    }
  }

  $effect(() => {
    if (!containerEl) return;
    patchContent(rendered);
  });

  $effect(() => {
    return () => {
      for (const comp of mountedByEl.values()) {
        unmount(comp);
      }
      mountedByEl.clear();
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

  /* A reference to a node that does not exist. The rule lives here, not in the
     link's own component, because it has to outrank the link colour above. */
  .chat-markdown :global(a.ns-node-card-inline--missing) {
    color: hsl(var(--muted-foreground));
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
