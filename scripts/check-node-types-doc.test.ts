// Covers the check that keeps the per-type reference equal to the core node
// types (ADR-086). The comparisons run against a small reference and a
// matching dump built here, so each test changes one side and names the
// disagreement it expects; nothing reads the real reference or compiles Rust.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { $ } from "bun";
import {
  compareNodeTypesDoc,
  DOCS_DIR_ENV_VAR,
  parseDocType,
  parseNodeTypesDoc,
  readDoc,
  resolveDocsDir,
  skipReason,
  type CoreSchemaDump,
  type DumpedField,
} from "./check-node-types-doc";

const DOC = `# Node Types

| Field | Type | Written by |
|---|---|---|
| \`id\` | text | client |

## 2. The type registry

| Kind | Types |
|---|---|
| Concrete core types | text, task, play |
| Abstract bases | ai-chat |
| Core subtypes | ai-chat-pty (ai-chat-acp is named, and is created later) |
| User-defined types | Created through the schema API |

## 3. Primitive types

| Type | Content | Derived |
|---|---|---|
| \`text\` | Markdown | — |

## 4. Flat types

### \`task\`

| Field | Storage | Type | Values | Required / default | Wire |
|---|---|---|---|---|---|
| \`status\` | \`task.status\` | enum, extensible (\`TaskStatus\`) | \`open\`, \`done\` + user values | required / \`open\` | \`status\` |
| \`due_date\`, \`started_at\` | \`task.*\` | date | — | — | \`dueDate\` |
| \`tags\` | \`task.tags\` | string[] | — | \`[]\` | \`tags\` |

- **Relationships:** \`blocks\` / \`blocked_by\`.

### \`ai-chat\` (abstract base) and subtypes

**\`ai-chat\` (abstract)**: never created directly.

| Field | Storage | Type |
|---|---|---|
| (title) | \`content\` | |
| \`agent\` | \`ai-chat.agent\` | string, required |

**\`ai-chat-pty\`**: an external agent in a terminal.

| Field | Storage | Type | Notes |
|---|---|---|---|
| \`session_status\` | \`ai-chat-pty.session_status\` | enum, closed (\`AiChatSessionStatus\`) | \`active\`, \`ended\`; default \`active\` |
| \`exit_code\` | \`ai-chat-pty.exit_code\` | number | |

**\`ai-chat-acp\`**: named, and created later.

## 5. Structured types

### \`play\`

| Field | Storage | Type | Default |
|---|---|---|---|
| \`rules\` | \`play.rules\` | \`RuleDefinition[]\` | \`[]\` |
| \`suspended_reason\` | \`play.suspended_reason\` | enum, closed: \`failed\`, \`drift\`; system | — |
`;

/** The 1-based line of the reference that contains \`snippet\`. */
const lineOf = (snippet: string): number => DOC.split("\n").findIndex((line) => line.includes(snippet)) + 1;

const field = (name: string, type: string, rest: Partial<DumpedField> = {}): DumpedField => ({ name, type, ...rest });
const values = (...all: string[]) => all.map((value) => ({ value }));

/** The dump the reference above describes exactly. */
function matchingDump(): CoreSchemaDump {
  return {
    types: [
      { id: "text", kind: "concrete" },
      { id: "task", kind: "concrete" },
      { id: "play", kind: "concrete" },
      { id: "ai-chat", kind: "abstract_base" },
      { id: "ai-chat-pty", kind: "core_subtype" },
    ],
    schemas: [
      { id: "text", fields: [] },
      {
        id: "task",
        fields: [
          field("status", "enum", { coreValues: values("open", "done"), extensible: true }),
          field("due_date", "date"),
          field("started_at", "date"),
          field("tags", "array", { itemType: "text" }),
        ],
      },
      { id: "ai-chat", fields: [field("agent", "text")] },
      {
        id: "ai-chat-pty",
        fields: [field("session_status", "enum", { coreValues: values("active", "ended") }), field("exit_code", "number")],
      },
      {
        id: "play",
        fields: [
          field("rules", "array", { itemType: "object" }),
          field("suspended_reason", "enum", { coreValues: values("failed", "drift") }),
        ],
      },
    ],
  };
}

/** The dump with one schema's fields rewritten. */
function withFields(type: string, change: (fields: DumpedField[]) => DumpedField[]): CoreSchemaDump {
  const dump = matchingDump();
  const schema = dump.schemas.find((candidate) => candidate.id === type);
  if (schema === undefined) throw new Error(`no ${type} schema in the fixture`);
  schema.fields = change(schema.fields);
  return dump;
}

