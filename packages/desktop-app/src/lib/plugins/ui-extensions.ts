/**
 * UI-Extension Registry
 * =============================
 *
 * The registry behind NodeSpace's frontend extension API (ADR-082). An
 * extension is a plain object, `{ id, apiVersion, chrome?, viewerTabs?,
 * settingsSections?, settingsSlots? }`, whose contributions each carry an `id`,
 * an optional `when()` predicate, an optional `priority` and a lazy `load()` for
 * the component they mount. Shared hosts (`app-shell.svelte`,
 * `collection-node-viewer.svelte`, the Settings pane and Databases page) import
 * nothing from an extension; they render whatever the registry contributes.
 *
 * Registration:
 *   - `registerExtensions([...])` is what a build entry calls with the
 *     extensions it bundles. `uiExtensionRegistry.register(ext)` registers one.
 *   - Registration never throws: a throw at startup would blank the app. A
 *     duplicate extension id (the first registration is kept), an `apiVersion`
 *     that is not {@link EXTENSION_API_VERSION}`.major`, and a contribution id
 *     repeated within one extension (the later one is dropped) are logged and
 *     skipped. Registering the identical object twice is a silent no-op.
 *
 * Keys and ordering:
 *   - A host sees each contribution as `Keyed`: `key` is
 *     `<extension id>/<contribution id>`, unique across extensions, and is what
 *     hosts key their `{#each}` blocks and tab selection by. Ids may contain `/`,
 *     so a contribution whose key an earlier extension already holds is dropped
 *     (a repeated key would throw in a keyed `{#each}`).
 *   - Lookups return contributions in descending `priority` (default 0), ties in
 *     registration order (extension first, then contribution order). The one
 *     exception is `settingsSections()`, which returns registration order: the
 *     Settings category list places a section by its `after` and applies priority
 *     only among sections that share an anchor.
 *
 * Lookups never evaluate `when`. This class is plain data with no `$state` and
 * no reactivity; the reactive filtering by `when` lives in the sibling
 * `ui-extensions.svelte.ts` wrapper (ADR-049).
 *
 * Collection-tree roots: an extension may also carry `collectionTreeRoots`, a
 * function naming collections the sidebar tree hides as containers.
 * `collectionTreeRoots()` calls every one on each lookup and returns their union,
 * so the collections store reads it straight from this registry inside its
 * derivation.
 *
 * Lifecycle: an extension may also carry `lifecycle` hooks and a `debugDump`.
 * They are invoked by `extension-lifecycle.ts`, not by this registry, so the
 * registry stays free of any host wiring.
 */

import type { Component } from 'svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UiExtensionRegistry');

/** The extension API version this build implements. */
export const EXTENSION_API_VERSION = { major: 1, minor: 0 } as const;

// --- Lifecycle hooks (ADR-082 §2.5) ---------------------------------------------

/**
 * Callbacks the host invokes at fixed points in the app's life. Where exactly,
 * and what each guarantees, is part of the versioned extension API and is
 * documented in `extension-lifecycle.ts`, which also implements the dispatch.
 */
export interface ExtensionLifecycle {
  /**
   * Runs once per webview load, when the app shell mounts, and only when the
   * Tauri bridge is present. May return a cleanup, directly or through a
   * promise; the host runs it when the shell unmounts.
   */
  start?(): void | (() => void) | Promise<void | (() => void)>;
  /**
   * Called synchronously each time a database becomes the active one, after the
   * previous database's caches have been evicted. Async work belongs to the
   * extension, which serializes it itself.
   */
  onDatabaseActivated?(databaseId: string): void;
}

// --- End of lifecycle hooks -----------------------------------------------------

/**
 * One contribution: a lazily-loaded component plus the conditions under which a
 * host shows it. Nothing is imported eagerly, so a host never pulls an
 * extension's components into its own bundle graph.
 */
export interface Contribution<P extends Record<string, unknown> = Record<string, never>> {
  /** Unique within its extension, across all of the extension's contribution lists. */
  id: string;
  /** Reactive and pure. Absent means always shown. */
  when?: () => boolean;
  /** Higher renders first; default 0. */
  priority?: number;
  load: () => Promise<{ default: Component<P> }>;
}

/** The chrome slots a contribution can target in the app shell. */
export type ChromeSlot = 'app-shell-overlay' | 'app-shell-modal';

