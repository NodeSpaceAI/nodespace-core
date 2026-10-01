/**
 * The REQUIRES_EXTENSION CommandError guard: it accepts only the shape the app
 * library sends for a database that requires an extension this build does not
 * support, so the refusal view never renders a partial or foreign payload.
 */
import { describe, it, expect } from 'vitest';
import {
  isRequiresExtension,
  type RequiresExtensionCommandError
} from '$lib/types/requires-extension';

function refusal(
  payload: Record<string, unknown> = {},
  error: Record<string, unknown> = {}
): Record<string, unknown> {
  return {
    message: 'This database needs Fixture App',
    code: 'REQUIRES_EXTENSION',
    details: 'FailedPrecondition',
    requiresExtension: {
      unsupportedExtensions: ['fixture-ext'],
      message: 'This database needs Fixture App',
      downloadLabel: 'Download Fixture App',
      downloadUrl: 'https://example.test/fixture-app',
      ...payload
    },
    ...error
  };
}

describe('isRequiresExtension', () => {
  it('accepts a well-formed REQUIRES_EXTENSION error and narrows to its payload', () => {
    const error: unknown = refusal();

    expect(isRequiresExtension(error)).toBe(true);
    const narrowed = error as RequiresExtensionCommandError;
    expect(narrowed.requiresExtension.downloadUrl).toBe('https://example.test/fixture-app');
  });

  it('accepts an empty list of unsupported extensions', () => {
    expect(isRequiresExtension(refusal({ unsupportedExtensions: [] }))).toBe(true);
  });

  it('rejects anything that is not an object', () => {
    for (const value of [null, undefined, 'REQUIRES_EXTENSION', 42, true]) {
      expect(isRequiresExtension(value)).toBe(false);
    }
  });

  it('rejects another error code, even with a payload', () => {
    expect(isRequiresExtension(refusal({}, { code: 'VERSION_CONFLICT' }))).toBe(false);
    expect(isRequiresExtension(refusal({}, { code: undefined }))).toBe(false);
  });

  it('rejects an error without a string message', () => {
    expect(isRequiresExtension(refusal({}, { message: undefined }))).toBe(false);
  });

  it('rejects a REQUIRES_EXTENSION error without a payload object', () => {
    expect(isRequiresExtension(refusal({}, { requiresExtension: undefined }))).toBe(false);
    expect(isRequiresExtension(refusal({}, { requiresExtension: null }))).toBe(false);
    expect(isRequiresExtension(refusal({}, { requiresExtension: 'pending' }))).toBe(false);
  });

  it.each([
    ['unsupportedExtensions', undefined],
    ['unsupportedExtensions', 'fixture-ext'],
    ['unsupportedExtensions', ['fixture-ext', 7]],
    ['message', undefined],
    ['message', 7],
    ['downloadLabel', undefined],
    ['downloadLabel', null],
    ['downloadUrl', undefined],
    ['downloadUrl', 7]
  ])('rejects a payload whose %s is %j', (field, value) => {
    expect(isRequiresExtension(refusal({ [field]: value }))).toBe(false);
  });

  it('rejects a download link that is not https', () => {
    // The refusal carries a download link and nothing else.
    for (const downloadUrl of [
      'http://example.test/fixture-app',
      'mailto:someone@example.test',
      'javascript:void(0)',
      ''
    ]) {
      expect(isRequiresExtension(refusal({ downloadUrl }))).toBe(false);
    }
  });
});