describe("compareNodeTypesDoc — a matching reference", () => {
  test("reports nothing", () => {
    expect(compareNodeTypesDoc(matchingDump(), DOC)).toEqual([]);
  });

  test("a registered type with no seeded schema needs only a sheet", () => {
    const dump = matchingDump();
    dump.types.push({ id: "schema", kind: "concrete" });
    const doc = DOC.replace("text, task, play", "text, task, play, schema") + "\n### `schema` (meta-type)\n\nFlat properties.\n";
    expect(compareNodeTypesDoc(dump, doc)).toEqual([]);
  });
});

describe("compareNodeTypesDoc — a schema's fields change and the reference doesn't", () => {
  test("a field the schema gains", () => {
    const dump = withFields("task", (fields) => [...fields, field("estimate", "number")]);
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("### `task`")}: the \`task\` schema declares \`estimate\`, and its sheet's field table doesn't list it`,
    ]);
  });

  test("a field the schema loses", () => {
    const dump = withFields("task", (fields) => fields.filter((f) => f.name !== "started_at"));
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("`due_date`, `started_at`")}: the \`task\` sheet lists \`started_at\`, and the schema declares no such field`,
    ]);
  });

  test("a field the schema renames is one gained and one lost", () => {
    const dump = withFields("task", (fields) => fields.map((f) => (f.name === "due_date" ? { ...f, name: "due_on" } : f)));
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("### `task`")}: the \`task\` schema declares \`due_on\`, and its sheet's field table doesn't list it`,
      `line ${lineOf("`due_date`, `started_at`")}: the \`task\` sheet lists \`due_date\`, and the schema declares no such field`,
    ]);
  });

  test("a primitive type that gains a field has no table to list it in", () => {
    const dump = withFields("text", () => [field("language", "text")]);
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| `text` |")}: the \`text\` schema declares \`language\`, and its sheet's field table doesn't list it`,
    ]);
  });

  test("a field's type", () => {
    const dump = withFields("ai-chat-pty", (fields) => fields.map((f) => (f.name === "exit_code" ? { ...f, type: "text" } : f)));
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| `exit_code`")}: \`ai-chat-pty.exit_code\` is text in the schema and number in the sheet`,
    ]);
  });

  test("an array's element type", () => {
    const dump = withFields("task", (fields) => fields.map((f) => (f.name === "tags" ? { ...f, itemType: "number" } : f)));
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| `tags`")}: \`task.tags\` is array of number in the schema and array of text in the sheet`,
    ]);
  });

  test("an enum value added, and the values' order", () => {
    const added = withFields("task", (fields) =>
      fields.map((f) => (f.name === "status" ? { ...f, coreValues: values("open", "blocked", "done") } : f))
    );
    expect(compareNodeTypesDoc(added, DOC)).toEqual([
      `line ${lineOf("| `status`")}: \`task.status\` has the values open, blocked, done, and the sheet lists open, done`,
    ]);
    const reordered = withFields("ai-chat-pty", (fields) =>
      fields.map((f) => (f.name === "session_status" ? { ...f, coreValues: values("ended", "active") } : f))
    );
    expect(compareNodeTypesDoc(reordered, DOC)).toEqual([
      `line ${lineOf("| `session_status`")}: \`ai-chat-pty.session_status\` has the values ended, active, and the sheet lists active, ended`,
    ]);
  });

  test("an enum that stops being extensible, or starts", () => {
    const closed = withFields("task", (fields) => fields.map((f) => (f.name === "status" ? { ...f, extensible: false } : f)));
    expect(compareNodeTypesDoc(closed, DOC)).toEqual([
      `line ${lineOf("| `status`")}: \`task.status\` is closed in the schema and extensible in the sheet`,
    ]);
    const opened = withFields("ai-chat-pty", (fields) =>
      fields.map((f) => (f.name === "session_status" ? { ...f, extensible: true } : f))
    );
    expect(compareNodeTypesDoc(opened, DOC)).toEqual([
      `line ${lineOf("| `session_status`")}: \`ai-chat-pty.session_status\` is extensible in the schema and closed in the sheet`,
    ]);
  });
});

