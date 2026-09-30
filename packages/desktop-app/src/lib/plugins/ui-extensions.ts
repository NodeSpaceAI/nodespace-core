/**
 * UI-Extension Registry
 * =============================
 *
 * The registry behind NodeSpace's frontend extension API (ADR-082). An
 * extension is a plain object, `{ id, apiVersion, chrome?, viewerTabs? }`, whose
 * contributions each carry an `id`, an optional `when()` predicate, an optional
 * `priority` and a lazy `load()` for the component they mount. Shared hosts
 * (`app-shell.svelte`, `collection-node-viewer.svelte`) import nothing from an
 * extension; they render whatever the registry contributes.
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
 *     registration order (extension first, then contribution order).
 *
 * Lookups never evaluate `when`. This class is plain data with no `$state` and
 * no reactivity; the reactive filtering by `when` lives in the sibling
 * `ui-extensions.svelte.ts` wrapper (ADR-049).
 */

import type { Component } from 'svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UiExtensionRegistry');

/** The extension API version this build implements. */
export const EXTENSION_API_VERSION = { major: 1, minor: 0 } as const;

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

export interface NodespaceExtension {
  id: string;
  apiVersion: typeof EXTENSION_API_VERSION.major;
  chrome?: ChromeContribution[];
  viewerTabs?: ViewerTabContribution[];
}

/** A contribution as a host sees it; `key` is `${extensionId}/${id}`. */
export type Keyed<C> = C & { extensionId: string; key: string };

interface RegisteredExtension {
  extension: NodespaceExtension;
  chrome: Keyed<ChromeContribution>[];
  viewerTabs: Keyed<ViewerTabContribution>[];
}

function priorityOf(c: { priority?: number }): number {
  return typeof c.priority === 'number' && Number.isFinite(c.priority) ? c.priority : 0;
}

/** Descending priority. `Array.prototype.sort` is stable, so ties keep their incoming order. */
function byPriority<C extends { priority?: number }>(list: C[]): C[] {
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
      viewerTabs: keyContributions(ext.id, ext.viewerTabs, 'viewerTabs', seenIds, takenKeys)
    });
    log.debug('Registered extension', { id: ext.id });
  }

  /** Every key held by a registered extension. */
  private registeredKeys(): Set<string> {
    const keys = new Set<string>();
    for (const entry of this.extensions.values()) {
      for (const c of entry.chrome) keys.add(c.key);
      for (const t of entry.viewerTabs) keys.add(t.key);
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
