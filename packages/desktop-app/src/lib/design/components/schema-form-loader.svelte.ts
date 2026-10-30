/**
 * SchemaFormLoader — lazy-loads type-specific and generic schema forms for the viewed node.
 *
 * Type-specific forms (e.g. TaskSchemaForm) come from the plugin registry. Any node type
 * with no registered form — user-defined schema types and core types alike — falls back to
 * a generic schema form driven by the node's SchemaNode definition, fetched from the backend.
 *
 * State lives here as `$state` so the viewer re-renders when a form or schema resolves.
 * One instance per viewer component; loading is event-driven (the viewer calls the load
 * methods when the viewed node changes), not `$effect`-driven.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { isSchemaNode, type SchemaNode } from '$lib/types/schema-node';
import { needsGenericSchemaForm } from './node-type-predicates';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('SchemaFormLoader');

export class SchemaFormLoader {
  /**
   * Cache of loaded schema-form components keyed by node type.
   * Only loaded components are recorded; a type with no usable form (none registered, plugin
   * disabled, failed import) is absent, so a later call looks it up again.
   */
  loadedForms = $state<Record<string, unknown>>({});

  /** Generic schema definition for a node type with no hardcoded schema form. */
  genericSchema = $state<SchemaNode | null>(null);

  /**
   * Schemas already fetched from the backend, keyed by node type.
   * `null` = fetched, this type has no schema (structural types like text/header).
   *
   * `genericSchema` is cleared on every navigation, but a schema definition does not change
   * underneath an open viewer, so revisiting a type reuses this instead of re-fetching.
   * Negative entries matter most: without them, every navigation to a schema-less type
   * would issue a backend round trip that is expected to fail.
   *
   * Scoped per `SchemaFormLoader` instance — one per viewer — which is what makes negative
   * caching safe. A type defined mid-session is opened in a viewer with a fresh loader, so
   * it is never served a stale "no schema" answer. Hoisting this to a module-level singleton
   * would be a real behavior change, not just an optimization.
   */
  private schemaCache = new Map<string, SchemaNode | null>();

  /**
   * The type the viewer is currently on, as far as generic-schema loading is concerned.
   * Set when a fetch starts and cleared at the navigation boundary; a fetch that resolves
   * after either event must not publish its result. See `loadGenericSchema`.
   */
  private pendingType: string | null = null;

  /**
   * The node type most recently passed to {@link loadForm} — tracked so
   * {@link hasTitleTemplate} can consult the plugin registry for a
   * hardcoded-form type (task, person), which never populates
   * `genericSchema` (see `loadForm`'s branch below).
   *
   * Reactive because the viewer's header derives from `hasTitleTemplate` before
   * `loadForm` runs: for a hardcoded-form type this field is the only thing that
   * changes, so a plain field would leave the header on its first answer.
   */
  private currentNodeType = $state<string | null>(null);

  /**
   * True when the viewed node's type is title_template-driven — header should be
   * read-only, and its displayed value comes from the node's computed `title`
   * rather than its content.
   *
   * Two sources, because a hardcoded-form type (task, person) never populates
   * `genericSchema` (`loadForm` returns early via the plugin registry's schema
   * form lookup, so `loadGenericSchema` — the only thing that sets
   * `genericSchema` — never runs for it): `pluginRegistry.hasTitleTemplate`
   * covers those (declared statically on the plugin, e.g. personNodePlugin's
   * `config.hasTitleTemplate`); `genericSchema?.titleTemplate` covers every
   * other type (core types with no hardcoded form, and user-defined schema
   * types), fetched from the backend's SchemaNode. The two must stay in
   * agreement per type — a hardcoded-form type declares its template on the
   * plugin, not by the schema fetch path used here for everything else.
   */
  get hasTitleTemplate(): boolean {
    if (this.currentNodeType && pluginRegistry.hasTitleTemplate(this.currentNodeType)) {
      return true;
    }
    return this.genericSchema?.titleTemplate != null;
  }

  /** The loaded type-specific form for a node type (undefined until one has loaded). */
  getForm(nodeType: string): unknown {
    return this.loadedForms[nodeType];
  }

  /** Reset the generic schema (call when navigating to a different node). */
  resetGenericSchema(): void {
    this.genericSchema = null;
    // The viewer calls this at the navigation boundary, so any fetch still in flight for
    // the previous node is superseded here — not only when the new node happens to trigger
    // a fetch of its own. Types with a hardcoded form (task, person) never reach
    // `loadGenericSchema`, so without this the token would still name the previous type and
    // a late response would pass the recency check.
    this.pendingType = null;
    this.currentNodeType = null;
  }

  /**
   * Load a schema form component from the plugin registry if not already loaded.
   * Returns true if a type-specific form exists, false if the generic fallback should be used.
   * Cached in `loadedForms` for subsequent renders.
   */
  async loadForm(nodeType: string): Promise<boolean> {
    // Set synchronously (not after the await below) so `hasTitleTemplate`, and the header
    // deriving from it, update as soon as the type is known rather than after the form's
    // lazy import resolves.
    this.currentNodeType = nodeType;

    // Skip if the typed form is already loaded. Only a loaded component is cached: a type with
    // no usable form (none registered, plugin disabled, or its import failed) is looked up
    // again on the next call, so enabling the plugin later is picked up instead of the
    // negative answer sticking until reload. The lookup is a registry predicate, and the
    // generic schema it falls back to has a cache of its own.
    const cached = this.loadedForms[nodeType];
    if (cached) return true;

    // No typed form registered or enabled for this type: use the generic schema-driven form.
    if (needsGenericSchemaForm(nodeType)) {
      // Unconditional by design: reaching this branch already means "no type-specific
      // form", which is the only thing the generic fallback depends on. Core types with
      // no registered form (e.g. project) need it exactly as much as user-defined ones.
      // Types with no schema at all simply resolve to no schema and render nothing.
      this.loadGenericSchema(nodeType);
      return false;
    }

    try {
      const component = await pluginRegistry.getSchemaForm(nodeType);
      if (component) {
        this.loadedForms = { ...this.loadedForms, [nodeType]: component };
        return true;
      }
      // The registered form did not load; not cached, so the next call retries.
      return false;
    } catch (error) {
      log.warn(`Failed to load schema form for ${nodeType}:`, error);
      return false;
    }
  }

  /**
   * Load the generic schema definition for a node type, fetching it at most once per type.
   *
   * Schema definitions are immutable for the session, so a cache hit — including a negative
   * one — is served without touching the backend.
   *
   * Callers invoke this without awaiting, and the viewer calls `resetGenericSchema()` +
   * `loadForm()` on every navigation, so an in-flight fetch for the previous node can
   * resolve after the switch. `pendingType` is checked before publishing to `genericSchema`
   * so a superseded response is dropped: a stale schema would not just show the wrong
   * properties, it feeds `hasTitleTemplate`, which makes the header read-only.
   *
   * The cache write is deliberately outside that guard — a fetched schema is valid for its
   * own type regardless of where the user navigated in the meantime.
   */
  async loadGenericSchema(nodeType: string): Promise<void> {
    this.pendingType = nodeType;

    if (this.schemaCache.has(nodeType)) {
      this.genericSchema = this.schemaCache.get(nodeType) ?? null;
      return;
    }

    try {
      const schemaNode = await backendAdapter.getSchema(nodeType);
      if (isSchemaNode(schemaNode)) {
        this.schemaCache.set(nodeType, schemaNode);
        if (this.pendingType === nodeType) this.genericSchema = schemaNode;
        return;
      }
      this.schemaCache.set(nodeType, null);
    } catch (error) {
      // Expected for structural types (text, header, code-block, …) that have no schema —
      // debug, not warn, so ordinary outlining doesn't emit a stream of warnings.
      this.schemaCache.set(nodeType, null);
      log.debug(`No generic schema for ${nodeType}:`, error);
    }
  }
}