describe("compareNodeTypesDoc — the reference is wrong", () => {
  test("a storage key in another type's bucket", () => {
    const doc = DOC.replace("`ai-chat-pty.exit_code`", "`ai-chat.exit_code`");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toEqual([
      `line ${lineOf("| `exit_code`")}: \`ai-chat-pty.exit_code\` is stored at \`ai-chat-pty.exit_code\`, and the sheet says \`ai-chat.exit_code\``,
    ]);
  });

  test("an enum with no values listed", () => {
    const doc = DOC.replace("enum, closed: `failed`, `drift`; system", "enum, closed; system");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toEqual([
      `line ${lineOf("| `suspended_reason`")}: \`play.suspended_reason\` is an enum, and the sheet lists no values for it (the schema's: failed, drift)`,
    ]);
  });

  test("an enum row that says neither extensible nor closed", () => {
    const doc = DOC.replace("enum, closed: `failed`, `drift`; system", "enum `failed`, `drift`; system");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toEqual([
      `line ${lineOf("| `suspended_reason`")}: \`play.suspended_reason\` is an enum, and the sheet's Type cell says neither \`extensible\` nor \`closed\``,
    ]);
  });

  test("a field table on the sheet of a type the code seeds no schema for", () => {
    const dump = matchingDump();
    dump.schemas = dump.schemas.filter((schema) => schema.id !== "ai-chat");
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| `agent`")}: the \`ai-chat\` sheet lists \`agent\`, and the code seeds no \`ai-chat\` schema`,
    ]);
  });

  test("a field listed twice", () => {
    const doc = DOC.replace("| `tags` |", "| `status` | `task.status` | enum, extensible | `open`, `done` | — | — |\n| `tags` |");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toEqual([`line ${lineOf("| `tags`")}: \`task\` lists the field \`status\` twice`]);
  });

  test("rows the parser can't read are reported, not skipped", () => {
    const doc = DOC.replace("| `exit_code` | `ai-chat-pty.exit_code` | number |", "| `exit_code` | `ai-chat-pty.exit_code` | integer |")
      .replace("| `due_date`, `started_at` | `task.*` |", "| `due_date`, `started_at` | `task.due_date` |")
      .replace("| `rules` | `play.rules` |", "| rules | `play.rules` |");
    const problems = compareNodeTypesDoc(matchingDump(), doc);
    expect(problems).toContain(`line ${lineOf("| `exit_code`")}: the Type cell "integer" opens with no known type`);
    expect(problems).toContain(`line ${lineOf("`due_date`, `started_at`")}: a row for 2 fields needs a \`<type>.*\` storage key, not \`task.due_date\``);
    expect(problems).toContain(`line ${lineOf("`play.rules`")}: the Field cell names no field in backticks`);
  });

  test("a field table outside any sheet", () => {
    const doc = DOC.replace("### `play`\n", "### Play\n");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toContain(`line ${lineOf("| Field | Storage | Type | Default |")}: a field table that is not under a type's sheet`);
  });
});

describe("compareNodeTypesDoc — the type list", () => {
  test("a core type with no sheet", () => {
    const dump = matchingDump();
    dump.types.push({ id: "person", kind: "concrete" });
    dump.schemas.push({ id: "person", fields: [field("email", "text")] });
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| Concrete core types")}: \`person\` is registered as one of the concrete core types, and the registry table doesn't list it there`,
      "`person` is a core type with no sheet",
    ]);
  });

  test("a type listed under the wrong kind", () => {
    const dump = matchingDump();
    dump.types = dump.types.map((type) => (type.id === "ai-chat" ? { ...type, kind: "concrete" } : type));
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| Concrete core types")}: \`ai-chat\` is registered as one of the concrete core types, and the registry table doesn't list it there`,
      `line ${lineOf("| Abstract bases")}: the registry table lists \`ai-chat\` under "Abstract bases", and the code registers no such type of that kind`,
    ]);
  });

  test("a documented type the code doesn't register", () => {
    const dump = matchingDump();
    dump.types = dump.types.filter((type) => type.id !== "ai-chat-pty");
    dump.schemas = dump.schemas.filter((schema) => schema.id !== "ai-chat-pty");
    expect(compareNodeTypesDoc(dump, DOC)).toEqual([
      `line ${lineOf("| Core subtypes")}: the registry table lists \`ai-chat-pty\` under "Core subtypes", and the code registers no such type of that kind`,
      `line ${lineOf("**`ai-chat-pty`**")}: \`ai-chat-pty\` has a field table, and the code registers no such core type`,
    ]);
  });

  test("a seeded schema the registry doesn't list", () => {
    const dump = matchingDump();
    dump.schemas.push({ id: "invoice", fields: [] });
    expect(compareNodeTypesDoc(dump, DOC)).toEqual(["the code seeds a `invoice` schema for a type the registry doesn't list"]);
  });

  test("a type named only in a parenthesis or a table-less sheet is not a registered type", () => {
    const doc = parseNodeTypesDoc(DOC);
    expect(doc.registry.core_subtype?.types).toEqual(["ai-chat-pty"]);
    expect(doc.sheets.get("ai-chat-acp")?.fields).toEqual([]);
  });

  test("a missing kind row", () => {
    const doc = DOC.replace("| Abstract bases | ai-chat |\n", "");
    expect(compareNodeTypesDoc(matchingDump(), doc)).toContain('the registry table has no "Abstract bases" row');
  });
});

