#!/usr/bin/env bun
// Keeps the per-type reference, `components/node-types.md` in the docs
// repository, equal to the core node types the code ships (ADR-086).
//
// It compares two things with what `packages/core/examples/dump_core_schemas.rs`
// prints (the `CoreNodeType` registry and `get_core_schemas()`):
//
// 1. The registry table: every concrete core type, abstract base and core
//    subtype is listed under its kind, and nothing else is.
// 2. One sheet per type. A type's sheet is the section under a heading or a
//    bold lead that opens with the type's name in backticks, or its row in the
//    primitive types table. The sheet's field table (the one with `Field`,
//    `Storage` and `Type` columns) lists exactly the fields the type's own
//    schema declares, each with its storage key (`<type>.<field>`), its type
//    and, for an enum, its values in order.
//
// The reference describes the decided design and carries no status labels, so
// this passes only while code and reference agree: a change to a core type's
// shape updates both.
//
// The docs repository is a sibling of the primary checkout. A worktree lives
// inside the primary checkout, so the sibling is found from git's common
// directory rather than relative to this checkout. NODESPACE_DOCS_DIR names it
// explicitly. A machine without the docs checkout skips the check with a
// warning; an explicit NODESPACE_DOCS_DIR that has no reference is an error.
//
// A plain run reads the docs working tree, so a type and its sheet can be
// changed together and checked before either is committed. The merge gate
// passes --published and compares with the docs remote's main instead: what a
// merge is tested against must not depend on the state of a checkout on the
// machine that happens to run the gate. So a type change lands docs first:
// push the sheet, then queue the merge.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { primaryRootFromCommonDir } from "./setup-rust-tooling";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

export const DOCS_DIR_ENV_VAR = "NODESPACE_DOCS_DIR";
const DOCS_REPO_NAME = "nodespace-docs";
const DOC_FILE = join("components", "node-types.md");

/** One core type, as the registry records it. */
export interface DumpedType {
  id: string;
  kind: RegistryKind;
}

/** One field of a seeded schema: the parts the reference documents. */
export interface DumpedField {
  name: string;
  type: string;
  itemType?: string;
  coreValues?: { value: string }[];
  extensible?: boolean;
}

export interface DumpedSchema {
  id: string;
  fields: DumpedField[];
}

/** What `dump_core_schemas` prints. */
export interface CoreSchemaDump {
  types: DumpedType[];
  schemas: DumpedSchema[];
}

export type RegistryKind = "concrete" | "abstract_base" | "core_subtype";

/** The registry table's row label for each kind. */
const KIND_ROWS: Record<RegistryKind, string> = {
  concrete: "Concrete core types",
  abstract_base: "Abstract bases",
  core_subtype: "Core subtypes",
};

/** One documented field, from a row of a sheet's field table. */
export interface DocField {
  name: string;
  /** The storage key as written, e.g. `task.status`. */
  storage: string;
  type: string;
  itemType?: string;
  /** An enum's values, in the order listed. */
  enumValues?: string[];
  /** Whether the row says `extensible` or `closed`; an enum row says one. */
  extensible?: boolean;
  line: number;
}

export interface DocSheet {
  type: string;
  line: number;
  fields: DocField[];
}

export interface ParsedDoc {
  /** The registry table's types per kind; a kind whose row is missing is absent. */
  registry: Partial<Record<RegistryKind, { types: string[]; line: number }>>;
  sheets: Map<string, DocSheet>;
  /** Parts of the document the parser could not read, each naming its line. */
  problems: string[];
}

