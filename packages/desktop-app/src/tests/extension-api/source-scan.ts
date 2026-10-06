/**
 * Source scanning for the extension API's boundary tests: export
 * lists and import specifiers, read from the TypeScript
 * syntax tree of each file (no type checking).
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

export const APP_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
export const SRC_ROOT = path.join(APP_ROOT, 'src');
export const LIB_ROOT = path.join(SRC_ROOT, 'lib');
export const HOST_API_DIR = path.join(LIB_ROOT, 'extension-api');

/** Parses TypeScript source; for a `.svelte` file, the contents of its `<script>` blocks. */
export function parseSource(file: string, source = fs.readFileSync(file, 'utf8')): ts.SourceFile {
  const code = file.endsWith('.svelte')
    ? [...source.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n')
    : source;
  return ts.createSourceFile(file, code, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
}

function hasModifier(node: ts.Node, kind: ts.SyntaxKind): boolean {
  return ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((m) => m.kind === kind);
}

/** Resolves an import specifier against `fromFile` to a source path, or `null` for a package. */
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
 * The names a module exports. A namespace re-export
 * (`import * as X from '...'; export { X }`) also contributes `X.<member>` for
 * each export of the namespace's module. Throws on `export *` and
 * `export * as`, whose names cannot be listed from the entry's own text.
 */
export function exportedNames(file: string, source?: string): ExportedName[] {
  const sf = parseSource(file, source);
  const where = path.relative(APP_ROOT, file);

  const namespaces = new Map<string, string>();
  for (const st of sf.statements) {
    if (!ts.isImportDeclaration(st) || !ts.isStringLiteral(st.moduleSpecifier)) continue;
    const bindings = st.importClause?.namedBindings;
    if (bindings && ts.isNamespaceImport(bindings)) {
      namespaces.set(bindings.name.text, st.moduleSpecifier.text);
    }
  }

  const names: ExportedName[] = [];
  for (const st of sf.statements) {
    if (ts.isExportDeclaration(st)) {
      if (!st.exportClause || ts.isNamespaceExport(st.exportClause)) {
        throw new Error(`${where} uses \`export *\`; list its exports explicitly`);
      }
      for (const element of st.exportClause.elements) {
        const name = element.name.text;
        names.push({ name, isType: st.isTypeOnly || element.isTypeOnly });
        const local = (element.propertyName ?? element.name).text;
        const namespace = st.moduleSpecifier === undefined ? namespaces.get(local) : undefined;
        if (namespace === undefined) continue;
        const target = resolveSpecifier(namespace, file);
        if (target === null || !fs.existsSync(target)) {
          throw new Error(`${where}: cannot resolve namespace \`${local}\` (${namespace})`);
        }
        for (const member of exportedNames(target)) {
          names.push({ name: `${name}.${member.name}`, isType: member.isType });
        }
      }
    } else if (ts.isExportAssignment(st)) {
      names.push({ name: 'default', isType: false });
    } else if (hasModifier(st, ts.SyntaxKind.ExportKeyword)) {
      if (hasModifier(st, ts.SyntaxKind.DefaultKeyword)) {
        names.push({ name: 'default', isType: false });
      } else if (ts.isVariableStatement(st)) {
        for (const declaration of st.declarationList.declarations) {
          if (!ts.isIdentifier(declaration.name)) {
            throw new Error(`${where}: export a destructured binding by name instead`);
          }
          names.push({ name: declaration.name.text, isType: false });
        }
      } else if (ts.isInterfaceDeclaration(st) || ts.isTypeAliasDeclaration(st)) {
        names.push({ name: st.name.text, isType: true });
      } else if (
        (ts.isFunctionDeclaration(st) || ts.isClassDeclaration(st) || ts.isEnumDeclaration(st)) &&
        st.name
      ) {
        names.push({ name: st.name.text, isType: false });
      } else {
        throw new Error(`${where}: unrecognized export \`${st.getText(sf).slice(0, 60)}\``);
      }
    }
  }
  return names;
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
 * The module specifiers a file imports or re-exports: static, side-effect and
 * dynamic imports, `export ... from`, and `import('...')` types. Comments and
 * string contents never count, since they are read from the syntax tree.
 */
export function importSpecifiers(file: string, source?: string): string[] {
  const specifiers: string[] = [];
  const visit = (node: ts.Node): void => {
    if (
      (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
      node.moduleSpecifier &&
      ts.isStringLiteral(node.moduleSpecifier)
    ) {
      specifiers.push(node.moduleSpecifier.text);
    } else if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword &&
      node.arguments.length > 0 &&
      ts.isStringLiteralLike(node.arguments[0])
    ) {
      specifiers.push(node.arguments[0].text);
    } else if (
      ts.isImportTypeNode(node) &&
      ts.isLiteralTypeNode(node.argument) &&
      ts.isStringLiteral(node.argument.literal)
    ) {
      specifiers.push(node.argument.literal.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(parseSource(file, source));
  return specifiers;
}