describe("parseDocType", () => {
  test("reads the type a cell opens with", () => {
    expect(parseDocType("string, optional")).toEqual({ type: "text" });
    expect(parseDocType("string (`*` = all)")).toEqual({ type: "text" });
    expect(parseDocType("number ≥ 1")).toEqual({ type: "number" });
    expect(parseDocType("boolean, synced; the user's switch")).toEqual({ type: "boolean" });
    expect(parseDocType("date; start ≤ end")).toEqual({ type: "date" });
    expect(parseDocType("datetime, system")).toEqual({ type: "datetime" });
    expect(parseDocType("object (JSON Schema): nesting depth ≤ 9")).toEqual({ type: "object" });
    expect(parseDocType("string[]")).toEqual({ type: "array", itemType: "text" });
    expect(parseDocType("`QueryFilter[]`")).toEqual({ type: "array", itemType: "object" });
  });

  test("refuses a word outside the vocabulary", () => {
    expect(parseDocType("text")).toBeNull();
    expect(parseDocType("dates")).toBeNull();
    expect(parseDocType("")).toBeNull();
  });
});

describe("resolveDocsDir", () => {
  let root: string;

  beforeEach(() => {
    root = realpathSync(mkdtempSync(join(tmpdir(), "check-node-types-doc-test-")));
  });

  afterEach(() => {
    rmSync(root, { recursive: true, force: true });
  });

  /** A primary checkout at `<root>/core` with a worktree where the merge gate keeps its own. */
  async function checkoutWithGateWorktree(): Promise<{ primary: string; gate: string }> {
    const primary = join(root, "core");
    const gate = join(primary, ".claude", "worktrees", "_gate");
    mkdirSync(primary);
    await $`git init -q -b main`.cwd(primary);
    writeFileSync(join(primary, "README.md"), "fixture\n");
    await $`git add README.md`.cwd(primary);
    await $`git -c user.name=test -c user.email=test@example.com commit -q -m init`.cwd(primary);
    await $`git worktree add -q --detach ${gate}`.cwd(primary).quiet();
    return { primary, gate };
  }

  test("finds the primary checkout's sibling from the primary checkout", async () => {
    const { primary } = await checkoutWithGateWorktree();
    expect(await resolveDocsDir(primary, {})).toEqual({ dir: join(root, "nodespace-docs"), explicit: false });
  });

  test("finds the same sibling from inside the merge gate's worktree", async () => {
    const { gate } = await checkoutWithGateWorktree();
    expect(await resolveDocsDir(gate, {})).toEqual({ dir: join(root, "nodespace-docs"), explicit: false });
    // From a subdirectory of the worktree too: the check runs from wherever the script lives.
    mkdirSync(join(gate, "scripts"));
    expect(await resolveDocsDir(join(gate, "scripts"), {})).toEqual({ dir: join(root, "nodespace-docs"), explicit: false });
  });

  test("the environment variable names the docs repository, absolute or relative to the checkout", async () => {
    const { gate } = await checkoutWithGateWorktree();
    expect(await resolveDocsDir(gate, { [DOCS_DIR_ENV_VAR]: "/elsewhere/docs" })).toEqual({
      dir: "/elsewhere/docs",
      explicit: true,
    });
    expect(await resolveDocsDir(gate, { [DOCS_DIR_ENV_VAR]: "../docs" })).toEqual({
      dir: join(gate, "..", "docs"),
      explicit: true,
    });
  });

  test("only a machine with no docs checkout skips; a named directory is never skipped", () => {
    const dir = join(root, "nodespace-docs");
    expect(skipReason({ dir, explicit: false })).toContain(`no docs checkout at ${dir}`);
    expect(skipReason({ dir, explicit: true })).toBeNull();
    mkdirSync(dir);
    expect(skipReason({ dir, explicit: false })).toBeNull();
  });
});

