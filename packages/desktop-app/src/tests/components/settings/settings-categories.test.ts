/**
 * settings-categories.ts: the Settings sidebar's category list.
 *
 * Core's own list and its Labs gates, and how contributed sections are placed
 * (`after`, priority, registration order), dropped (id collisions) and hidden
 * (`when()`). These are pure functions over the process-wide registry, so each
 * test registers throwaway extensions and unregisters them afterwards.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

import {
  CORE_SETTINGS_CATEGORIES,
  findSettingsSection,
  isSettingsCategoryVisible,
  settingsCategories
} from '$lib/components/settings/settings-categories';
import {
  uiExtensionRegistry,
  type NodespaceExtension,
  type SettingsSectionContribution
} from '$lib/plugins/ui-extensions';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { setAllLabsFlags } from '../../helpers/labs-flags';

const noComponent = () => Promise.reject(new Error('not loaded in this test'));

function section(
  id: string,
  extra: Partial<SettingsSectionContribution> = {}
): SettingsSectionContribution {
  return { id, label: id, load: noComponent, ...extra };
}

const registered: string[] = [];

/**
 * Register an extension carrying `sections`; it is unregistered after the test.
 * A test that asserts a warning uses an extension id no other test uses: the
 * category list warns once per section key for the life of the process.
 */
function register(extensionId: string, sections: SettingsSectionContribution[]): void {
  const extension: NodespaceExtension = {
    id: extensionId,
    apiVersion: 2,
    settingsSections: sections
  };
  uiExtensionRegistry.register(extension);
  registered.push(extensionId);
}

const ids = () => settingsCategories().map((c) => c.id);

const CORE_ALL = [
  'database',
  'display',
  'ai-models',
  'import',
  'integrations',
  'playbooks',
  'labs',
  'about'
];
const CORE_OFF = ['database', 'display', 'import', 'integrations', 'labs', 'about'];

beforeEach(() => {
  localStorage.clear();
  setAllLabsFlags(false);
  log.warn.mockClear();
});

afterEach(() => {
  for (const id of registered.splice(0)) uiExtensionRegistry.unregister(id);
  setAllLabsFlags(false);
  localStorage.clear();
});

describe('core categories', () => {
  it('lists every category, in order, when all Labs flags are on', () => {
    setAllLabsFlags(true);

    expect(ids()).toEqual(CORE_ALL);
    expect(settingsCategories().map((c) => c.label)).toEqual([
      'Database',
      'Display',
      'AI Models',
      'Import Sources',
      'Integrations',
      'Playbooks',
      'Labs',
      'About'
    ]);
  });

  it('hides the two Labs-gated categories when all flags are off', () => {
    expect(ids()).toEqual(CORE_OFF);
  });

  it('follows each Labs flag independently', () => {
    labsFlags.aiChatEnabled = true;
    expect(ids()).toEqual(['database', 'display', 'ai-models', 'import', 'integrations', 'labs', 'about']);

    labsFlags.aiChatEnabled = false;
    labsFlags.playbooksEnabled = true;
    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'playbooks', 'labs', 'about']);
  });

  it('keys a core entry by its id and gives it no section', () => {
    for (const entry of settingsCategories()) {
      expect(entry.key).toBe(entry.id);
      expect(entry.section).toBeUndefined();
    }
  });

  it('keeps About last and the ids unique', () => {
    const coreIds = CORE_SETTINGS_CATEGORIES.map((c) => c.id);
    expect(coreIds).toEqual(CORE_ALL);
    expect(new Set(coreIds).size).toBe(coreIds.length);
  });
});

