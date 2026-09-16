/**
 * Guards the single-accent rule for node type colors.
 *
 * DESIGN.md: node type colors "all map to primary. They share a single accent
 * to keep the node list visually quiet." Four disagreeing definitions of the
 * --node-* variables once coexisted, so which color a node rendered in depended
 * on which component tree drew it.
 *
 * Stylelint cannot catch a return of that bug. Its design-token rules reject a
 * custom property assigned a raw literal, so `--node-user: 200 100% 45%` in a
 * second file would be flagged — but `--node-user: var(--chart-3)` passes every
 * rule while re-fragmenting the system just as badly. There is no "this
 * property may only be declared in this file" rule, so the invariant needs an
 * executable guard.
 */

import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const srcRoot = path.join(packageRoot, 'src');
const appCssPath = path.join(srcRoot, 'app.css');

/** Every .css and .svelte file under src/, recursively. */
function styleBearingFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return styleBearingFiles(full);
    return /\.(css|svelte)$/.test(entry.name) ? [full] : [];
  });
}

/**
 * Matches a *declaration* of a --node-* custom property (`--node-foo: value;`),
 * not a var() reference to one. --node-indent and other non-color node vars are
 * excluded: the single-accent rule is about color only.
 */
const NODE_COLOR_DECLARATION = /^\s*(--node-[a-z-]+)\s*:\s*([^;]+);/gm;
const NON_COLOR_NODE_VARS = new Set(['--node-indent']);

function nodeColorDeclarations(source: string): Array<{ name: string; value: string }> {
  return [...source.matchAll(NODE_COLOR_DECLARATION)]
    .map((m) => ({ name: m[1], value: m[2].trim() }))
    .filter((d) => !NON_COLOR_NODE_VARS.has(d.name));
}

describe('node accent colors', () => {
  it('declares every --node-* color variable only in app.css', () => {
    const offenders = styleBearingFiles(srcRoot)
      .filter((file) => file !== appCssPath)
      .flatMap((file) =>
        nodeColorDeclarations(fs.readFileSync(file, 'utf8')).map(
          (d) => `${path.relative(packageRoot, file)} declares ${d.name}: ${d.value}`
        )
      );

    expect(offenders).toEqual([]);
  });

  it('derives every node color from --primary so the two cannot drift', () => {
    const declarations = nodeColorDeclarations(fs.readFileSync(appCssPath, 'utf8'));

    expect(declarations.length).toBeGreaterThan(0);
    for (const { name, value } of declarations) {
      expect(`${name}: ${value}`).toBe(`${name}: var(--primary)`);
    }
  });

  it('defines every node color variable the icon registry references', () => {
    const registry = fs.readFileSync(
      path.join(srcRoot, 'lib/design/icons/registry.ts'),
      'utf8'
    );
    const defined = new Set(
      nodeColorDeclarations(fs.readFileSync(appCssPath, 'utf8')).map((d) => d.name)
    );

    // A referenced-but-undefined var resolves to nothing at runtime, which is
    // how --node-project silently rendered in a stale fallback color.
    const referenced = [...registry.matchAll(/var\((--node-[a-z-]+)\)/g)].map((m) => m[1]);

    expect(referenced.length).toBeGreaterThan(0);
    expect([...new Set(referenced)].filter((name) => !defined.has(name))).toEqual([]);
  });
});