const TYPE_NAME = "[a-z][a-z0-9-]*";
/** A heading or a bold lead that opens with a type's name in backticks. */
const SHEET_ANCHOR = new RegExp(`^(?:#{2,6}\\s+|\\*\\*)\`(${TYPE_NAME})\``);
const BACKTICKED = /`([^`]+)`/g;
/** The run of backticked, comma-separated tokens a cell opens with. */
const LEADING_VALUES = /^`[^`]+`(?:\s*,\s*`[^`]+`)*/;

/** The reference's type words, as the schema field types they stand for. */
const DOC_SCALARS: Record<string, string> = {
  string: "text",
  number: "number",
  boolean: "boolean",
  date: "date",
  datetime: "datetime",
  enum: "enum",
  object: "object",
};

/** The cells of a table row, or null when the line isn't one. */
function tableCells(line: string): string[] | null {
  const trimmed = line.trim();
  if (!trimmed.startsWith("|")) return null;
  // An escaped pipe is cell content.
  const cells = trimmed.split(/(?<!\\)\|/).map((cell) => cell.trim());
  return cells.slice(1, trimmed.endsWith("|") ? -1 : undefined);
}

function isDelimiterRow(cells: string[]): boolean {
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell));
}

function backticked(cell: string): string[] {
  return [...cell.matchAll(BACKTICKED)].map((match) => match[1]);
}

/** A `Type` cell's schema field type, or null when it opens with no known type word. */
export function parseDocType(cell: string): { type: string; itemType?: string } | null {
  const text = cell.trim();
  // An array of a named object type: `QueryFilter[]`.
  if (/^`[A-Z][A-Za-z0-9]*\[\]`/.test(text)) return { type: "array", itemType: "object" };
  const word = /^([a-z]+)(\[\])?(?![\w-])/.exec(text);
  if (word === null) return null;
  const scalar = DOC_SCALARS[word[1]];
  if (scalar === undefined) return null;
  return word[2] === undefined ? { type: scalar } : { type: "array", itemType: scalar };
}

/**
 * An enum row's values: the backticked list one of its cells opens with. The
 * `Values` column comes first, then what follows `enum`, `enum, closed:` or
 * `enum, extensible:` in the `Type` cell, then the `Notes` column.
 */
function enumValues(headers: string[], cells: string[], typeColumn: number): string[] | undefined {
  const columnsNamed = (prefix: string) =>
    headers.map((header, i) => (header.startsWith(prefix) ? i : -1)).filter((i) => i !== -1);
  const order = [...columnsNamed("Values"), typeColumn, ...columnsNamed("Notes")];
  for (const column of order) {
    const cell = (cells[column] ?? "").trim();
    const text = column === typeColumn ? cell.replace(/^enum(?:,\s*(?:extensible|closed))?[:\s]*/, "") : cell;
    const run = LEADING_VALUES.exec(text);
    if (run !== null) return backticked(run[0]);
  }
  return undefined;
}

/** Reads the registry table, the primitive types table and every sheet's field table. */
export function parseNodeTypesDoc(markdown: string): ParsedDoc {
  const doc: ParsedDoc = { registry: {}, sheets: new Map(), problems: [] };
  const lines = markdown.split("\n");
  let current: DocSheet | null = null;

  const sheetFor = (type: string, line: number): DocSheet => {
    const existing = doc.sheets.get(type);
    if (existing !== undefined) return existing;
    const sheet: DocSheet = { type, line, fields: [] };
    doc.sheets.set(type, sheet);
    return sheet;
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const lineNumber = i + 1;

    const anchor = SHEET_ANCHOR.exec(line);
    if (anchor !== null) {
      current = sheetFor(anchor[1], lineNumber);
      continue;
    }
    // Any other heading ends the sheet above it.
    if (line.startsWith("#")) {
      current = null;
      continue;
    }

    const headers = tableCells(line);
    const delimiter = headers === null ? null : tableCells(lines[i + 1] ?? "");
    if (headers === null || delimiter === null || !isDelimiterRow(delimiter)) continue;

    const rows: { cells: string[]; line: number }[] = [];
    let next = i + 2;
    for (; next < lines.length; next++) {
      const cells = tableCells(lines[next]);
      if (cells === null) break;
      rows.push({ cells, line: next + 1 });
    }
    i = next - 1;

    if (headers[0] === "Kind" && headers[1] === "Types") {
      for (const row of rows) {
        const kind = (Object.keys(KIND_ROWS) as RegistryKind[]).find((k) => KIND_ROWS[k] === row.cells[0]);
        if (kind === undefined) continue;
        const types = (row.cells[1] ?? "")
          .replace(/\([^)]*\)/g, "")
          .split(",")
          .map((type) => type.trim())
          .filter((type) => type !== "");
        doc.registry[kind] = { types, line: row.line };
      }
      continue;
    }

    // The primitive types table: one row is one type's whole sheet.
    if (headers[0] === "Type") {
      for (const row of rows) {
        const names = backticked(row.cells[0] ?? "");
        if (names.length === 1) sheetFor(names[0], row.line);
        else doc.problems.push(`line ${row.line}: a row of the types table doesn't open with one type name in backticks`);
      }
      continue;
    }

    const fieldColumn = headers.indexOf("Field");
    const storageColumn = headers.indexOf("Storage");
    const typeColumn = headers.indexOf("Type");
    if (fieldColumn === -1 || storageColumn === -1 || typeColumn === -1) continue;
    if (current === null) {
      doc.problems.push(`line ${lineNumber}: a field table that is not under a type's sheet`);
      continue;
    }

    for (const row of rows) {
      const names = backticked(row.cells[fieldColumn] ?? "");
      const storage = backticked(row.cells[storageColumn] ?? "")[0];
      if (storage === undefined) {
        doc.problems.push(`line ${row.line}: the Storage cell names no storage key in backticks`);
        continue;
      }
      // `content` is an envelope column, not a schema field.
      if (names.length === 0 && storage === "content") continue;
      if (names.length === 0) {
        doc.problems.push(`line ${row.line}: the Field cell names no field in backticks`);
        continue;
      }
      const shared = storage.endsWith(".*");
      if (names.length > 1 && !shared) {
        doc.problems.push(
          `line ${row.line}: a row for ${names.length} fields needs a \`<type>.*\` storage key, not \`${storage}\``
        );
        continue;
      }
      const parsedType = parseDocType(row.cells[typeColumn] ?? "");
      if (parsedType === null) {
        doc.problems.push(`line ${row.line}: the Type cell "${row.cells[typeColumn] ?? ""}" opens with no known type`);
        continue;
      }
      const typeCell = row.cells[typeColumn] ?? "";
      const extensible = /\bextensible\b/.test(typeCell) ? true : /\bclosed\b/.test(typeCell) ? false : undefined;
      const values = parsedType.type === "enum" ? enumValues(headers, row.cells, typeColumn) : undefined;
      for (const name of names) {
        current.fields.push({
          name,
          storage: shared ? `${storage.slice(0, -1)}${name}` : storage,
          ...parsedType,
          enumValues: values,
          extensible,
          line: row.line,
        });
      }
    }
  }
  return doc;
}

