//! Writes the frontend's generated TypeScript (ADR-086 §8): one file per wire
//! type, the core type registry, and the typed core field table.
//!
//! `gen-ts <dir>` empties `<dir>` of `.ts` files and writes the current set.
//! `bun run gen:types` (`scripts/gen-types.ts`) runs it, formats the output
//! and writes it to `packages/desktop-app/src/lib/types/generated/`; the merge
//! gate runs the same script with `--check` and fails on any difference.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs, process};

use nodespace_types::*;
use serde::Serialize;
use ts_rs::{Config, TS};

const HEADER: &str =
    "// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.\n";

/// One generated file: its name in the output directory and its contents.
struct File {
    name: String,
    contents: String,
}

/// A generated type: its TypeScript name and the file that declares it.
struct Declared {
    name: String,
    file: File,
}

/// `AiChatPtyNode` to `ai-chat-pty-node`: the frontend's file naming.
fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn declare<T: TS + 'static>(cfg: &Config) -> Declared {
    let name = T::ident(cfg);
    let imports: BTreeSet<String> = T::dependencies(cfg)
        .into_iter()
        .map(|dep| dep.ts_name)
        .filter(|dep| *dep != name)
        .collect();

    let mut contents = String::from(HEADER);
    for import in &imports {
        writeln!(
            contents,
            "import type {{ {import} }} from './{}';",
            kebab(import)
        )
        .unwrap();
    }
    contents.push('\n');
    if let Some(docs) = T::docs() {
        contents.push_str(&docs);
    }
    writeln!(contents, "export {}", T::decl(cfg)).unwrap();

    Declared {
        file: File {
            name: format!("{}.ts", kebab(&name)),
            contents,
        },
        name,
    }
}

macro_rules! declare_all {
    ($cfg:expr, $($t:ty),* $(,)?) => { vec![$(declare::<$t>($cfg)),*] };
}

/// Every wire type, plus the shapes of the two generated registries.
fn declarations(cfg: &Config) -> Vec<Declared> {
    declare_all!(
        cfg,
        // node
        NodeReference,
        NodeEnvelope,
        OrderBy,
        NodeQuery,
        NodeUpdate,
        DeleteResult,
        // task
        TaskStatus,
        Priority,
        TaskNode,
        TaskNodeUpdate,
        // person, project
        PersonNode,
        PersonNodeUpdate,
        ProjectNode,
        ProjectNodeUpdate,
        ProjectStatus,
        // query
        FilterType,
        FilterOperator,
        RelationshipHop,
        RelationshipPath,
        SortDirection,
        QueryFilter,
        SortConfig,
        QueryGeneratedBy,
        QueryFields,
        QueryNode,
        QueryNodeUpdate,
        // play
        RuleClass,
        GraphEventType,
        ActionType,
        InlineSelector,
        SavedQuerySelector,
        Selector,
        Trigger,
        CreateNodeParams,
        UpdateNodeParams,
        AddRelationshipParams,
        RemoveRelationshipParams,
        RejectParams,
        Action,
        RuleDefinition,
        PlayFields,
        PlayNode,
        PlayNodeUpdate,
        // schema
        EnumValue,
        SchemaProtectionLevel,
        SchemaFieldType,
        SchemaField,
        EdgeField,
        RelationshipDirection,
        RelationshipCardinality,
        SchemaRelationship,
        SchemaChildrenRule,
        SchemaParentRule,
        SchemaNode,
        // ai-chat
        AiChatCompletedWrite,
        AiChatResolvedEntity,
        AiChatPendingDeletion,
        AiChatTurnOutcome,
        AiChatMessageRole,
        AiChatMessage,
        AiChatProvider,
        AiChatTurnStatus,
        AiChatSessionStatus,
        AiChatBase,
        AiChatNativeNode,
        AiChatPtyNode,
        // registry
        CoreNodeType,
        CoreTypeKind,
        TypeCategory,
        ContentRole,
        IncompatibleDatabase,
        // generated registries
        StructuralRules,
        CoreTypeEntry,
        StructuredShape,
        TypedCoreField,
    )
}

/// A type's two structural rules (ADR-089).
#[derive(Serialize, TS)]
struct StructuralRules {
    /// Which children the type's nodes may have.
    children: SchemaChildrenRule,
    /// Where the type's nodes may sit in the `has_child` tree.
    parent: SchemaParentRule,
}

