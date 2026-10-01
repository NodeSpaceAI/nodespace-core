/**
 * Source scanning for the extension API's surface and boundary tests: export
 * lists, type declarations and import specifiers, read from source text with
 * targeted parsing rather than a TypeScript program.
 */
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const APP_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
export const SRC_ROOT = path.join(APP_ROOT, 'src');
export const LIB_ROOT = path.join(SRC_ROOT, 'lib');
export const HOST_API_DIR = path.join(LIB_ROOT, 'extension-api');

/**
 * Removes `//` and `/* *\/` comments, leaving string and template literals
 * intact. Regex literals are not recognized, so this is for files whose code
 * has none; a quote that would mislead it ends up unterminated, which throws
 * rather than silently dropping code.
 */
export function stripComments(source: string): string {
  let out = '';
  let i = 0;
  while (i < source.length) {
    const ch = source[i];
    const next = source[i + 1];
    if (ch === '/' && next === '/') {
      while (i < source.length && source[i] !== '\n') i++;
      continue;
    }
    if (ch === '/' && next === '*') {
      const end = source.indexOf('*/', i + 2);
      if (end === -1) throw new Error('Unterminated block comment');
      // Keep line breaks so a comment between two tokens still separates them.
      out += source.slice(i, end + 2).includes('\n') ? '\n' : ' ';
      i = end + 2;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const start = i;
      i++;
      while (i < source.length && source[i] !== ch) {
        if (source[i] === '\\') i++;
        else if (source[i] === '\n' && ch !== '`') {
          throw new Error(`Unterminated string literal: ${source.slice(start, i)}`);
        }
        i++;
      }
      if (i >= source.length)
        throw new Error(`Unterminated string literal: ${source.slice(start)}`);
      out += source.slice(start, i + 1);
      i++;
      continue;
    }
    out += ch;
    i++;
  }
  return out;
}

/** Resolves an import specifier against `fromFile` to a source file, or `null` for a package. */
export function resolveSpecifier(specifier: string, fromFile: string): string | null {
  let base: string;
  if (specifier.startsWith('$lib/')) base = path.join(LIB_ROOT, specifier.slice('$lib/'.length));
  else if (specifier === '$lib') base = LIB_ROOT;
  else if (specifier.startsWith('@nodespace/extension-api')) {
    base = path.join(HOST_API_DIR, specifier.slice('@nodespace/extension-api'.length));
  } else if (specifier.startsWith('.')) base = path.resolve(path.dirname(fromFile), specifier);
  else return null;
  for (const candidate of [base, `${base}.ts`, `${base}.js`, path.join(base, 'index.ts')]) {
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) return candidate;
  }
  return base;
}

export interface ExportedName {
  name: string;
  /** Exported with `type`, or an `interface` / `type` declaration: absent at runtime. */
  isType: boolean;
}

/**
 * The names a module exports, read from its source. A namespace re-export
 * (`import * as X from '...'; export { X }`) also contributes `X.<member>` for
 * each export of the namespace's module. Throws on `export *`, whose names
 * cannot be listed from the entry's own text.
 */
export function exportedNames(file: string): ExportedName[] {
  const code = stripComments(fs.readFileSync(file, 'utf8'));
  if (/\bexport\s*\*/.test(code)) {
    throw new Error(
      `${path.relative(APP_ROOT, file)} uses \`export *\`; list its exports explicitly`
    );
  }

  const namespaces = new Map<string, string>();
  for (const m of code.matchAll(/\bimport\s+\*\s+as\s+([\w$]+)\s+from\s+['"]([^'"]+)['"]/g)) {
    namespaces.set(m[1], m[2]);
  }

  const names: ExportedName[] = [];
  for (const m of code.matchAll(
    /\bexport\s+(type\s+)?\{([^}]*)\}(?:\s*from\s*['"]([^'"]+)['"])?/g
  )) {
    const listIsType = m[1] !== undefined;
    const fromSpecifier = m[3];
    for (const raw of m[2].split(',')) {
      const item = raw.trim();
      if (item === '') continue;
      const parts = /^(type\s+)?([\w$]+)(?:\s+as\s+([\w$]+))?$/.exec(item);
      if (!parts) throw new Error(`Unrecognized export item \`${item}\` in ${file}`);
      const local = parts[2];
      const name = parts[3] ?? local;
      names.push({ name, isType: listIsType || parts[1] !== undefined });
      const namespace = fromSpecifier === undefined ? namespaces.get(local) : undefined;
      if (namespace !== undefined) {
        const target = resolveSpecifier(namespace, file);
        if (target === null || !fs.existsSync(target)) {
          throw new Error(`Cannot resolve namespace \`${local}\` (${namespace}) from ${file}`);
        }
        for (const member of exportedNames(target)) {
          names.push({ name: `${name}.${member.name}`, isType: member.isType });
        }
      }
    }
  }
  for (const m of code.matchAll(
    /\bexport\s+(?:declare\s+)?(?:async\s+)?(const|let|var|function\*?|class|interface|type|enum)\s+([\w$]+)/g
  )) {
    names.push({ name: m[2], isType: m[1] === 'interface' || m[1] === 'type' });
  }
  if (/\bexport\s+default\b/.test(code)) names.push({ name: 'default', isType: false });
  return names;
}