function describeType(type: string, itemType?: string): string {
  return itemType === undefined ? type : `${type} of ${itemType}`;
}

/**
 * Every way the reference and the code disagree, one message each. Empty when
 * they match.
 */
export function compareNodeTypesDoc(dump: CoreSchemaDump, markdown: string): string[] {
  const doc = parseNodeTypesDoc(markdown);
  const problems = [...doc.problems];
  const registered = new Set(dump.types.map((type) => type.id));

  for (const kind of Object.keys(KIND_ROWS) as RegistryKind[]) {
    const inCode = dump.types.filter((type) => type.kind === kind).map((type) => type.id);
    const row = doc.registry[kind];
    if (row === undefined) {
      problems.push(`the registry table has no "${KIND_ROWS[kind]}" row`);
      continue;
    }
    for (const type of inCode) {
      if (!row.types.includes(type)) {
        problems.push(
          `line ${row.line}: \`${type}\` is registered as one of the ${KIND_ROWS[kind].toLowerCase()}, and the registry table doesn't list it there`
        );
      }
    }
    for (const type of row.types) {
      if (!inCode.includes(type)) {
        problems.push(
          `line ${row.line}: the registry table lists \`${type}\` under "${KIND_ROWS[kind]}", and the code registers no such type of that kind`
        );
      }
    }
  }

  for (const schema of dump.schemas) {
    if (!registered.has(schema.id)) {
      problems.push(`the code seeds a \`${schema.id}\` schema for a type the registry doesn't list`);
    }
  }

  for (const sheet of doc.sheets.values()) {
    if (!registered.has(sheet.type) && sheet.fields.length > 0) {
      problems.push(`line ${sheet.line}: \`${sheet.type}\` has a field table, and the code registers no such core type`);
    }
  }

  for (const type of dump.types) {
    const sheet = doc.sheets.get(type.id);
    if (sheet === undefined) {
      problems.push(`\`${type.id}\` is a core type with no sheet`);
      continue;
    }
    // The `schema` meta-type is registered but has no seeded schema of its
    // own, so its sheet has no field table.
    const schema = dump.schemas.find((candidate) => candidate.id === type.id);
    if (schema === undefined) {
      for (const field of sheet.fields) {
        problems.push(
          `line ${field.line}: the \`${type.id}\` sheet lists \`${field.name}\`, and the code seeds no \`${type.id}\` schema`
        );
      }
      continue;
    }

    const documented = new Map<string, DocField>();
    for (const field of sheet.fields) {
      if (documented.has(field.name)) {
        problems.push(`line ${field.line}: \`${type.id}\` lists the field \`${field.name}\` twice`);
      }
      documented.set(field.name, field);
    }

    for (const field of schema.fields) {
      const docField = documented.get(field.name);
      if (docField === undefined) {
        problems.push(
          `line ${sheet.line}: the \`${type.id}\` schema declares \`${field.name}\`, and its sheet's field table doesn't list it`
        );
        continue;
      }
      const at = `line ${docField.line}: \`${type.id}.${field.name}\``;
      const storage = `${type.id}.${field.name}`;
      if (docField.storage !== storage) {
        problems.push(`${at} is stored at \`${storage}\`, and the sheet says \`${docField.storage}\``);
      }
      if (docField.type !== field.type || docField.itemType !== field.itemType) {
        problems.push(
          `${at} is ${describeType(field.type, field.itemType)} in the schema and ${describeType(docField.type, docField.itemType)} in the sheet`
        );
        continue;
      }
      if (field.type !== "enum") continue;
      const values = (field.coreValues ?? []).map((value) => value.value);
      if (docField.enumValues === undefined) {
        problems.push(`${at} is an enum, and the sheet lists no values for it (the schema's: ${values.join(", ")})`);
      } else if (docField.enumValues.join("\n") !== values.join("\n")) {
        problems.push(`${at} has the values ${values.join(", ")}, and the sheet lists ${docField.enumValues.join(", ")}`);
      }
      if (docField.extensible === undefined) {
        problems.push(`${at} is an enum, and the sheet's Type cell says neither \`extensible\` nor \`closed\``);
      } else if (docField.extensible !== (field.extensible ?? false)) {
        problems.push(
          `${at} is ${field.extensible ? "extensible" : "closed"} in the schema and ${docField.extensible ? "extensible" : "closed"} in the sheet`
        );
      }
    }
    for (const field of sheet.fields) {
      if (!schema.fields.some((declared) => declared.name === field.name)) {
        problems.push(
          `line ${field.line}: the \`${type.id}\` sheet lists \`${field.name}\`, and the schema declares no such field`
        );
      }
    }
  }
  return problems;
}