/// A core type's registry entry, as the frontend reads it.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
struct CoreTypeEntry {
    /// The stored `node_type`.
    id: CoreNodeType,
    /// The core type this one `extends`.
    parent: Option<CoreNodeType>,
    /// Whether nodes of exactly this type may exist.
    #[serde(rename = "abstract")]
    is_abstract: bool,
    /// Offered by the `@` mention picker (the effective rule: it narrows down the chain).
    mentionable: bool,
    /// Whether the type's typed fields are written through a typed update
    /// command. A type without one writes them as a `properties` patch keyed
    /// by storage name.
    typed_update: bool,
    /// The structural rules the type itself declares, before inheritance.
    structure: StructuralRules,
}

/// The JSON shape of a typed core field that is not a string.
#[derive(Serialize, TS)]
#[serde(rename_all = "lowercase")]
enum StructuredShape {
    Array,
    Number,
    Object,
}

/// One core field the backend moves out of `properties` to a top-level typed
/// field: `due_date` is stored in the `task` bucket and travels as `dueDate`.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
struct TypedCoreField {
    /// Schema field name, as stored in the type's bucket (`due_date`).
    storage: &'static str,
    /// Top-level key on the typed node (`dueDate`).
    wire: &'static str,
    /// Dates are normalized to `YYYY-MM-DD` on read.
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<bool>,
    /// Not a string: promoted as stored when it has this JSON shape, dropped
    /// otherwise, as the Rust decoder drops a malformed field to its default.
    #[serde(skip_serializing_if = "Option::is_none")]
    structured: Option<StructuredShape>,
    /// System-managed: read on the wire, never sent in a typed update.
    #[serde(skip_serializing_if = "Option::is_none")]
    read_only: Option<bool>,
}

impl From<&PromotedField> for TypedCoreField {
    fn from(field: &PromotedField) -> Self {
        Self {
            storage: field.storage,
            wire: field.wire,
            date: (field.shape == PromotedShape::Date).then_some(true),
            structured: match field.shape {
                PromotedShape::Text | PromotedShape::Date => None,
                PromotedShape::Number => Some(StructuredShape::Number),
                PromotedShape::Array => Some(StructuredShape::Array),
                PromotedShape::Object => Some(StructuredShape::Object),
            },
            read_only: field.read_only.then_some(true),
        }
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("registry data always serializes")
}

/// `CORE_NODE_TYPES`: the registry, in `CoreNodeType::ALL` order.
fn core_node_types_file() -> File {
    let mut contents = String::from(HEADER);
    contents.push_str("import type { CoreTypeEntry } from './core-type-entry';\n\n");
    contents.push_str("/** Every core type, in registry order. */\n");
    contents.push_str("export const CORE_NODE_TYPES = [\n");
    for core in CoreNodeType::ALL {
        let entry = CoreTypeEntry {
            id: core,
            parent: core.parent(),
            is_abstract: core.is_abstract(),
            mentionable: core.participation().mentionable,
            typed_update: core.wire() == (WireShape::Typed { update: true }),
            structure: StructuralRules {
                children: core.declared_structure().children.into(),
                parent: core.declared_structure().parent.into(),
            },
        };
        writeln!(contents, "  {},", json(&entry)).unwrap();
    }
    contents.push_str("] as const satisfies readonly CoreTypeEntry[];\n");
    File {
        name: "core-node-types.ts".to_string(),
        contents,
    }
}

/// The values the typed conversion fills when a stored node has none: what a
/// node of `core` with no properties carries in its promoted fields.
fn promoted_defaults(core: CoreNodeType) -> BTreeMap<&'static str, serde_json::Value> {
    let node = Node::new(core.to_string(), String::new(), serde_json::json!({}));
    let typed = node_to_typed_value(node).expect("an empty core node always converts");
    core_promoted_fields(core)
        .iter()
        .filter_map(|field| {
            let value = typed.get(field.wire).filter(|v| !v.is_null())?;
            Some((field.wire, value.clone()))
        })
        .collect()
}

