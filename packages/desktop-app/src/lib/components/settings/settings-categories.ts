/**
 * The Settings category list: core's own categories plus the sections
 * extensions contribute (ADR-082 §2.2, §2.3).
 *
 * `settingsCategories()` is what the sidebar renders. The pane asks
 * `isSettingsCategoryVisible` whether the category it is showing still exists
 * and `findSettingsSection` for the contributed section to mount. All three read
 * Labs flags and `when()` predicates, so call them inside a `$derived` or an
 * effect and they re-run when those change.
 */

import { labsFlags } from '$lib/stores/labs-flags.svelte';
import {
  byPriority,
  uiExtensionRegistry,
  type Keyed,
  type SettingsSectionContribution
} from '$lib/plugins/ui-extensions';
import { getActiveSettingsSections } from '$lib/plugins/ui-extensions.svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('SettingsCategories');

export interface CoreSettingsCategory {
  id: string;
  label: string;
  /** Absent means always listed. Reactive: it may read `$state`. */
  visible?: () => boolean;
}

/**
 * Core's categories in display order. "AI Models" is gated behind the Labs "AI
 * Chat" toggle and "Playbooks" behind the Labs "Playbooks" toggle (both default
 * off); the flags gate the UI only. "Account" is gated behind the Labs "Team
 * synchronization" toggle (default off): the entry itself, not just its content,
 * stays hidden until a user opts in.
 */
export const CORE_SETTINGS_CATEGORIES: readonly CoreSettingsCategory[] = [
  { id: 'database', label: 'Database' },
  { id: 'account', label: 'Account', visible: () => labsFlags.syncEnabled },
  { id: 'display', label: 'Display' },
  { id: 'ai-models', label: 'AI Models', visible: () => labsFlags.aiChatEnabled },
  { id: 'import', label: 'Import Sources' },
  { id: 'integrations', label: 'Integrations' },
  { id: 'playbooks', label: 'Playbooks', visible: () => labsFlags.playbooksEnabled },
  { id: 'labs', label: 'Labs' },
  { id: 'about', label: 'About' }
];

/**
 * A section whose `after` is absent, or names nothing that exists, is listed just
 * before this category, after any sections anchored to the entry above it. It must
 * name an entry of `CORE_SETTINGS_CATEGORIES`: those sections are emitted when the
 * list reaches it.
 */
const FALLBACK_BEFORE_ID = 'about';

/** One row of the sidebar. */
export interface SettingsCategoryEntry {
  /** The navigation id: what `activeCategory`, `navigate()` and `initialCategory` hold. */
  id: string;
  label: string;
  /** The `{#each}` key: the id for a core category, `<extension id>/<section id>` for a section. */
  key: string;
  /** Set for a contributed section: what the pane mounts. */
  section?: Keyed<SettingsSectionContribution>;
}

/**
 * Section keys already reported as unusable, so a re-derived list does not repeat
 * the warning. It lives for the process: a test that asserts a warning uses an
 * extension id no other test uses.
 */
const warnedKeys = new Set<string>();

function warnOnce(key: string, message: string, context: Record<string, unknown>): void {
  if (warnedKeys.has(key)) return;
  warnedKeys.add(key);
  log.warn(message, { key, ...context });
}

/**
 * Drop sections whose id is a core id or the id of an earlier-registered
 * section: core wins, and among sections the first registration wins. Each drop
 * is logged once.
 */
function withoutIdCollisions(
  sections: Keyed<SettingsSectionContribution>[]
): Keyed<SettingsSectionContribution>[] {
  const taken = new Set(CORE_SETTINGS_CATEGORIES.map((c) => c.id));
  const kept: Keyed<SettingsSectionContribution>[] = [];
  for (const section of sections) {
    if (taken.has(section.id)) {
      warnOnce(section.key, 'Settings section ignored: its id is already in use', {
        id: section.id
      });
      continue;
    }
    taken.add(section.id);
    kept.push(section);
  }
  return kept;
}