export interface DocsLocation {
  dir: string;
  /** Whether NODESPACE_DOCS_DIR named it, rather than the sibling default. */
  explicit: boolean;
}

/**
 * Where the docs repository is: NODESPACE_DOCS_DIR, else the sibling of the
 * primary checkout. A worktree's own parent is a directory inside the primary
 * checkout, so the sibling is found from git's common directory.
 */
export async function resolveDocsDir(
  cwd: string = REPO,
  env: Record<string, string | undefined> = process.env
): Promise<DocsLocation> {
  const explicit = env[DOCS_DIR_ENV_VAR];
  if (explicit) return { dir: resolve(cwd, explicit), explicit: true };
  const commonDir = (await $`git rev-parse --path-format=absolute --git-common-dir`.cwd(cwd).quiet().text()).trim();
  return { dir: join(dirname(primaryRootFromCommonDir(commonDir)), DOCS_REPO_NAME), explicit: false };
}

/** What the merge gate reads: the docs repository's published main. */
const PUBLISHED_REF = "origin/main";

export type DocRead = { markdown: string; source: string } | { skip: string } | { error: string };

/**
 * Why this machine can't run the check: it has no docs checkout. Null when it
 * has one, or when NODESPACE_DOCS_DIR names one, which is then an error to
 * report if it isn't there, since skipping would hide the mistake.
 */
