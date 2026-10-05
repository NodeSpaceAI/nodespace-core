/**
 * Host API for build-time extensions (ADR-082 §3.6)
 * ==================================================
 *
 * An extension imports core's frontend only through `@nodespace/extension-api`
 * and its `/ui` and `/testing` entries. The build aliases the package name to
 * this directory. Extension code may import nothing else from core, so core is
 * free to refactor whatever sits behind these entries.
 *
 * Core modules never import the host API: it is the surface core offers
 * extensions, not something core consumes. `extension-api-boundary.test.ts`
 * holds that boundary.
 *
 * Compatibility policy (ADR-082 §8)
 * ---------------------------------
 * `EXTENSION_API_VERSION` (`{ major, minor }`, in `plugins/ui-extensions.ts`)
 * versions:
 *   - this module and its `/ui` and `/testing` entries;
 *   - the `virtual:nodespace-extensions` contract and the `NODESPACE_EXTENSIONS`
 *     variable;
 *   - slot ids, their host props and mount semantics;
 *   - node-type registration through `nodeTypes`, and the `PluginDefinition`
 *     and node-component types it takes;
 *   - contribution failure behaviour (ADR-082 §3.4);
 *   - hook timing, as `plugins/extension-lifecycle.ts` documents it.
 *
 * Which bump a change needs:
 *   - major: removing, renaming or retyping an export; changing a slot's
 *     location, props or mount semantics; changing a hook's timing.
 *   - minor: a new optional slot, hook, service or field.
 *   - none: anything not reachable through the API.
 *
 * Process:
 *   - A change that alters the API bumps `EXTENSION_API_VERSION` in the same PR
 *     and re-records the surface snapshot:
 *       UPDATE_EXTENSION_API_SURFACE=1 bun run --cwd packages/desktop-app test src/tests/extension-api
 *   - The PR description says so, and so do core's release notes, under an
 *     "Extension API" heading.
 *   - Every entry lists its exports explicitly; `export *` is refused, so
 *     nothing reaches the API by accident. Every file in this directory is an
 *     entry. `extension-api-surface.test.ts` compares the export lists and the
 *     API's type declarations with the snapshot. It fails on a change without a
 *     bump and on a removal without a major one; whether a changed type is major
 *     or minor is the reviewer's call, by the rules above.
 *   - Core modules never import the host API.
 */

import type { CreatedNode, CreateNodeInput } from '$lib/services/backend-adapter';
import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { collectionsData } from '$lib/stores/collections.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import { schemasData } from '$lib/stores/schemas.svelte';
import type { Node } from '$lib/types';

// --- Registration ---------------------------------------------------------------

export { EXTENSION_API_VERSION } from '$lib/plugins/ui-extensions';
export type {
  ChromeContribution,
  ChromeSlot,
  CollectionTreeRootsContribution,
  Contribution,
  ExtensionLifecycle,
  NodeTypeContribution,
  NodespaceExtension,
  ReplaceableSlot,
  ReplaceableSlotContribution,
  SettingsSectionContribution,
  SettingsSlot,
  SettingsSlotContribution,
  SettingsSlotContributionFor,
  TreeItemActionContribution,
  TreeItemActionProps,
  ViewerTabContribution
} from '$lib/plugins/ui-extensions';

// --- Node types -----------------------------------------------------------------

/**
 * What a `nodeTypes` entry registers with core's plugin registry (ADR-082
 * §2.1): the type's `PluginDefinition`, and the component types and props its
 * node component takes.
 */
export type { NodeComponent, PluginDefinition } from '$lib/plugins/types';
export type { NodeComponentProps } from '$lib/types/node-viewers';

// --- Databases ------------------------------------------------------------------

export type { DatabaseInfo } from '$lib/stores/database.svelte';

/**
 * The registry of local databases and the one the app is viewing. The getters
 * read the store's state, so a read inside a derivation re-runs when it changes.
 */