/** A component mounted into an app-shell chrome slot. */
export type ChromeContribution = Contribution & { slot: ChromeSlot };

/**
 * A tab contributed to the viewer of `nodeType` nodes. The component receives
 * the host node's id.
 */
export type ViewerTabContribution = Contribution<{ nodeId: string }> & {
  nodeType: string;
  label: string;
};

// --- Settings extension points (ADR-082 §2.2) --------------------------------

/**
 * A category in the Settings sidebar and the pane it opens. Its `id` is also its
 * navigation id, sharing one namespace with the core categories, so `after`,
 * `navigate(id)` and `settingsStore.initialCategory` all take core and
 * contributed ids alike. The section is placed after the entry `after` names, or
 * before About when `after` is absent or names nothing that exists. The
 * component receives `navigate` to switch the pane to another category.
 */
export type SettingsSectionContribution = Contribution<{
  navigate: (sectionId: string) => void;
}> & { label: string; after?: string };

/** The places inside the Databases settings page a contribution can target. */
export type SettingsSlot = 'database.actions' | 'database.row';

/**
 * Content mounted into a Databases-page slot: `database.actions` sits in the
 * header beside the New and Open buttons and takes no props; `database.row` sits
 * inside each database's row and receives that row's database id.
 */
export type SettingsSlotContribution =
  | (Contribution & { slot: 'database.actions' })
  | (Contribution<{ databaseId: string }> & { slot: 'database.row' });

/** The contributions of `SettingsSlotContribution` that target `S`. */
export type SettingsSlotContributionFor<S extends SettingsSlot> = Extract<
  SettingsSlotContribution,
  { slot: S }
>;

// --- End of settings extension points -----------------------------------------

// --- Collection-tree roots (ADR-082 §2.2) --------------------------------------

/**
 * Names the collections the sidebar collection tree treats as invisible
 * containers. A root is never shown: a collection whose only parent is a root
 * is shown at the top level instead of nested under it, and the root's own row
 * is dropped. The host takes the union of every extension's list with core's
 * own root, and with no extension contributing the tree is unchanged.
 *
 * It is presentation only. The host evaluates it inside a derivation, so it
 * must read only reactive sources and have no side effects (ADR-049). One that
 * throws counts as an empty list and is logged once (ADR-082 §2.4).
 */
export type CollectionTreeRootsContribution = () => readonly string[];

// --- End of collection-tree roots ----------------------------------------------

export interface NodespaceExtension {
  id: string;
  apiVersion: typeof EXTENSION_API_VERSION.major;
  /** Host-invoked callbacks; see {@link ExtensionLifecycle}. */
  lifecycle?: ExtensionLifecycle;
  /**
   * A synchronous, JSON-representable snapshot of the extension's state for the
   * debug channel's store dump, keyed there by the extension id. May include
   * user content and personal data.
   */
  debugDump?: () => unknown;
  chrome?: ChromeContribution[];
  viewerTabs?: ViewerTabContribution[];
  settingsSections?: SettingsSectionContribution[];
  settingsSlots?: SettingsSlotContribution[];
  /** Collections the sidebar tree hides as containers; see {@link CollectionTreeRootsContribution}. */
  collectionTreeRoots?: CollectionTreeRootsContribution;
}

/** A contribution as a host sees it; `key` is `${extensionId}/${id}`. */
export type Keyed<C> = C & { extensionId: string; key: string };

interface RegisteredExtension {
  extension: NodespaceExtension;
  chrome: Keyed<ChromeContribution>[];
  viewerTabs: Keyed<ViewerTabContribution>[];
  settingsSections: Keyed<SettingsSectionContribution>[];
  settingsSlots: Keyed<SettingsSlotContribution>[];
}

function priorityOf(c: { priority?: number }): number {
  return typeof c.priority === 'number' && Number.isFinite(c.priority) ? c.priority : 0;
}

/**
 * Sorts `list` in place by descending priority and returns it. `Array.prototype.sort`
 * is stable, so ties keep their incoming order.
 */
export function byPriority<C extends { priority?: number }>(list: C[]): C[] {
  return list.sort((a, b) => priorityOf(b) - priorityOf(a));
}