export function skipReason(docs: DocsLocation): string | null {
  if (docs.explicit || existsSync(docs.dir)) return null;
  return `no docs checkout at ${docs.dir}. Clone the docs repository beside the primary checkout, or set ${DOCS_DIR_ENV_VAR}.`;
}

/**
 * The reference's text. `published` reads the docs repository's main as its
 * remote has it, fetched first: the merge gate's verdict then depends on
 * neither the edits someone has in progress in that checkout nor on how
 * recently it was pulled. Otherwise the working tree is read, which is what a
 * change to a type and its sheet is developed against.
 */
export async function readDoc(docs: DocsLocation, published: boolean): Promise<DocRead> {
  const skip = skipReason(docs);
  if (skip !== null) return { skip };
  if (!existsSync(docs.dir)) return { error: `${DOCS_DIR_ENV_VAR} is set, and ${docs.dir} does not exist.` };
  const path = join(docs.dir, DOC_FILE);
  if (!published) {
    if (!existsSync(path)) return { error: `${path} does not exist.` };
    return { markdown: readFileSync(path, "utf8"), source: path };
  }
  const fetched = await $`git fetch --quiet origin`.cwd(docs.dir).quiet().nothrow();
  const shown = await $`git show ${`${PUBLISHED_REF}:${DOC_FILE}`}`.cwd(docs.dir).quiet().nothrow();
  if (shown.exitCode !== 0) {
    return { error: `could not read ${DOC_FILE} at ${PUBLISHED_REF} in ${docs.dir}:\n${shown.stderr.toString().trim()}` };
  }
  const commit = (await $`git rev-parse --short ${PUBLISHED_REF}`.cwd(docs.dir).quiet().nothrow().text()).trim();
  const stale = fetched.exitCode === 0 ? "" : ", as last fetched: the fetch failed";
  return { markdown: shown.stdout.toString(), source: `${DOC_FILE} at ${PUBLISHED_REF} (${commit}${stale}) in ${docs.dir}` };
}

/** Why a merge gate or a test:changed run on this machine will skip the check; null when it won't. */
export async function stageSkipReason(): Promise<string | null> {
  return skipReason(await resolveDocsDir());
}

if (import.meta.main) {
  // --published: compare with the docs repository's remote main, not its working tree.
  const doc = await readDoc(await resolveDocsDir(), process.argv.includes("--published"));
  if ("error" in doc) {
    console.error(`❌ ${doc.error}`);
    process.exit(1);
  }
  if ("skip" in doc) {
    console.warn(`⚠ Skipping the node-types.md check: ${doc.skip}`);
    process.exit(0);
  }

  const dumped = await $`cargo run -q -p nodespace-core --example dump_core_schemas`.cwd(REPO).quiet().nothrow();
  if (dumped.exitCode !== 0) {
    console.error(`❌ Could not dump the core schemas:\n${dumped.stderr.toString()}`);
    process.exit(1);
  }
  const dump = JSON.parse(dumped.stdout.toString()) as CoreSchemaDump;
  const problems = compareNodeTypesDoc(dump, doc.markdown);
  if (problems.length > 0) {
    console.error(`❌ ${doc.source} and the core node types disagree (${problems.length}):`);
    for (const problem of problems) console.error(`  - ${problem}`);
    console.error(
      "\nA change to a core type updates its sheet in the same change (the node type sequence). Fix whichever side is wrong."
    );
    process.exit(1);
  }
  console.log(`✅ ${doc.source} matches the ${dump.types.length} core node types and their schemas.`);
}