/**
 * The anchor each section is listed after: a core id, another section's id, or
 * `null` for "before About". A section that names itself, or sits on an `after`
 * cycle, is listed before About instead, so it is never lost.
 */
function anchorsOf(
  sections: Keyed<SettingsSectionContribution>[]
): Map<Keyed<SettingsSectionContribution>, string | null> {
  const byId = new Map(sections.map((s) => [s.id, s]));
  const known = new Set([...CORE_SETTINGS_CATEGORIES.map((c) => c.id), ...byId.keys()]);
  const anchors = new Map<Keyed<SettingsSectionContribution>, string | null>();
  for (const s of sections) {
    anchors.set(s, typeof s.after === 'string' && known.has(s.after) ? s.after : null);
  }
  for (const s of sections) {
    const seen = new Set<string>();
    let cursor = anchors.get(s) ?? null;
    while (cursor !== null) {
      const next = byId.get(cursor);
      if (!next) break; // reached a core category
      if (next === s) {
        warnOnce(s.key, 'Settings section anchored to itself through a cycle; listing it before About', {
          id: s.id
        });
        anchors.set(s, null);
        break;
      }
      // A cycle this section only leads into: it is broken when its own members are visited.
      if (seen.has(cursor)) break;
      seen.add(cursor);
      cursor = anchors.get(next) ?? null;
    }
  }
  return anchors;
}

/**
 * The sidebar entries, in display order.
 *
 *   1. Start from the full core list, hidden entries included, so a section
 *      placed after a hidden category keeps its place.
 *   2. Drop sections that collide with a core id or an earlier section.
 *   3. Insert each section after the entry its `after` names, which may be a core
 *      category or another section; sections sharing an anchor go in descending
 *      priority, then registration order. Without a usable `after`, a section
 *      goes before About, after any sections anchored to the entry above About
 *      (whatever their priorities).
 *   4. Drop hidden core entries and sections whose `when()` is false or throws.
 */
export function settingsCategories(): SettingsCategoryEntry[] {
  const sections = withoutIdCollisions(uiExtensionRegistry.settingsSections());
  const anchors = anchorsOf(sections);

  const anchored = new Map<string | null, Keyed<SettingsSectionContribution>[]>();
  for (const s of sections) {
    const anchor = anchors.get(s) ?? null;
    anchored.set(anchor, [...(anchored.get(anchor) ?? []), s]);
  }
  /** The sections listed right after `anchor` (`null`: before About), in display order. */
  const childrenOf = (anchor: string | null): Keyed<SettingsSectionContribution>[] =>
    byPriority([...(anchored.get(anchor) ?? [])]);

  const active = new Set(getActiveSettingsSections().map((s) => s.key));
  const ordered: { entry: SettingsCategoryEntry; shown: boolean }[] = [];
  const placeSection = (s: Keyed<SettingsSectionContribution>): void => {
    ordered.push({
      entry: { id: s.id, label: s.label, key: s.key, section: s },
      shown: active.has(s.key)
    });
    for (const child of childrenOf(s.id)) placeSection(child);
  };

  for (const core of CORE_SETTINGS_CATEGORIES) {
    if (core.id === FALLBACK_BEFORE_ID) {
      for (const s of childrenOf(null)) placeSection(s);
    }
    ordered.push({
      entry: { id: core.id, label: core.label, key: core.id },
      shown: core.visible?.() ?? true
    });
    for (const s of childrenOf(core.id)) placeSection(s);
  }

  return ordered.filter((o) => o.shown).map((o) => o.entry);
}

/** Whether `id` names a category the sidebar lists now. Unknown, unregistered and hidden ids are not. */
export function isSettingsCategoryVisible(id: string): boolean {
  return settingsCategories().some((c) => c.id === id);
}

/** The contributed section the sidebar lists under `id`, if any. Core ids have none. */
export function findSettingsSection(id: string): Keyed<SettingsSectionContribution> | undefined {
  return settingsCategories().find((c) => c.id === id)?.section;
}
