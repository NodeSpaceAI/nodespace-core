import { describe, it, expect } from 'vitest';
import { evaluateTitleTemplate } from '$lib/utils/title-template';

describe('evaluateTitleTemplate', () => {
  it('interpolates a basic two-token template', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: 'Alice', last_name: 'Smith' })).toBe('Alice Smith');
  });

  it('returns empty string when all tokens are missing', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', {})).toBe('');
  });

  it('returns empty string when all tokens are null', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: null, last_name: null })).toBe('');
  });

  it('returns empty string when all tokens are undefined', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: undefined, last_name: undefined })).toBe('');
  });

  it('handles a partially filled template (one token present)', () => {
    expect(evaluateTitleTemplate('{first_name} {last_name}', { first_name: 'Alice' })).toBe('Alice');
  });

  it('collapses whitespace when a middle token is empty', () => {
    // "Alice  Smith" → trimmed/collapsed to "Alice Smith"
    expect(evaluateTitleTemplate('{first_name} {middle} {last_name}', { first_name: 'Alice', last_name: 'Smith' })).toBe('Alice Smith');
  });

  it('trims leading and trailing whitespace', () => {
    expect(evaluateTitleTemplate('  {first_name}  ', { first_name: 'Alice' })).toBe('Alice');
  });

  it('coerces numeric values to string', () => {
    expect(evaluateTitleTemplate('Invoice #{number}', { number: 42 })).toBe('Invoice #42');
  });

  it('coerces boolean values to string', () => {
    expect(evaluateTitleTemplate('Active: {active}', { active: true })).toBe('Active: true');
  });

  it('handles repeated tokens', () => {
    expect(evaluateTitleTemplate('{name} ({name})', { name: 'Alice' })).toBe('Alice (Alice)');
  });

  it('returns the template literal text when there are no tokens', () => {
    expect(evaluateTitleTemplate('No tokens here', {})).toBe('No tokens here');
  });

  it('does not match hyphenated token names (unsupported by \\w+ regex)', () => {
    // {first-name} is NOT a valid token — the whole {first-name} passes through literally
    expect(evaluateTitleTemplate('{first-name}', { 'first-name': 'Alice' })).toBe('{first-name}');
  });

  it('handles empty template string', () => {
    expect(evaluateTitleTemplate('', { first_name: 'Alice' })).toBe('');
  });
});
