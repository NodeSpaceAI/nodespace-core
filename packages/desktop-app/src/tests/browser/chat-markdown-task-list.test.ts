import { describe, it, expect, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import ChatMarkdown from '$lib/components/chat/chat-markdown.svelte';

describe('ChatMarkdown task lists', () => {
  afterEach(() => cleanup());

  it('renders checkboxes as plain text with their checked state', () => {
    const { container } = render(ChatMarkdown, { content: '- [x] done\n- [ ] todo' });
    expect(container.querySelector('input')).toBeNull();
    const items = Array.from(container.querySelectorAll('li')).map((li) => li.textContent?.trim());
    expect(items).toEqual(['[x] done', '[ ] todo']);
  });
});
