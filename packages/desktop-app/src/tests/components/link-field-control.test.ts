/**
 * LinkFieldControl — shows a link as its title, opens only web URLs, and
 * writes a link whole: a complete one, or `null` to clear.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const { openUrl } = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock('$lib/utils/external-links', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/utils/external-links')>()),
  openUrl
}));

import LinkFieldControl from '$lib/components/schema/link-field-control.svelte';

const core = { title: 'Core', url: 'https://example.com/core' };

function renderControl(value: unknown) {
  const onChange = vi.fn();
  const view = render(LinkFieldControl, { props: { value, fieldId: 'link-field', onChange } });
  return { ...view, onChange };
}

async function fillAndSave(
  view: ReturnType<typeof renderControl>,
  title: string,
  url: string
): Promise<void> {
  await fireEvent.input(view.getByLabelText('Link title'), { target: { value: title } });
  await fireEvent.input(view.getByLabelText('Link URL'), { target: { value: url } });
  await fireEvent.click(view.getByText('Save'));
}

beforeEach(() => {
  openUrl.mockClear();
});

afterEach(() => {
  cleanup();
});

describe('LinkFieldControl', () => {
  it('shows the title and opens the URL in the system browser', async () => {
    const view = renderControl(core);
    await fireEvent.click(view.getByText('Core'));
    expect(openUrl).toHaveBeenCalledWith('https://example.com/core');
  });

  it('shows a link with another scheme as text, with nothing to open', () => {
    const view = renderControl({ title: 'Clone', url: 'ssh://git@example.com/core.git' });
    expect(view.getByTestId('link-text').textContent?.trim()).toBe(
      'Clone (ssh://git@example.com/core.git)'
    );
    expect(view.queryByRole('button', { name: 'Clone' })).toBeNull();
  });

  it('edits the title and URL and writes the link whole', async () => {
    const view = renderControl(core);
    await fireEvent.click(view.getByText('Edit'));
    expect((view.getByLabelText('Link title') as HTMLInputElement).value).toBe('Core');
    await fillAndSave(view, 'Docs', 'https://example.com/docs');
    expect(view.onChange).toHaveBeenCalledWith({ title: 'Docs', url: 'https://example.com/docs' });
  });

  it('adds a link where there is none', async () => {
    const view = renderControl(null);
    await fireEvent.click(view.getByText('Add link'));
    await fillAndSave(view, 'Core', 'https://example.com/core');
    expect(view.onChange).toHaveBeenCalledWith(core);
  });

  it('does not write a URL that is not absolute', async () => {
    const view = renderControl(null);
    await fireEvent.click(view.getByText('Add link'));
    await fillAndSave(view, 'Core', 'example.com/core');
    expect(view.onChange).not.toHaveBeenCalled();
    expect(view.getByRole('alert').textContent).toContain('full URL');
  });

  it('clears the link when both inputs are emptied', async () => {
    const view = renderControl(core);
    await fireEvent.click(view.getByText('Edit'));
    await fillAndSave(view, '', '');
    expect(view.onChange).toHaveBeenCalledWith(null);
  });

  it('writes nothing when an unset link is left blank or cancelled', async () => {
    const view = renderControl(null);
    await fireEvent.click(view.getByText('Add link'));
    await fireEvent.click(view.getByText('Save'));
    await fireEvent.click(view.getByText('Add link'));
    await fireEvent.click(view.getByText('Cancel'));
    expect(view.onChange).not.toHaveBeenCalled();
  });
});