export interface ExtensionDatabases {
  /** The database the app is viewing; `null` until the registry has loaded. */
  readonly activeDatabaseId: string | null;
  /** The registry entry for `activeDatabaseId`, or `null` when none is selected. */
  readonly activeDatabase: DatabaseInfo | null;
  /** Every registered database. */
  readonly list: readonly DatabaseInfo[];
  /** The message of the last registry operation that failed; cleared when the next one starts. */
  readonly error: string | null;
  /**
   * Creates a database and registers it, then reloads the list. Without `path`
   * the daemon places the file in its managed directory. Resolves to the new
   * entry, or to `null` with `error` set.
   */
  create(name: string, path?: string): Promise<DatabaseInfo | null>;
  /**
   * Makes `id` the active database. Does nothing for the database already
   * active, or for an id the registry does not hold. Extensions learn of the
   * switch through `lifecycle.onDatabaseActivated`.
   */
  switchTo(id: string): Promise<void>;
}

export const databases: ExtensionDatabases = {
  get activeDatabaseId() {
    return databaseStore.activeDatabaseId;
  },
  get activeDatabase() {
    return databaseStore.activeDatabase;
  },
  get list() {
    return databaseStore.databases;
  },
  get error() {
    return databaseStore.error;
  },
  create: (name, path) => databaseStore.create(name, path),
  switchTo: (id) => databaseStore.switchTo(id)
};

// --- Nodes ----------------------------------------------------------------------

interface ExtensionNodes {
  /**
   * The node as the shared node store holds it, or `undefined` when it is not
   * cached. A reactive read: inside a derivation it re-runs when the node is
   * set, updated or evicted. It neither fetches nor pins, so a node outside
   * every open document can be evicted while it is shown.
   */
  getNode(id: string): Node | undefined;
  /** Reads the node from the daemon; `null` when it does not exist. Leaves the node store alone. */
  fetchNode(id: string): Promise<Node | null>;
  /** Creates a node through the daemon. */
  createNode(input: CreateNodeInput): Promise<CreatedNode>;
  /**
   * Updates a node through the app's node-update path: the change shows in
   * `getNode` at once and is saved to the daemon in the background. `properties`
   * is merged into the node's existing properties, not substituted for them, so a key cannot be removed through it. It
   * returns nothing, so it neither waits for the save nor reports a failed one:
   * a failure surfaces through core's conflict notifications. A node the store
   * does not hold is left alone and logged.
   */
  updateNode(id: string, changes: ExtensionNodeChanges): void;
}

/** The fields `nodes.updateNode` changes. */
export type ExtensionNodeChanges = Partial<Pick<Node, 'content' | 'properties'>>;

/** What a write through `nodes.updateNode` is attributed to in the node store. */
const EXTENSION_UPDATE_SOURCE = { type: 'viewer', viewerId: 'extension-api' } as const;

export const nodes: ExtensionNodes = {
  getNode: (id) => sharedNodeStore.getNode(id),
  fetchNode: (id) => backendAdapter.getNode(id),
  createNode: (input) => backendAdapter.createNode(input),
  // Picks the two fields at runtime: the type is only a compile-time guard, and
  // the store would also apply a type conversion or a version from an untyped caller.
  updateNode: (id, { content, properties }) =>
    sharedNodeStore.updateNode(
      id,
      {
        ...(content !== undefined && { content }),
        ...(properties !== undefined && { properties })
      },
      EXTENSION_UPDATE_SOURCE
    )
};

// --- Collections and schemas ----------------------------------------------------

interface ExtensionReloadable {
  /** Re-fetches the whole list from the daemon; the sidebar and pickers follow. */
  reload(): Promise<void>;
}

/** The active database's collections. */
export const collections: ExtensionReloadable = {
  reload: () => collectionsData.loadCollections()
};

/** The active database's schemas. */
export const schemas: ExtensionReloadable = {
  reload: () => schemasData.loadSchemas()
};

// --- Daemon, logging and errors -------------------------------------------------

export { onDaemonReconnect } from '$lib/services/daemon-status';
export { createLogger } from '$lib/utils/logger';
export { isCommandError, toError, type CommandError } from '$lib/types/errors';