describe('section placement', () => {
  it('places a section after the core category its `after` names', () => {
    register('ext', [section('mine', { after: 'database' })]);

    expect(ids()).toEqual(['database', 'mine', 'display', 'import', 'integrations', 'labs', 'about']);
  });

  it('places a section before About when `after` is absent', () => {
    register('ext', [section('mine')]);

    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'labs', 'mine', 'about']);
  });

  it('places a section before About when `after` names nothing that exists', () => {
    register('ext', [section('mine', { after: 'no-such-category' })]);

    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'labs', 'mine', 'about']);
  });

  it('places a section after About when it says so', () => {
    register('ext', [section('mine', { after: 'about' })]);

    expect(ids().slice(-2)).toEqual(['about', 'mine']);
  });

  it('places a section after another section', () => {
    register('ext', [
      section('second', { after: 'first' }),
      section('first', { after: 'display' })
    ]);

    expect(ids()).toEqual(['database', 'display', 'first', 'second', 'import', 'integrations', 'labs', 'about']);
  });

  it('places a section after a core category that is currently hidden, in that category’s slot', () => {
    register('ext', [section('mine', { after: 'ai-models' })]);

    // AI Models is hidden, yet the section sits where it would be: after Display, before Import.
    expect(ids()).toEqual(['database', 'display', 'mine', 'import', 'integrations', 'labs', 'about']);

    labsFlags.aiChatEnabled = true;
    expect(ids()).toEqual(['database', 'display', 'ai-models', 'mine', 'import', 'integrations', 'labs', 'about']);
  });

  it('places a section after another section that is currently hidden, in that section’s slot', () => {
    let anchorShown = false;
    register('ext', [
      section('anchor', { after: 'display', when: () => anchorShown }),
      section('follower', { after: 'anchor' })
    ]);

    expect(ids()).toEqual(['database', 'display', 'follower', 'import', 'integrations', 'labs', 'about']);

    anchorShown = true;
    expect(ids()).toEqual(['database', 'display', 'anchor', 'follower', 'import', 'integrations', 'labs', 'about']);
  });

  it('keys a section as <extension id>/<section id> and carries the contribution', () => {
    register('ext', [section('mine', { label: 'Mine', after: 'database' })]);

    const entry = settingsCategories().find((c) => c.id === 'mine');
    expect(entry).toMatchObject({ id: 'mine', label: 'Mine', key: 'ext/mine' });
    expect(entry?.section).toMatchObject({ id: 'mine', extensionId: 'ext', key: 'ext/mine' });
  });
});

describe('section ordering', () => {
  it('orders sections that share an anchor by descending priority', () => {
    register('ext', [
      section('low', { after: 'display', priority: -1 }),
      section('plain', { after: 'display' }),
      section('high', { after: 'display', priority: 5 })
    ]);

    expect(ids()).toEqual(['database', 'display', 'high', 'plain', 'low', 'import', 'integrations', 'labs', 'about']);
  });

  it('breaks priority ties by registration order, across extensions', () => {
    register('first', [section('a', { after: 'display' })]);
    register('second', [section('b', { after: 'display' }), section('c', { after: 'display' })]);

    expect(ids()).toEqual(['database', 'display', 'a', 'b', 'c', 'import', 'integrations', 'labs', 'about']);
  });

  it('orders sections that go before About by the same rule', () => {
    register('ext', [section('plain'), section('high', { priority: 2 }), section('unknown', { after: 'nope' })]);

    expect(ids().slice(-4)).toEqual(['high', 'plain', 'unknown', 'about']);
  });

  it('uses priority only among sections sharing an anchor, never to move one to another anchor', () => {
    register('ext', [
      section('early', { after: 'database' }),
      section('late-but-boosted', { after: 'labs', priority: 100 })
    ]);

    expect(ids()).toEqual(['database', 'early', 'display', 'import', 'integrations', 'labs', 'late-but-boosted', 'about']);
  });

  it('lists sections anchored to Labs before those with no anchor, which sit just before About', () => {
    register('ext', [section('unanchored'), section('after-labs', { after: 'labs' })]);

    expect(ids().slice(-3)).toEqual(['after-labs', 'unanchored', 'about']);
  });
});

describe('id collisions', () => {
  it('ignores a section whose id is a core id, and logs it', () => {
    register('collides-core', [section('display', { label: 'Impostor' })]);

    expect(ids()).toEqual(CORE_OFF);
    expect(settingsCategories().find((c) => c.id === 'display')?.label).toBe('Display');
    expect(findSettingsSection('display')).toBeUndefined();
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('ignored'),
      expect.objectContaining({ key: 'collides-core/display', id: 'display' })
    );
  });

  it('ignores a section whose id is a core id even when that category is hidden', () => {
    register('collides-hidden', [section('playbooks', { label: 'Impostor' })]);

    expect(ids()).toEqual(CORE_OFF);
    expect(isSettingsCategoryVisible('playbooks')).toBe(false);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('ignored'),
      expect.objectContaining({ key: 'collides-hidden/playbooks' })
    );
  });

  it('keeps the first-registered of two sections with one id, and logs the other', () => {
    register('winner', [section('shared', { label: 'First', after: 'display' })]);
    register('loser', [section('shared', { label: 'Second', after: 'labs' })]);

    const shared = settingsCategories().filter((c) => c.id === 'shared');
    expect(shared).toHaveLength(1);
    expect(shared[0]).toMatchObject({ label: 'First', key: 'winner/shared' });
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('ignored'),
      expect.objectContaining({ key: 'loser/shared' })
    );
  });

  it('does not let a dropped section anchor another', () => {
    register('collides-anchor', [
      section('display', { label: 'Impostor', after: 'labs' }),
      section('follower', { after: 'display' })
    ]);

    // `follower` anchors to the core Display, not to the dropped section after Labs.
    expect(ids()).toEqual(['database', 'display', 'follower', 'import', 'integrations', 'labs', 'about']);
  });

  it('logs a collision once, however often the list is rebuilt', () => {
    register('collides-once', [section('labs')]);

    settingsCategories();
    settingsCategories();
    isSettingsCategoryVisible('labs');

    expect(log.warn).toHaveBeenCalledTimes(1);
  });
});

