/**
 * UI-Extension Registry
 * =============================
 *
 * A sibling to the node-type {@link PluginRegistry} for Pro-edition GUI chrome
 * (the sync pill, the re-login modal, the collaboration tab). Shared components
 * (`app-shell.svelte`, `collection-node-viewer.svelte`) import nothing Pro; they
 * render whatever the registry contributes for the currently-resolved variant.
 *
 * Two independent signals drive which variant renders (ADR-053):
 *   1. `proSync.tier` — "can this daemon binary sync?" (app-global, static per
 *      session).
 *   2. the active database's `DatabaseSettingsNode` — "is sync enabled /
 *      authenticated for THIS database?" (per-database, changes on switch or
 *      settings edit).
 *
 * The registry itself is a plain data class (mirrors `PluginRegistry`); the
 * reactivity that filters contributions by the resolved variant lives in the
 * sibling `ui-extensions.svelte.ts` wrapper, never baked into this class
 * (ADR-049).
 */

import type { Component } from 'svelte';
import { createLogger } from '$lib/utils/logger';
import type { ProSyncVariant } from './pro-sync-variant.svelte';

const log = createLogger('UiExtensionRegistry');

/** The chrome slots a contribution can target in the app shell. */
export type ChromeSlot = 'app-shell-overlay' | 'app-shell-modal';

/**
 * One chrome contribution: a lazily-loaded component mounted into `slot` when the
 * resolved variant equals `variant`. Kept declarative — no component is imported
 * eagerly, so the shared shell never pulls a Pro component into its bundle graph.
 */
export interface ChromeContribution {
  slot: ChromeSlot;
  /** Which `resolveVariant()` output this renders for. */
  variant: ProSyncVariant;
  lazyLoad: () => Promise<{ default: Component }>;
  /** Higher renders first when several match (default 0). */
  priority?: number;
}

/**
 * One node-viewer extension: a tab contributed to a viewer of `tab.appliesTo`
 * node type, mounted when the resolved variant equals `variant`. The component
 * receives the host node's id.
 */
export interface ViewerExtension {
  tab: { appliesTo: string; id: string; label: string };
  variant: ProSyncVariant;
  lazyLoad: () => Promise<{ default: Component<{ nodeId: string }> }>;
  priority?: number;
}

/**
 * A declarative bundle of chrome + viewer contributions for one feature. Multiple
 * definitions can register; the wrapper flattens and variant-filters across all.
 */
export interface UiExtensionDefinition {
  id: string;
  name: string;
  version: string;
  chrome?: ChromeContribution[];
  viewerExtensions?: ViewerExtension[];
}

/**
 * Holds registered UI-extension definitions. Pure data + lookups — no `$state`,
 * no reactivity (that is layered on in `ui-extensions.svelte.ts`). Mirrors the
 * structural shape of `PluginRegistry` (plain class, `Map`, register/unregister).
 */
export class UiExtensionRegistry {
  private extensions = new Map<string, UiExtensionDefinition>();

  /** Register (or replace) a definition by id. Idempotent. */
  register(def: UiExtensionDefinition): void {
    this.extensions.set(def.id, def);
    log.debug('registered ui-extension', { id: def.id });
  }

  /** Remove a definition by id. */
  unregister(id: string): void {
    this.extensions.delete(id);
  }

  /** Whether a definition with `id` is registered. */
  has(id: string): boolean {
    return this.extensions.has(id);
  }

  /** All registered definitions, in insertion order. */
  all(): UiExtensionDefinition[] {
    return [...this.extensions.values()];
  }

  /**
   * Every chrome contribution targeting `slot`, across all definitions, sorted by
   * descending priority. Does NOT variant-filter — the reactive wrapper does that.
   */
  chromeFor(slot: ChromeSlot): ChromeContribution[] {
    const out: ChromeContribution[] = [];
    for (const def of this.extensions.values()) {
      for (const c of def.chrome ?? []) {
        if (c.slot === slot) out.push(c);
      }
    }
    return out.sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
  }

  /**
   * Every viewer extension whose tab applies to `nodeType`, across all
   * definitions, sorted by descending priority. Does NOT variant-filter.
   */
  viewersFor(nodeType: string): ViewerExtension[] {
    const out: ViewerExtension[] = [];
    for (const def of this.extensions.values()) {
      for (const e of def.viewerExtensions ?? []) {
        if (e.tab.appliesTo === nodeType) out.push(e);
      }
    }
    return out.sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
  }
}

/** Process-wide singleton (mirrors `pluginRegistry`). */
export const uiExtensionRegistry = new UiExtensionRegistry();
