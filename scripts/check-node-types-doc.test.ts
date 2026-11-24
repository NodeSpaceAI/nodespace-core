// Covers the check that keeps the committed node types reference equal to the
// core node types the code ships (ADR-086). The sheet is generated from a dump
// built here, so each test changes one side and names the disagreement it
// expects; nothing compiles Rust.
import { describe, expect, test } from "bun:test";
import {
  compareNodeTypesSheet,
  renderNodeTypesSheet,
  SHEET_PATH,
  unregisteredSchemas,
  type CoreSchemaDump,
  type DumpedField,
} from "./check-node-types-doc";

const field = (name: string, type: string, rest: Partial<DumpedField> = {}): DumpedField => ({ name, type, ...rest });
const values = (...all: string[]) => all.map((value) => ({ value }));

function dump(): CoreSchemaDump {
  return {
    types: [
      { id: "text", kind: "concrete" },
      { id: "task", kind: "concrete" },
      { id: "schema", kind: "concrete" },
      { id: "ai-chat", kind: "abstract_base" },
      { id: "ai-chat-pty", kind: "core_subtype" },
    ],
    schemas: [
      { id: "text", fields: [] },
      {
        id: "task",
        fields: [
          field("status", "enum", { coreValues: values("open", "done"), extensible: true }),
          field("tags", "array", { itemType: "text" }),
        ],
      },
      { id: "ai-chat", fields: [field("agent", "text")] },
      { id: "ai-chat-pty", fields: [field("session_status", "enum", { coreValues: values("active", "ended") })] },
    ],
  };
}

const sheet = (d: CoreSchemaDump = dump()) => renderNodeTypesSheet(d);

describe("renderNodeTypesSheet", () => {
  test("lists every type under its kind in registry order", () => {
    const text = sheet();
    expect(text).toContain("| Concrete core types | text, task, schema |");
    expect(text).toContain("| Abstract bases | ai-chat |");
    expect(text).toContain("| Core subtypes | ai-chat-pty |");
  });

  test("gives each field its storage key, type and enum values in order", () => {
    const text = sheet();
    expect(text).toContain("| `status` | `task.status` | enum, extensible | `open`, `done` |");
    expect(text).toContain("| `tags` | `task.tags` | array of text | |");
    expect(text).toContain("| `session_status` | `ai-chat-pty.session_status` | enum, closed | `active`, `ended` |");
  });

  test("a type with no seeded schema says so", () => {
    expect(sheet()).toMatch(/### `schema`\n\nNo seeded schema\./);
  });

  test("is deterministic and ends with a newline", () => {
    expect(sheet()).toBe(sheet());
    expect(sheet().endsWith("\n")).toBe(true);
  });

  test("the sheet lives inside the core package", () => {
    expect(SHEET_PATH).toBe("packages/core/shape-record/node-types.md");
  });
});

describe("compareNodeTypesSheet", () => {
  test("a matching sheet reports nothing", () => {
    expect(compareNodeTypesSheet(sheet(), sheet())).toEqual([]);
  });

  test("a new core type is reported", () => {
    const d = dump();
    d.types.push({ id: "play", kind: "concrete" });
    const problems = compareNodeTypesSheet(sheet(), sheet(d));
    expect(problems.some((p) => p.includes("play"))).toBe(true);
  });

  test("a removed core type is reported", () => {
    const d = dump();
    d.types = d.types.filter((t) => t.id !== "task");
    d.schemas = d.schemas.filter((s) => s.id !== "task");
    expect(compareNodeTypesSheet(sheet(), sheet(d)).length).toBeGreaterThan(0);
  });

  test("a type that changes kind is reported", () => {
    const d = dump();
    d.types[1] = { id: "task", kind: "core_subtype" };
    expect(compareNodeTypesSheet(sheet(), sheet(d)).length).toBeGreaterThan(0);
  });

  test("a changed field type is reported", () => {
    const d = dump();
    d.schemas[1].fields[1] = field("tags", "text");
    const problems = compareNodeTypesSheet(sheet(), sheet(d));
    expect(problems.join("\n")).toContain("task.tags");
  });

  test("a reordered enum is reported", () => {
    const d = dump();
    d.schemas[1].fields[0] = field("status", "enum", { coreValues: values("done", "open"), extensible: true });
    expect(compareNodeTypesSheet(sheet(), sheet(d)).length).toBeGreaterThan(0);
  });

  test("an enum going from closed to extensible is reported", () => {
    const d = dump();
    d.schemas[3].fields[0].extensible = true;
    expect(compareNodeTypesSheet(sheet(), sheet(d)).length).toBeGreaterThan(0);
  });

  test("an added field is reported", () => {
    const d = dump();
    d.schemas[0].fields.push(field("extra", "number"));
    expect(compareNodeTypesSheet(sheet(), sheet(d)).join("\n")).toContain("text.extra");
  });

  test("a hand edit of the committed sheet is reported", () => {
    expect(compareNodeTypesSheet(sheet().replace("`task.tags`", "`task.labels`"), sheet()).length).toBeGreaterThan(0);
  });

  test("a line only the committed sheet has is reported as no longer generated", () => {
    const problems = compareNodeTypesSheet(`${sheet()}| \`ghost\` | \`task.ghost\` | text | |\n`, sheet());
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("task.ghost");
    expect(problems[0]).toContain("no longer generates");
  });

  test("a missing committed sheet is reported", () => {
    expect(compareNodeTypesSheet(null, sheet())).toEqual([`${SHEET_PATH} does not exist.`]);
  });
});

describe("unregisteredSchemas", () => {
  test("a dump whose schemas are all registered reports nothing", () => {
    expect(unregisteredSchemas(dump())).toEqual([]);
  });

  test("a seeded schema for a type the registry does not list is reported", () => {
    const d = dump();
    d.schemas.push({ id: "ghost", fields: [] });
    const problems = unregisteredSchemas(d);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("`ghost`");
    expect(problems[0]).toContain("registry doesn't list");
  });
});