describe('anchor cycles', () => {
  it('lists a section that names itself before About instead of losing it, and logs it', () => {
    register('cycle-self', [section('selfish', { after: 'selfish' })]);

    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'labs', 'selfish', 'about']);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('cycle'),
      expect.objectContaining({ key: 'cycle-self/selfish' })
    );
  });

  it('lists every section of a cycle, the first-registered one before About, and logs it', () => {
    register('cycle-two', [section('a', { after: 'b' }), section('b', { after: 'a' })]);

    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'labs', 'a', 'b', 'about']);
    expect(log.warn).toHaveBeenCalledTimes(1);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('cycle'),
      expect.objectContaining({ key: 'cycle-two/a' })
    );
  });

  it('lists a section that leads into a cycle after its anchor, in registration order', () => {
    register('cycle-tail', [
      section('a', { after: 'b' }),
      section('b', { after: 'a' }),
      section('tail', { after: 'a' })
    ]);

    expect(ids()).toEqual(['database', 'display', 'import', 'integrations', 'labs', 'a', 'b', 'tail', 'about']);
  });
});

describe('when()', () => {
  it('hides a section while its when() is false', () => {
    let shown = false;
    register('ext', [section('mine', { after: 'database', when: () => shown })]);

    expect(ids()).toEqual(CORE_OFF);

    shown = true;
    expect(ids()).toEqual(['database', 'mine', 'display', 'import', 'integrations', 'labs', 'about']);
  });

  it('hides a section whose when() throws, and keeps the rest of the list', () => {
    register('ext', [
      section('broken', {
        after: 'database',
        when: () => {
          throw new Error('when failed');
        }
      }),
      section('fine', { after: 'database' })
    ]);

    expect(ids()).toEqual(['database', 'fine', 'display', 'import', 'integrations', 'labs', 'about']);
  });
});

describe('isSettingsCategoryVisible', () => {
  it('is true for every listed core category', () => {
    for (const id of CORE_OFF) expect(isSettingsCategoryVisible(id)).toBe(true);
  });

  it('is false for a Labs-gated category while its flag is off, true once on', () => {
    expect(isSettingsCategoryVisible('ai-models')).toBe(false);
    expect(isSettingsCategoryVisible('playbooks')).toBe(false);

    setAllLabsFlags(true);
    expect(isSettingsCategoryVisible('ai-models')).toBe(true);
    expect(isSettingsCategoryVisible('playbooks')).toBe(true);
  });

  it('is false for an id nothing registered', () => {
    expect(isSettingsCategoryVisible('no-such-category')).toBe(false);
    expect(isSettingsCategoryVisible('')).toBe(false);
  });

  it('is true for a shown section and false for a hidden one', () => {
    let shown = true;
    register('ext', [section('mine', { when: () => shown })]);

    expect(isSettingsCategoryVisible('mine')).toBe(true);

    shown = false;
    expect(isSettingsCategoryVisible('mine')).toBe(false);
  });
});

describe('findSettingsSection', () => {
  it('returns the keyed section for a shown section id', () => {
    register('ext', [section('mine')]);

    expect(findSettingsSection('mine')).toMatchObject({ id: 'mine', key: 'ext/mine' });
  });

  it('returns undefined for a core id, an unknown id and a hidden section', () => {
    register('ext', [section('mine', { when: () => false })]);

    expect(findSettingsSection('database')).toBeUndefined();
    expect(findSettingsSection('no-such-category')).toBeUndefined();
    expect(findSettingsSection('mine')).toBeUndefined();
  });
});
