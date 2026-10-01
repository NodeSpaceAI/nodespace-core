/**
 * One `EXTENSION_API_VERSION` covers the TypeScript and Rust extension surfaces
 * (ADR-082 §8), so the host API's constant and the app library's
 * `EXTENSION_API_VERSION` in `app-lib/src/extensions/mod.rs` must agree. A bump
 * that changes only one of them fails here.
 */
import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { EXTENSION_API_VERSION } from '@nodespace/extension-api';
import { APP_ROOT } from './source-scan';

const RUST_MODULE = path.join(APP_ROOT, 'app-lib/src/extensions/mod.rs');

const RUST_DECLARATION =
  /^pub const EXTENSION_API_VERSION:\s*\(u32,\s*u32\)\s*=\s*\(\s*(\d+)\s*,\s*(\d+)\s*\)\s*;/m;

/** The `(major, minor)` the Rust declaration in `source` holds; throws when there is none. */
function rustVersion(source: string): { major: number; minor: number } {
  const match = RUST_DECLARATION.exec(source);
  if (!match) throw new Error('No `pub const EXTENSION_API_VERSION: (u32, u32)` declaration');
  return { major: Number(match[1]), minor: Number(match[2]) };
}

describe('EXTENSION_API_VERSION agreement', () => {
  it('the Rust constant equals the TypeScript one', () => {
    const rust = rustVersion(fs.readFileSync(RUST_MODULE, 'utf8'));

    expect(rust).toEqual({
      major: EXTENSION_API_VERSION.major,
      minor: EXTENSION_API_VERSION.minor
    });
  });

  it('reads the declaration, and refuses a source without one', () => {
    expect(rustVersion('pub const EXTENSION_API_VERSION: (u32, u32) = (7, 3);\n')).toEqual({
      major: 7,
      minor: 3
    });
    expect(() => rustVersion('pub const OTHER: (u32, u32) = (7, 3);\n')).toThrow(
      'No `pub const EXTENSION_API_VERSION'
    );
  });
});