describe("readDoc", () => {
  let root: string;
  const DOC_PATH = join("components", "node-types.md");
  const git = (cwd: string, ...args: string[]) =>
    $`git -c user.name=test -c user.email=test@example.com ${args}`.cwd(cwd).quiet();

  beforeEach(() => {
    root = realpathSync(mkdtempSync(join(tmpdir(), "check-node-types-doc-test-")));
  });

  afterEach(() => {
    rmSync(root, { recursive: true, force: true });
  });

  /** Commits `text` as the reference in `checkout` and pushes it to the remote's main. */
  async function publish(checkout: string, text: string): Promise<void> {
    mkdirSync(join(checkout, "components"), { recursive: true });
    writeFileSync(join(checkout, DOC_PATH), text);
    await git(checkout, "add", "-A");
    await git(checkout, "commit", "-q", "-m", "reference");
    await git(checkout, "push", "-q", "origin", "HEAD:main");
  }

  /** A docs remote, and a clone of it at `<root>/<name>`. */
  async function clone(name: string): Promise<string> {
    const remote = join(root, "remote.git");
    await $`git init -q --bare -b main ${remote}`.quiet();
    const checkout = join(root, name);
    await $`git clone -q ${remote} ${checkout}`.quiet();
    return checkout;
  }

  test("a machine with no docs checkout skips, whichever form is asked for", async () => {
    const docs = { dir: join(root, "nodespace-docs"), explicit: false };
    expect(await readDoc(docs, false)).toEqual({ skip: expect.stringContaining("no docs checkout") });
    expect(await readDoc(docs, true)).toEqual({ skip: expect.stringContaining("no docs checkout") });
  });

  test("a docs directory with no reference in it is an error, not a skip", async () => {
    const dir = join(root, "nodespace-docs");
    mkdirSync(dir);
    expect(await readDoc({ dir, explicit: false }, false)).toEqual({ error: `${join(dir, DOC_PATH)} does not exist.` });
    const named = { dir: join(root, "nowhere"), explicit: true };
    const missing = { error: `${DOCS_DIR_ENV_VAR} is set, and ${named.dir} does not exist.` };
    expect(await readDoc(named, false)).toEqual(missing);
    expect(await readDoc(named, true)).toEqual(missing);
    // A checkout whose main has no reference.
    const docs = await clone("empty-docs");
    writeFileSync(join(docs, "README.md"), "docs\n");
    await git(docs, "add", "-A");
    await git(docs, "commit", "-q", "-m", "no reference");
    await git(docs, "push", "-q", "origin", "HEAD:main");
    expect(await readDoc({ dir: docs, explicit: false }, true)).toEqual({ error: expect.stringContaining("could not read") });
  });

  test("a plain run reads the working tree, uncommitted edits included", async () => {
    const docs = await clone("nodespace-docs");
    await publish(docs, "published\n");
    writeFileSync(join(docs, DOC_PATH), "being edited\n");
    expect(await readDoc({ dir: docs, explicit: false }, false)).toEqual({
      markdown: "being edited\n",
      source: join(docs, DOC_PATH),
    });
  });

  test("the published form reads the remote's main: not uncommitted edits, not an unpushed commit", async () => {
    const docs = await clone("nodespace-docs");
    await publish(docs, "published\n");
    writeFileSync(join(docs, DOC_PATH), "committed here, not pushed\n");
    await git(docs, "commit", "-q", "-am", "local only");
    writeFileSync(join(docs, DOC_PATH), "being edited\n");

    const read = await readDoc({ dir: docs, explicit: false }, true);
    expect(read).toEqual({ markdown: "published\n", source: expect.stringContaining("at origin/main (") });
  });

  test("the published form fetches, so a checkout behind its remote reads what was pushed since", async () => {
    const docs = await clone("nodespace-docs");
    await publish(docs, "first\n");
    const elsewhere = join(root, "elsewhere");
    await $`git clone -q ${join(root, "remote.git")} ${elsewhere}`.quiet();
    await publish(elsewhere, "pushed from another machine\n");

    expect(await readDoc({ dir: docs, explicit: false }, true)).toEqual({
      markdown: "pushed from another machine\n",
      source: expect.stringContaining("at origin/main ("),
    });
  });

  test("when the fetch fails, the published form reads main as last fetched and says so", async () => {
    const docs = await clone("nodespace-docs");
    await publish(docs, "first\n");
    await git(docs, "remote", "set-url", "origin", join(root, "gone.git"));

    expect(await readDoc({ dir: docs, explicit: false }, true)).toEqual({
      markdown: "first\n",
      source: expect.stringContaining("as last fetched: the fetch failed"),
    });
  });
});
