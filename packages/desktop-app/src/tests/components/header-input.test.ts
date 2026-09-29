import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import HeaderInput from '$lib/design/components/header-input.svelte';

describe('HeaderInput', () => {
  afterEach(cleanup);

  it('is one always-present input that reports typed values, focus and blur', async () => {
    const oninput = vi.fn();
    const onfocus = vi.fn();
    const onblur = vi.fn();
    const { getByLabelText } = render(HeaderInput, {
      props: { value: 'Title', ariaLabel: 'Page title', oninput, onfocus, onblur }
    });
    const input = getByLabelText('Page title') as unknown as { value: string; tagName: string };
    expect(input.tagName).toBe('INPUT');
    expect(input.value).toBe('Title');
    await fireEvent.focus(input as unknown as Element);
    await fireEvent.input(input as unknown as Element, { target: { value: 'New' } });
    await fireEvent.blur(input as unknown as Element);
    expect(onfocus).toHaveBeenCalledTimes(1);
    expect(oninput).toHaveBeenCalledWith('New');
    expect(onblur).toHaveBeenCalledTimes(1);
  });

  it('marks readonly display values', () => {
    const { getByLabelText } = render(HeaderInput, {
      props: { value: 'Computed', ariaLabel: 'Page title', readonly: true }
    });
    const input = getByLabelText('Page title');
    expect(input.hasAttribute('readonly')).toBe(true);
    expect(input.classList.contains('header-input--readonly')).toBe(true);
  });
});