/**
 * Validate one contribution list and attach keys. A malformed or repeated
 * contribution is logged and dropped, never thrown. `seenIds` is shared across
 * an extension's lists because ids are unique across all of them; `takenKeys`
 * holds every key registered so far, across extensions.
 */
function keyContributions<C extends { id: string; load: unknown }>(
  extensionId: string,
  list: readonly C[] | undefined,
  listName: string,
  seenIds: Set<string>,
  takenKeys: Set<string>
): Keyed<C>[] {
  if (list === undefined) return [];
  if (!Array.isArray(list)) {
    log.error('Extension contribution list is not an array; skipped', {
      extensionId,
      list: listName
    });
    return [];
  }
  const out: Keyed<C>[] = [];
  for (const c of list) {
    if (
      typeof c !== 'object' ||
      c === null ||
      typeof c.id !== 'string' ||
      c.id === '' ||
      typeof c.load !== 'function'
    ) {
      log.error('Extension contribution needs a string id and a load function; dropped', {
        extensionId,
        list: listName
      });
      continue;
    }
    if (seenIds.has(c.id)) {
      log.error('Duplicate contribution id within an extension; the later one is dropped', {
        extensionId,
        contributionId: c.id
      });
      continue;
    }
    const key = `${extensionId}/${c.id}`;
    if (takenKeys.has(key)) {
      log.error('Contribution key is already held by another extension; dropped', { key });
      continue;
    }
    seenIds.add(c.id);
    takenKeys.add(key);
    out.push({ ...c, extensionId, key });
  }
  return out;
}

/**
 * Holds registered extensions. Pure data + lookups — no `$state`, no
 * reactivity (that is layered on in `ui-extensions.svelte.ts`). Mirrors the
 * structural shape of `PluginRegistry` (plain class, `Map`, register/unregister).
 */
export class UiExtensionRegistry {
  private extensions = new Map<string, RegisteredExtension>();

  /** Register an extension. Never throws; see the module doc for what is skipped. */
  register(ext: NodespaceExtension): void {
    try {
      this.registerUnchecked(ext);
    } catch (error) {
      log.error('Extension registration failed', { error });
    }
  }

  private registerUnchecked(ext: NodespaceExtension): void {
    if (typeof ext !== 'object' || ext === null || typeof ext.id !== 'string' || ext.id === '') {
      log.error('Extension ignored: it has no string id');
      return;
    }
    const existing = this.extensions.get(ext.id);
    if (existing) {
      if (existing.extension === ext) {
        log.debug('Extension already registered', { id: ext.id });
      } else {
        log.error('Duplicate extension id; keeping the first registration', { id: ext.id });
      }
      return;
    }
    if (ext.apiVersion !== EXTENSION_API_VERSION.major) {
      log.error('Extension apiVersion mismatch; not registered', {
        id: ext.id,
        apiVersion: ext.apiVersion,
        supported: EXTENSION_API_VERSION.major
      });
      return;
    }
    const seenIds = new Set<string>();
    const takenKeys = this.registeredKeys();
    this.extensions.set(ext.id, {
      extension: ext,
      chrome: keyContributions(ext.id, ext.chrome, 'chrome', seenIds, takenKeys),
      viewerTabs: keyContributions(ext.id, ext.viewerTabs, 'viewerTabs', seenIds, takenKeys),
      settingsSections: keyContributions(
        ext.id,
        ext.settingsSections,
        'settingsSections',
        seenIds,
        takenKeys
      ),
      settingsSlots: keyContributions(ext.id, ext.settingsSlots, 'settingsSlots', seenIds, takenKeys)
    });
    log.debug('Registered extension', { id: ext.id });
  }

  /** Every key held by a registered extension. */
  private registeredKeys(): Set<string> {
    const keys = new Set<string>();
    for (const entry of this.extensions.values()) {
      for (const c of entry.chrome) keys.add(c.key);
      for (const t of entry.viewerTabs) keys.add(t.key);
      for (const s of entry.settingsSections) keys.add(s.key);
      for (const s of entry.settingsSlots) keys.add(s.key);
    }
    return keys;
  }

  /** Remove an extension by id. */
  unregister(id: string): void {
    this.extensions.delete(id);
  }

  /** Whether an extension with `id` is registered. */
  has(id: string): boolean {
    return this.extensions.has(id);
  }

