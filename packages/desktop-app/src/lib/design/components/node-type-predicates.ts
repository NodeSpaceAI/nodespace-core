/**
 * Node type predicates
 *
 * Answers what the UI actually needs to know about a node type: **which frontend
 * integration does it have?** Every predicate here delegates to the plugin registry, so
 * they stay correct as types are registered and unregistered at runtime.
 *
 * This deliberately replaces an older hand-maintained "core node types" list that was
 * used as a proxy for "has a dedicated frontend integration". The two are unrelated:
 * `project` is a core type with no plugin registration at all, while `person`,
 * `document`, `user` and `ai-chat` are registered plugins that were never in the list.
 * Ask the registry, not a list.
 */

import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { resolveTitleOrContent } from '$lib/utils/node-display-title';

/**
 * True when a plugin registers an inline node component for this type.
 *
 * Types with one (text, task, header, query, …) are edited in place in the outline.
 * Types without one render through the BaseNode fallback as a read-only entity row.
 */
export function hasInlineNodeComponent(nodeType: string): boolean {
  return pluginRegistry.hasNodeComponent(nodeType);
}

/**
 * True when this type renders in the outline as a read-only entity row rather than an
 * inline-editable node — i.e. no plugin registered an inline node component for it.
 *
 * Entity rows get an "open in other pane" affordance, are skipped by arrow navigation
 * (there is nothing to put a caret into), and open directly rather than resolving to a
 * parent viewer.
 */
export function rendersAsEntityRow(nodeType: string): boolean {
  return !hasInlineNodeComponent(nodeType);
}

/**
 * True when nodeType's plugin `name` is a legitimate user-facing entity noun (e.g. "Person"),
 * rather than an internal label chosen for the plugin registry / slash-command menu (e.g.
 * "Text Node", "Header Node").
 *
 * This is a DIFFERENT question from `rendersAsEntityRow`, and deliberately not derived from
 * it. Every entity row already has an entity-noun name by construction (nothing that renders
 * only as a read-only row names itself for a slash-command menu), but the converse doesn't
 * hold: `person` has both an inline node component (so `rendersAsEntityRow('person')` is
 * false) and an entity-noun name ("Person"). A gate that conflates the two — e.g. asking only
 * `rendersAsEntityRow` before using a plugin's name as an untitled node's display name — wrongly
 * excludes person and would exclude any future type with the same combination. Ask this
 * directly instead. See `PluginDefinition.entityNoun` (types.ts) for how a plugin opts in.
 */
export function hasEntityNounName(nodeType: string): boolean {
  return pluginRegistry.hasEntityNounName(nodeType);
}

/**
 * True when this type needs the generic, schema-driven properties form — no plugin
 * registered a hardcoded, type-specific schema form for it.
 *
 * `task` and `person` have hardcoded forms; `project` and every user-defined type fall
 * back to the generic one.
 */
export function needsGenericSchemaForm(nodeType: string): boolean {
  return !pluginRegistry.hasSchemaForm(nodeType);
}

/**
 * The value a viewer header shows while it is NOT focused.
 *
 * Delegates the title-vs-content decision to `resolveTitleOrContent` (see that function for
 * the full rule and why it exists) and additionally strips markdown header syntax from a
 * content-sourced value (`## Foo` → `Foo`), for header nodes whose content carries the `#`
 * markers. A template-computed title is built from property values and has no such syntax
 * to strip, so the stripping only applies on the `content` branch.
 */
export function computeHeaderDisplayValue(
  node: { title?: string | null; content?: string | null } | null | undefined,
  hasTitleTemplate: boolean
): string {
  const value = resolveTitleOrContent(node, hasTitleTemplate);
  return hasTitleTemplate ? value : value.replace(/^#+\s*/, '');
}