/// `TYPED_CORE_FIELDS` and `TYPED_CORE_DEFAULTS`, from the promoted fields.
fn typed_core_fields_file() -> File {
    let mut fields = String::new();
    let mut defaults = String::new();
    for core in CoreNodeType::ALL {
        let promoted = core_promoted_fields(core);
        if promoted.is_empty() {
            continue;
        }
        writeln!(fields, "  {}: [", json(&core)).unwrap();
        for field in promoted {
            writeln!(fields, "    {},", json(&TypedCoreField::from(field))).unwrap();
        }
        fields.push_str("  ],\n");

        let filled = promoted_defaults(core);
        if !filled.is_empty() {
            writeln!(defaults, "  {}: {},", json(&core), json(&filled)).unwrap();
        }
    }

    let mut contents = String::from(HEADER);
    contents.push_str("import type { TypedCoreField } from './typed-core-field';\n\n");
    contents.push_str(
        "/** The typed core fields of each core type that has any, in schema order. */\n",
    );
    contents.push_str(
        "export const TYPED_CORE_FIELDS: Readonly<Record<string, readonly TypedCoreField[]>> = {\n",
    );
    contents.push_str(&fields);
    contents.push_str("};\n\n");
    contents.push_str(
        "/**\n * The values the backend fills when a stored node has none, by typed key.\n \
         * Consumers copy before use, since a default can be an array.\n */\n",
    );
    contents.push_str(
        "export const TYPED_CORE_DEFAULTS: Readonly<Record<string, Readonly<Record<string, unknown>>>> = {\n",
    );
    contents.push_str(&defaults);
    contents.push_str("};\n");
    File {
        name: "typed-core-fields.ts".to_string(),
        contents,
    }
}

/// The barrel: every generated type and both registries.
fn index_file(declared: &[Declared]) -> File {
    let mut contents = String::from(HEADER);
    let mut names: Vec<&str> = declared.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    for name in names {
        writeln!(
            contents,
            "export type {{ {name} }} from './{}';",
            kebab(name)
        )
        .unwrap();
    }
    contents.push_str("export { CORE_NODE_TYPES } from './core-node-types';\n");
    contents.push_str(
        "export { TYPED_CORE_DEFAULTS, TYPED_CORE_FIELDS } from './typed-core-fields';\n",
    );
    File {
        name: "index.ts".to_string(),
        contents,
    }
}

/// `TaskStatus`, `ProjectStatus`, `Priority` and `SchemaFieldType` serialize by
/// hand, while their TypeScript unions are derived from the variant names.
/// This holds the two together: every value serde writes for a named variant
/// must be a literal of the generated union.
fn check_hand_serialized_enums(cfg: &Config) -> Result<(), String> {
    fn check<T: TS + Serialize>(cfg: &Config, values: &[T]) -> Vec<String> {
        let decl = T::decl(cfg);
        values
            .iter()
            .map(json)
            .filter(|literal| !decl.contains(literal.as_str()))
            .map(|literal| {
                format!(
                    "{} serializes {literal}, missing from `{decl}`",
                    T::ident(cfg)
                )
            })
            .collect()
    }

    let mut problems = check(cfg, &TaskStatus::CORE);
    problems.extend(check(cfg, &ProjectStatus::CORE));
    problems.extend(check(
        cfg,
        &[
            Priority::Highest,
            Priority::High,
            Priority::Medium,
            Priority::Low,
            Priority::Lowest,
        ],
    ));
    problems.extend(check(cfg, &SchemaFieldType::ALL));
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn main() {
    let Some(out_dir) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: gen-ts <output directory>");
        process::exit(2);
    };

    // JavaScript numbers carry every integer the wire types use.
    let cfg = Config::new().with_large_int("number");
    let declared = declarations(&cfg);
    if let Err(problems) = check_hand_serialized_enums(&cfg) {
        eprintln!("A generated union does not match what its enum serializes:\n{problems}");
        process::exit(1);
    }

    let registries = [
        core_node_types_file(),
        typed_core_fields_file(),
        index_file(&declared),
    ];

    fs::create_dir_all(&out_dir).expect("output directory can be created");
    for entry in fs::read_dir(&out_dir).expect("output directory is readable") {
        let path = entry.expect("directory entry is readable").path();
        if path.extension().is_some_and(|ext| ext == "ts") {
            fs::remove_file(&path).expect("stale generated file can be removed");
        }
    }
    for file in declared.iter().map(|d| &d.file).chain(&registries) {
        fs::write(out_dir.join(&file.name), &file.contents).expect("generated file can be written");
    }
}