  /** All registered extensions, in registration order. */
  all(): NodespaceExtension[] {
    return [...this.extensions.values()].map((entry) => entry.extension);
  }

  /**
   * Every chrome contribution targeting `slot`, across all extensions, in
   * descending priority with ties in registration order. Does NOT evaluate `when`.
   */
  chromeFor(slot: ChromeSlot): Keyed<ChromeContribution>[] {
    const out: Keyed<ChromeContribution>[] = [];
    for (const entry of this.extensions.values()) {
      for (const c of entry.chrome) {
        if (c.slot === slot) out.push(c);
      }
    }
    return byPriority(out);
  }

  /**
   * Every viewer tab for `nodeType`, across all extensions, in descending
   * priority with ties in registration order. Does NOT evaluate `when`.
   */
  viewerTabsFor(nodeType: string): Keyed<ViewerTabContribution>[] {
    const out: Keyed<ViewerTabContribution>[] = [];
    for (const entry of this.extensions.values()) {
      for (const t of entry.viewerTabs) {
        if (t.nodeType === nodeType) out.push(t);
      }
    }
    return byPriority(out);
  }

  /**
   * Every settings section, across all extensions, in registration order
   * (extension first, then contribution order). Not sorted by priority: a
   * section's place comes from `after`, and priority only orders sections that
   * share an anchor, which the Settings category list resolves. Does NOT
   * evaluate `when`.
   */
  settingsSections(): Keyed<SettingsSectionContribution>[] {
    const out: Keyed<SettingsSectionContribution>[] = [];
    for (const entry of this.extensions.values()) out.push(...entry.settingsSections);
    return out;
  }

  /**
   * Every contribution to the Databases-page `slot`, across all extensions, in
   * descending priority with ties in registration order. Does NOT evaluate `when`.
   */
  settingsSlotFor<S extends SettingsSlot>(slot: S): Keyed<SettingsSlotContributionFor<S>>[] {
    const out: Keyed<SettingsSlotContribution>[] = [];
    for (const entry of this.extensions.values()) {
      for (const c of entry.settingsSlots) {
        if (c.slot === slot) out.push(c);
      }
    }
    // Every entry above has `slot === S`, which the union filter cannot express.
    return byPriority(out) as Keyed<SettingsSlotContributionFor<S>>[];
  }

  // --- Collection-tree roots (ADR-082 §2.2) ----------------------------------

  /** Extensions whose `collectionTreeRoots` failed and has not returned normally since. */
  private failingTreeRoots = new WeakSet<NodespaceExtension>();

  /**
   * The union of every extension's {@link CollectionTreeRootsContribution}, in
   * registration order. Calls each extension's function on every lookup, with no
   * caching, so a caller inside a derivation re-runs when the reactive state that
   * function reads changes. A contributor that throws or returns something other
   * than an array counts as empty and is logged once, and again only after it
   * has returned normally in between. Entries that are not strings are ignored.
   */
  collectionTreeRoots(): ReadonlySet<string> {
    const roots = new Set<string>();
    for (const { extension } of this.extensions.values()) {
      try {
        const contribute = extension.collectionTreeRoots;
        if (contribute === undefined) continue;
        const ids: unknown = contribute.call(extension);
        if (!Array.isArray(ids)) throw new TypeError('collectionTreeRoots must return an array');
        for (const id of ids) {
          if (typeof id === 'string') roots.add(id);
        }
        this.failingTreeRoots.delete(extension);
      } catch (error) {
        if (!this.failingTreeRoots.has(extension)) {
          this.failingTreeRoots.add(extension);
          log.warn('Extension collectionTreeRoots failed; treating it as empty', {
            extensionId: extension.id,
            error
          });
        }
      }
    }
    return roots;
  }

  // --- End of collection-tree roots ------------------------------------------
}

/** Process-wide singleton (mirrors `pluginRegistry`). */
export const uiExtensionRegistry = new UiExtensionRegistry();

/**
 * Register every extension a build bundles, in order. A non-array argument is
 * logged and ignored.
 */
export function registerExtensions(extensions: readonly NodespaceExtension[]): void {
  if (!Array.isArray(extensions)) {
    log.error('registerExtensions expects an array of extensions; ignored', {
      received: typeof extensions
    });
    return;
  }
  for (const ext of extensions) uiExtensionRegistry.register(ext);
}