/** Collapses formatting that does not change a declaration's meaning. */
export function normalizeDeclaration(text: string): string {
  return text
    .replace(/\s+/g, ' ')
    .replace(/ ?([^\w$ '"`]) ?/g, '$1')
    .replace(/([=(<:,[])[|&]/g, '$1')
    .replace(/[;,]([}\])>])/g, '$1')
    .trim();
}

/**
 * The end (exclusive) of the `interface` or `type` declaration that starts at
 * `start`: the closing brace of an interface's body, or the `;` that ends a type
 * alias at bracket depth 0.
 */
function declarationEnd(code: string, start: number, kind: 'interface' | 'type'): number {
  let curly = 0;
  let other = 0;
  let angle = 0;
  let bodyOpened = false;
  for (let i = start; i < code.length; i++) {
    const ch = code[i];
    if (ch === "'" || ch === '"' || ch === '`') {
      i++;
      while (i < code.length && code[i] !== ch) {
        if (code[i] === '\\') i++;
        i++;
      }
      continue;
    }
    if (ch === '<') angle++;
    else if (ch === '>' && code[i - 1] !== '=') angle--;
    else if (ch === '(' || ch === '[') other++;
    else if (ch === ')' || ch === ']') other--;
    else if (ch === '{') {
      if (kind === 'interface' && curly === 0 && angle === 0 && other === 0) bodyOpened = true;
      curly++;
    } else if (ch === '}') {
      curly--;
      if (kind === 'interface' && bodyOpened && curly === 0) return i + 1;
    } else if (ch === ';' && kind === 'type' && curly === 0 && angle === 0 && other === 0) {
      return i + 1;
    }
  }
  throw new Error(`Unterminated ${kind} declaration at offset ${start}`);
}

/**
 * Every `interface` and `type` declaration in `file`, as normalized text keyed by
 * name. `exportedOnly` skips the ones without `export`.
 */
export function typeDeclarations(file: string, exportedOnly: boolean): Map<string, string> {
  const code = stripComments(fs.readFileSync(file, 'utf8'));
  const declarations = new Map<string, string>();
  // The lookahead keeps an import item such as `  type Foo,` from counting.
  for (const m of code.matchAll(
    /^[ \t]*(export\s+)?(?:declare\s+)?(interface|type)\s+([\w$]+)\s*(?=[<={]|extends\b)/gm
  )) {
    if (exportedOnly && m[1] === undefined) continue;
    const kind = m[2] as 'interface' | 'type';
    const start = m.index ?? 0;
    const text = code.slice(start, declarationEnd(code, start, kind));
    declarations.set(m[3], normalizeDeclaration(text.replace(/^\s*export\s+/, '')));
  }
  return declarations;
}

/** SHA-256 over declarations, independent of their order. */
export function hashDeclarations(declarations: Map<string, string>): string {
  const lines = [...declarations].sort(([a], [b]) => a.localeCompare(b));
  return createHash('sha256')
    .update(lines.map(([name, text]) => `${name}\t${text}`).join('\n'))
    .digest('hex');
}

/** Every `.ts` and `.svelte` file under `dir`, recursively. */
export function sourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(full);
    return /\.(ts|svelte)$/.test(entry.name) ? [full] : [];
  });
}

/**
 * The module specifiers `source` imports or re-exports: static, side-effect,
 * dynamic and `import('...')` type references. In a `.svelte` file only the
 * `<script>` blocks count. Lines that are comments (`//`, `/*`, or a `*` doc
 * continuation) are skipped, as `shared-node-store-pin-guard.test.ts` does; a
 * full comment parser is not needed for import statements.
 */
export function importSpecifiers(source: string, isSvelte: boolean): string[] {
  const code = isSvelte
    ? [...source.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n')
    : source;
  const lines = code.split('\n').filter((line) => {
    const trimmed = line.trim();
    return !trimmed.startsWith('//') && !trimmed.startsWith('*') && !trimmed.startsWith('/*');
  });
  const text = lines.join('\n');
  const specifiers: string[] = [];
  for (const m of text.matchAll(/\bfrom\s*['"]([^'"\n]+)['"]/g)) specifiers.push(m[1]);
  for (const m of text.matchAll(/^\s*import\s*['"]([^'"\n]+)['"]/gm)) specifiers.push(m[1]);
  for (const m of text.matchAll(/\bimport\s*\(\s*['"]([^'"\n]+)['"]\s*\)/g)) specifiers.push(m[1]);
  return specifiers;
}
