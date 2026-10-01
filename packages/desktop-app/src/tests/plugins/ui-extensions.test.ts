/**
 * UI-extension registry: the generic contract of the frontend extension API.
 *
 * Registration rules (never throws; duplicates, version mismatches and repeated
 * contribution ids are logged and dropped), keyed and ordered lookups, and the
 * `when()` predicates the reactive wrapper filters by. The class tests use their
 * own registry instances; `registerExtensions` and the wrapper accessors use the
 * process-wide singleton with the fixture extension, unregistered after each test.
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
  EXTENSION_API_VERSION,
  UiExtensionRegistry,
  registerExtensions,
  uiExtensionRegistry,
  type ChromeContribution,
  type NodespaceExtension,
  type SettingsSectionContribution,
  type SettingsSlotContribution,
  type ViewerTabContribution
} from '$lib/plugins/ui-extensions';
import {
  getActiveChromeContributions,
  getActiveSettingsSections,
  getActiveSettingsSlot,
  getActiveViewerTabs,
  isContributionActive
} from '$lib/plugins/ui-extensions.svelte';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';

const noComponent = () => Promise.reject(new Error('not loaded in this test'));

function chrome(
  id: string,
  extra: Partial<ChromeContribution> = {}
): ChromeContribution {
  return { id, slot: 'app-shell-modal', load: noComponent, ...extra };
}

function tab(id: string, extra: Partial<ViewerTabContribution> = {}): ViewerTabContribution {
  return { id, nodeType: 'collection', label: id, load: noComponent, ...extra };
}

function section(
  id: string,
  extra: Partial<SettingsSectionContribution> = {}
): SettingsSectionContribution {
  return { id, label: id, load: noComponent, ...extra };
}

function slotContribution(
  id: string,
  slot: SettingsSlotContribution['slot'],
  extra: { priority?: number; when?: () => boolean } = {}
): SettingsSlotContribution {
  return { id, slot, load: noComponent, ...extra } as SettingsSlotContribution;
}

function ext(id: string, rest: Partial<NodespaceExtension> = {}): NodespaceExtension {
  return { id, apiVersion: 1, ...rest };
}

const keysOf = (list: { key: string }[]) => list.map((c) => c.key);

beforeEach(() => {
  log.debug.mockClear();
  log.info.mockClear();
  log.warn.mockClear();
  log.error.mockClear();
});

describe('UiExtensionRegistry registration', () => {
  let registry: UiExtensionRegistry;

  beforeEach(() => {
    registry = new UiExtensionRegistry();
  });

  it('registers, reports and unregisters an extension by id', () => {
    const a = ext('a');
    registry.register(a);

    expect(registry.has('a')).toBe(true);
    expect(registry.all()).toEqual([a]);

    registry.unregister('a');
    expect(registry.has('a')).toBe(false);
    expect(registry.all()).toEqual([]);
  });

  it('lists extensions in registration order', () => {
    const [a, b, c] = [ext('a'), ext('b'), ext('c')];
    registry.register(b);
    registry.register(a);
    registry.register(c);

    expect(registry.all()).toEqual([b, a, c]);
  });

  it('treats re-registering the identical object as a silent no-op', () => {
    const a = ext('a', { chrome: [chrome('one')] });
    registry.register(a);
    registry.register(a);

    expect(registry.all()).toEqual([a]);
    expect(registry.chromeFor('app-shell-modal')).toHaveLength(1);
    expect(log.error).not.toHaveBeenCalled();
    expect(log.warn).not.toHaveBeenCalled();
    expect(log.debug).toHaveBeenCalledWith('Extension already registered', { id: 'a' });
  });

  it('keeps the first extension when a different object reuses its id, and logs', () => {
    const first = ext('a', { chrome: [chrome('first')] });
    const second = ext('a', { chrome: [chrome('second')] });
    registry.register(first);
    registry.register(second);

    expect(registry.all()).toEqual([first]);
    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/first']);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('Duplicate extension id'), {
      id: 'a'
    });
  });

  it('does not register an extension whose apiVersion is not the supported major, and logs', () => {
    const wrong = { ...ext('a', { chrome: [chrome('one')] }), apiVersion: 2 } as never;
    registry.register(wrong);

    expect(registry.has('a')).toBe(false);
    expect(registry.chromeFor('app-shell-modal')).toEqual([]);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(
      expect.stringContaining('apiVersion mismatch'),
      expect.objectContaining({ id: 'a', apiVersion: 2, supported: EXTENSION_API_VERSION.major })
    );
  });

  it('drops a repeated contribution id within one list, keeping the first, and logs', () => {
    registry.register(
      ext('a', {
        chrome: [
          chrome('dup', { slot: 'app-shell-overlay' }),
          chrome('dup', { slot: 'app-shell-modal' }),
          chrome('other')
        ]
      })
    );

    expect(keysOf(registry.chromeFor('app-shell-overlay'))).toEqual(['a/dup']);
    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/other']);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('Duplicate contribution id'), {
      extensionId: 'a',
      contributionId: 'dup'
    });
  });

  it('treats a contribution id as unique across the extension’s lists', () => {
    registry.register(ext('a', { chrome: [chrome('shared')], viewerTabs: [tab('shared')] }));

    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/shared']);
    expect(registry.viewerTabsFor('collection')).toEqual([]);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('Duplicate contribution id'), {
      extensionId: 'a',
      contributionId: 'shared'
    });
  });

  it('lets two extensions use the same contribution id, since keys carry the extension id', () => {
    registry.register(ext('a', { chrome: [chrome('same')] }));
    registry.register(ext('b', { chrome: [chrome('same')] }));

    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/same', 'b/same']);
    expect(log.error).not.toHaveBeenCalled();
  });

  it('drops a contribution whose key another extension already holds (ids may contain "/")', () => {
    registry.register(ext('a/b', { chrome: [chrome('c')] }));
    registry.register(ext('a', { chrome: [chrome('b/c'), chrome('other')] }));

    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/b/c', 'a/other']);
    expect(registry.has('a')).toBe(true);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('already held'), {
      key: 'a/b/c'
    });
  });

  it('never throws on malformed input, and logs it', () => {
    const malformed: unknown[] = [
      null,
      undefined,
      'a string',
      {},
      { id: '', apiVersion: 1 },
      { id: 'no-list', apiVersion: 1, chrome: 'not an array' },
      {
        id: 'bad-entries',
        apiVersion: 1,
        chrome: [
          null,
          { id: 'no-load', slot: 'app-shell-modal' },
          { slot: 'app-shell-modal', load: noComponent },
          { id: '', slot: 'app-shell-modal', load: noComponent }
        ]
      }
    ];

    for (const value of malformed) {
      expect(() => registry.register(value as NodespaceExtension)).not.toThrow();
    }

    // The malformed contributions are dropped; the extension around them is kept.
    expect(registry.chromeFor('app-shell-modal')).toEqual([]);
    expect(registry.has('bad-entries')).toBe(true);
    expect(log.error).toHaveBeenCalled();
  });

  it('never throws when a contribution getter throws', () => {
    const hostile = {
      id: 'hostile',
      apiVersion: 1,
      get chrome(): ChromeContribution[] {
        throw new Error('getter failed');
      }
    } as NodespaceExtension;

    expect(() => registry.register(hostile)).not.toThrow();
    expect(registry.has('hostile')).toBe(false);
    expect(log.error).toHaveBeenCalledWith(
      expect.stringContaining('registration failed'),
      expect.anything()
    );
  });
});

describe('UiExtensionRegistry settings contributions', () => {
  let registry: UiExtensionRegistry;

  beforeEach(() => {
    registry = new UiExtensionRegistry();
  });

  it('lists sections in registration order, ignoring priority', () => {
    registry.register(
      ext('a', { settingsSections: [section('a-low'), section('a-high', { priority: 9 })] })
    );
    registry.register(ext('b', { settingsSections: [section('b-negative', { priority: -1 })] }));

    expect(keysOf(registry.settingsSections())).toEqual(['a/a-low', 'a/a-high', 'b/b-negative']);
  });

  it('keys a section as <extension id>/<section id> and keeps its label and anchor', () => {
    registry.register(
      ext('ext', { settingsSections: [section('one', { label: 'One', after: 'display' })] })
    );

    expect(registry.settingsSections()[0]).toMatchObject({
      id: 'one',
      key: 'ext/one',
      extensionId: 'ext',
      label: 'One',
      after: 'display'
    });
  });

  it('selects slot contributions by slot', () => {
    registry.register(
      ext('a', {
        settingsSlots: [
          slotContribution('action', 'database.actions'),
          slotContribution('row', 'database.row')
        ]
      })
    );

    expect(keysOf(registry.settingsSlotFor('database.actions'))).toEqual(['a/action']);
    expect(keysOf(registry.settingsSlotFor('database.row'))).toEqual(['a/row']);
  });

  it('orders a slot by descending priority, ties in registration order', () => {
    registry.register(
      ext('a', {
        settingsSlots: [
          slotContribution('a-plain', 'database.row'),
          slotContribution('a-boosted', 'database.row', { priority: 4 })
        ]
      })
    );
    registry.register(
      ext('b', {
        settingsSlots: [
          slotContribution('b-boosted', 'database.row', { priority: 4 }),
          slotContribution('b-negative', 'database.row', { priority: -2 })
        ]
      })
    );

    expect(keysOf(registry.settingsSlotFor('database.row'))).toEqual([
      'a/a-boosted',
      'b/b-boosted',
      'a/a-plain',
      'b/b-negative'
    ]);
  });

  it('drops a repeated id, keeping the first, across sections and slots', () => {
    registry.register(
      ext('a', {
        settingsSections: [section('shared')],
        settingsSlots: [slotContribution('shared', 'database.actions')]
      })
    );

    expect(keysOf(registry.settingsSections())).toEqual(['a/shared']);
    expect(registry.settingsSlotFor('database.actions')).toEqual([]);
    expect(log.error).toHaveBeenCalledTimes(1);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('Duplicate contribution id'), {
      extensionId: 'a',
      contributionId: 'shared'
    });
  });

  it('lets two extensions use the same section id; both are listed, each under its own key', () => {
    registry.register(ext('a', { settingsSections: [section('same')] }));
    registry.register(ext('b', { settingsSections: [section('same')] }));

    expect(keysOf(registry.settingsSections())).toEqual(['a/same', 'b/same']);
    expect(log.error).not.toHaveBeenCalled();
  });

  it('drops a section or slot whose key another extension already holds', () => {
    registry.register(ext('a/b', { settingsSections: [section('c')] }));
    registry.register(
      ext('a', {
        settingsSections: [section('b/c'), section('other')],
        settingsSlots: [slotContribution('b/c', 'database.row')]
      })
    );

    expect(keysOf(registry.settingsSections())).toEqual(['a/b/c', 'a/other']);
    expect(registry.settingsSlotFor('database.row')).toEqual([]);
    expect(log.error).toHaveBeenCalledWith(expect.stringContaining('already held'), {
      key: 'a/b/c'
    });
  });

  it('drops malformed entries and non-array lists without throwing', () => {
    const malformed = {
      id: 'bad',
      apiVersion: 1,
      settingsSections: [null, { id: 'no-load', label: 'x' }, { label: 'no-id', load: noComponent }],
      settingsSlots: 'not an array'
    } as unknown as NodespaceExtension;

    expect(() => registry.register(malformed)).not.toThrow();

    expect(registry.has('bad')).toBe(true);
    expect(registry.settingsSections()).toEqual([]);
    expect(registry.settingsSlotFor('database.actions')).toEqual([]);
    expect(log.error).toHaveBeenCalled();
  });

  it('forgets an extension’s sections and slots when it is unregistered', () => {
    registry.register(
      ext('a', {
        settingsSections: [section('one')],
        settingsSlots: [slotContribution('row', 'database.row')]
      })
    );
    registry.unregister('a');

    expect(registry.settingsSections()).toEqual([]);
    expect(registry.settingsSlotFor('database.row')).toEqual([]);
  });

  it('never evaluates when()', () => {
    const when = vi.fn(() => true);
    registry.register(
      ext('a', {
        settingsSections: [section('one', { when })],
        settingsSlots: [slotContribution('row', 'database.row', { when })]
      })
    );

    registry.settingsSections();
    registry.settingsSlotFor('database.row');

    expect(when).not.toHaveBeenCalled();
  });
});

describe('registerExtensions', () => {
  afterEach(() => {
    uiExtensionRegistry.unregister('reg-a');
    uiExtensionRegistry.unregister('reg-b');
  });

  it('registers each extension in order', () => {
    const [a, b] = [ext('reg-a'), ext('reg-b')];
    registerExtensions([a, b]);

    const ids = uiExtensionRegistry.all().map((e) => e.id);
    expect(ids.indexOf('reg-a')).toBeGreaterThanOrEqual(0);
    expect(ids.indexOf('reg-a')).toBeLessThan(ids.indexOf('reg-b'));
  });

  it('logs and ignores a non-array argument', () => {
    const before = uiExtensionRegistry.all().length;

    expect(() => registerExtensions(ext('reg-a') as never)).not.toThrow();
    expect(() => registerExtensions(undefined as never)).not.toThrow();

    expect(uiExtensionRegistry.all()).toHaveLength(before);
    expect(uiExtensionRegistry.has('reg-a')).toBe(false);
    expect(log.error).toHaveBeenCalledTimes(2);
  });

  it('skips a bad extension without stopping the rest', () => {
    registerExtensions([null as never, ext('reg-a')]);

    expect(uiExtensionRegistry.has('reg-a')).toBe(true);
    expect(log.error).toHaveBeenCalledTimes(1);
  });
});

describe('UiExtensionRegistry lookups', () => {
  let registry: UiExtensionRegistry;

  beforeEach(() => {
    registry = new UiExtensionRegistry();
  });

  it('selects chrome by slot and viewer tabs by node type', () => {
    registry.register(
      ext('a', {
        chrome: [
          chrome('overlay', { slot: 'app-shell-overlay' }),
          chrome('modal', { slot: 'app-shell-modal' })
        ],
        viewerTabs: [tab('for-collection'), tab('for-text', { nodeType: 'text' })]
      })
    );

    expect(keysOf(registry.chromeFor('app-shell-overlay'))).toEqual(['a/overlay']);
    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/modal']);
    expect(keysOf(registry.viewerTabsFor('collection'))).toEqual(['a/for-collection']);
    expect(keysOf(registry.viewerTabsFor('text'))).toEqual(['a/for-text']);
    expect(registry.viewerTabsFor('date')).toEqual([]);
  });

  it('keys each contribution as <extension id>/<contribution id> and carries the extension id', () => {
    registry.register(ext('ext', { chrome: [chrome('c')], viewerTabs: [tab('t')] }));

    const [c] = registry.chromeFor('app-shell-modal');
    expect(c).toMatchObject({ id: 'c', key: 'ext/c', extensionId: 'ext', slot: 'app-shell-modal' });
    const [t] = registry.viewerTabsFor('collection');
    expect(t).toMatchObject({ id: 't', key: 'ext/t', extensionId: 'ext', label: 't' });
  });

  it('orders by descending priority, breaking ties by registration order', () => {
    registry.register(
      ext('a', { chrome: [chrome('a-low'), chrome('a-high', { priority: 5 })] })
    );
    registry.register(
      ext('b', { chrome: [chrome('b-high', { priority: 5 }), chrome('b-negative', { priority: -1 })] })
    );

    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual([
      'a/a-high',
      'b/b-high',
      'a/a-low',
      'b/b-negative'
    ]);
  });

  it('orders viewer tabs by the same rule', () => {
    registry.register(ext('a', { viewerTabs: [tab('first'), tab('boosted', { priority: 3 })] }));
    registry.register(ext('b', { viewerTabs: [tab('second')] }));

    expect(keysOf(registry.viewerTabsFor('collection'))).toEqual([
      'a/boosted',
      'a/first',
      'b/second'
    ]);
  });

  it('treats a non-finite priority as 0 so the order stays deterministic', () => {
    registry.register(
      ext('a', {
        chrome: [
          chrome('nan', { priority: Number.NaN }),
          chrome('one', { priority: 1 }),
          chrome('zero')
        ]
      })
    );

    expect(keysOf(registry.chromeFor('app-shell-modal'))).toEqual(['a/one', 'a/nan', 'a/zero']);
  });

  it('never evaluates when()', () => {
    const when = vi.fn(() => true);
    registry.register(ext('a', { chrome: [chrome('c', { when })], viewerTabs: [tab('t', { when })] }));

    registry.chromeFor('app-shell-modal');
    registry.viewerTabsFor('collection');

    expect(when).not.toHaveBeenCalled();
  });
});

describe('when() predicates', () => {
  it('a contribution without when() is active', () => {
    expect(isContributionActive({ key: 'predicate/absent' })).toBe(true);
  });

  it('follows the value of when()', () => {
    expect(isContributionActive({ key: 'predicate/true', when: () => true })).toBe(true);
    expect(isContributionActive({ key: 'predicate/false', when: () => false })).toBe(false);
  });

  it('a throwing when() is false and warns once per key', () => {
    const when = () => {
      throw new Error('predicate failed');
    };
    const c = { key: 'predicate/throws-once', when };

    expect(isContributionActive(c)).toBe(false);
    expect(isContributionActive(c)).toBe(false);
    expect(isContributionActive(c)).toBe(false);

    expect(log.warn).toHaveBeenCalledTimes(1);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('when() threw'),
      expect.objectContaining({ key: 'predicate/throws-once' })
    );
  });

  it('warns again only after the predicate has returned normally in between', () => {
    let mode: 'throw' | 'ok' = 'throw';
    const c = {
      key: 'predicate/recovers',
      when: () => {
        if (mode === 'throw') throw new Error('predicate failed');
        return true;
      }
    };

    expect(isContributionActive(c)).toBe(false);
    expect(isContributionActive(c)).toBe(false);
    expect(log.warn).toHaveBeenCalledTimes(1);

    mode = 'ok';
    expect(isContributionActive(c)).toBe(true);

    mode = 'throw';
    expect(isContributionActive(c)).toBe(false);
    expect(isContributionActive(c)).toBe(false);
    expect(log.warn).toHaveBeenCalledTimes(2);
  });

  it('warns separately for each key', () => {
    const when = () => {
      throw new Error('predicate failed');
    };

    isContributionActive({ key: 'predicate/key-one', when });
    isContributionActive({ key: 'predicate/key-two', when });

    expect(log.warn).toHaveBeenCalledTimes(2);
  });
});

describe('active contribution accessors over the fixture extension', () => {
  beforeEach(() => {
    uiExtensionRegistry.register(createTestExtension());
  });

  afterEach(() => {
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('registers the fixture under its id', () => {
    expect(uiExtensionRegistry.has(TEST_EXTENSION_ID)).toBe(true);
  });

  it('returns only contributions whose when() holds', () => {
    expect(getActiveChromeContributions('app-shell-overlay')).toEqual([]);
    expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
    expect(getActiveViewerTabs('collection')).toEqual([]);

    testExtensionFlags.overlay = true;
    testExtensionFlags.modal = true;
    testExtensionFlags.tab = true;

    expect(keysOf(getActiveChromeContributions('app-shell-overlay'))).toEqual([
      `${TEST_EXTENSION_ID}/overlay`
    ]);
    expect(keysOf(getActiveChromeContributions('app-shell-modal'))).toEqual([
      `${TEST_EXTENSION_ID}/modal`
    ]);
    expect(keysOf(getActiveViewerTabs('collection'))).toEqual([`${TEST_EXTENSION_ID}/tab`]);

    testExtensionFlags.modal = false;
    expect(getActiveChromeContributions('app-shell-modal')).toEqual([]);
  });

  it('orders active chrome by priority', () => {
    testExtensionFlags.modal = true;
    testExtensionFlags.modalSecondary = true;

    expect(keysOf(getActiveChromeContributions('app-shell-modal'))).toEqual([
      `${TEST_EXTENSION_ID}/modal-secondary`,
      `${TEST_EXTENSION_ID}/modal`
    ]);
  });

  it('filters out a contribution whose when() throws and keeps its siblings', () => {
    testExtensionFlags.modal = true;
    testExtensionFlags.throwingWhen = true;

    expect(keysOf(getActiveChromeContributions('app-shell-modal'))).toEqual([
      `${TEST_EXTENSION_ID}/modal`
    ]);
    getActiveChromeContributions('app-shell-modal');
    expect(log.warn).toHaveBeenCalledTimes(1);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('when() threw'),
      expect.objectContaining({ key: `${TEST_EXTENSION_ID}/throwing-when` })
    );

    // Back to normal, then failing again: warned a second time.
    testExtensionFlags.throwingWhen = false;
    getActiveChromeContributions('app-shell-modal');
    testExtensionFlags.throwingWhen = true;
    getActiveChromeContributions('app-shell-modal');
    expect(log.warn).toHaveBeenCalledTimes(2);
  });
});

describe('active settings accessors over the fixture extension', () => {
  beforeEach(() => {
    uiExtensionRegistry.register(createTestExtension());
  });

  afterEach(() => {
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('returns a section only while its when() holds', () => {
    expect(getActiveSettingsSections()).toEqual([]);

    testExtensionFlags.section = true;
    expect(keysOf(getActiveSettingsSections())).toEqual([`${TEST_EXTENSION_ID}/test-section`]);

    testExtensionFlags.section = false;
    expect(getActiveSettingsSections()).toEqual([]);
  });

  it('returns slot contributions only while their when() holds, per slot', () => {
    expect(getActiveSettingsSlot('database.actions')).toEqual([]);
    expect(getActiveSettingsSlot('database.row')).toEqual([]);

    testExtensionFlags.databaseActions = true;
    expect(keysOf(getActiveSettingsSlot('database.actions'))).toEqual([
      `${TEST_EXTENSION_ID}/database-action`
    ]);
    expect(getActiveSettingsSlot('database.row')).toEqual([]);

    testExtensionFlags.databaseRow = true;
    expect(keysOf(getActiveSettingsSlot('database.row'))).toEqual([
      `${TEST_EXTENSION_ID}/database-row`
    ]);
  });

  it('filters out a section whose when() throws, and warns once', () => {
    const throwing = ext('throwing-section', {
      settingsSections: [
        section('boom', {
          when: () => {
            throw new Error('when failed');
          }
        })
      ]
    });
    uiExtensionRegistry.register(throwing);
    testExtensionFlags.section = true;

    try {
      expect(keysOf(getActiveSettingsSections())).toEqual([`${TEST_EXTENSION_ID}/test-section`]);
      getActiveSettingsSections();
      expect(log.warn).toHaveBeenCalledTimes(1);
      expect(log.warn).toHaveBeenCalledWith(
        expect.stringContaining('when() threw'),
        expect.objectContaining({ key: 'throwing-section/boom' })
      );
    } finally {
      uiExtensionRegistry.unregister('throwing-section');
    }
  });
});

describe('UiExtensionRegistry collection-tree roots', () => {
  let registry: UiExtensionRegistry;

  beforeEach(() => {
    registry = new UiExtensionRegistry();
  });

  const roots = (ids: readonly string[]) =>
    ext('roots-' + ids.join('-'), {
      collectionTreeRoots: () => ids
    });

  it('is empty with no extension registered', () => {
    expect([...registry.collectionTreeRoots()]).toEqual([]);
  });

  it('is empty when no extension contributes roots', () => {
    registry.register(ext('plain'));
    registry.register(ext('with-tabs', { viewerTabs: [tab('tab')] }));

    expect([...registry.collectionTreeRoots()]).toEqual([]);
    expect(log.warn).not.toHaveBeenCalled();
  });

  it("returns the union of every extension's roots in registration order", () => {
    registry.register(roots(['b', 'a']));
    registry.register(ext('plain'));
    registry.register(ext('overlap', { collectionTreeRoots: () => ['c', 'a', 'd'] }));

    expect([...registry.collectionTreeRoots()]).toEqual(['b', 'a', 'c', 'd']);
  });

  it('drops an extension’s roots once it is unregistered', () => {
    registry.register(ext('first', { collectionTreeRoots: () => ['one'] }));
    registry.register(ext('second', { collectionTreeRoots: () => ['two'] }));

    registry.unregister('first');

    expect([...registry.collectionTreeRoots()]).toEqual(['two']);
  });

  it('ignores entries that are not strings', () => {
    const mixed = ['kept', 7, null, undefined, { id: 'object' }, ['nested'], 'also-kept'];
    registry.register(
      ext('mixed', { collectionTreeRoots: () => mixed as unknown as readonly string[] })
    );

    expect([...registry.collectionTreeRoots()]).toEqual(['kept', 'also-kept']);
  });

  it('calls the contributor on every lookup, so a derivation re-runs on the state it reads', () => {
    let current: string[] = ['first'];
    const contributor = vi.fn(() => current);
    registry.register(ext('live', { collectionTreeRoots: contributor }));

    expect([...registry.collectionTreeRoots()]).toEqual(['first']);
    current = ['second'];
    expect([...registry.collectionTreeRoots()]).toEqual(['second']);
    expect(contributor).toHaveBeenCalledTimes(2);
  });

  it('calls the contributor as a method of its extension', () => {
    const method = ext('method-style', {
      collectionTreeRoots() {
        return [`${this === method ? 'bound' : 'unbound'}-root`];
      }
    } as Partial<NodespaceExtension>);
    registry.register(method);

    expect([...registry.collectionTreeRoots()]).toEqual(['bound-root']);
  });

  it('counts a throwing contributor as empty and still returns the others', () => {
    registry.register(roots(['before']));
    registry.register(
      ext('throws', {
        collectionTreeRoots: () => {
          throw new Error('roots failed');
        }
      })
    );
    registry.register(roots(['after']));

    expect([...registry.collectionTreeRoots()]).toEqual(['before', 'after']);
  });

  it('logs a throwing contributor once, however often it is read', () => {
    registry.register(
      ext('throws-once', {
        collectionTreeRoots: () => {
          throw new Error('roots failed');
        }
      })
    );

    registry.collectionTreeRoots();
    registry.collectionTreeRoots();
    registry.collectionTreeRoots();

    expect(log.warn).toHaveBeenCalledTimes(1);
    expect(log.warn).toHaveBeenCalledWith(
      expect.stringContaining('collectionTreeRoots'),
      expect.objectContaining({ extensionId: 'throws-once' })
    );
  });

  it('logs again only after the contributor has returned normally in between', () => {
    let mode: 'throw' | 'ok' = 'throw';
    registry.register(
      ext('recovers', {
        collectionTreeRoots: () => {
          if (mode === 'throw') throw new Error('roots failed');
          return ['back'];
        }
      })
    );

    registry.collectionTreeRoots();
    registry.collectionTreeRoots();
    expect(log.warn).toHaveBeenCalledTimes(1);

    mode = 'ok';
    expect([...registry.collectionTreeRoots()]).toEqual(['back']);

    mode = 'throw';
    registry.collectionTreeRoots();
    registry.collectionTreeRoots();
    expect(log.warn).toHaveBeenCalledTimes(2);
  });

  it('logs each failing extension separately', () => {
    const failing = (id: string) =>
      ext(id, {
        collectionTreeRoots: () => {
          throw new Error('roots failed');
        }
      });
    registry.register(failing('fails-one'));
    registry.register(failing('fails-two'));

    registry.collectionTreeRoots();
    registry.collectionTreeRoots();

    expect(log.warn).toHaveBeenCalledTimes(2);
  });

  it.each([
    ['undefined', undefined],
    ['null', null],
    ['a string', 'ab'],
    ['a Set', new Set(['a'])],
    ['an object', { 0: 'a', length: 1 }]
  ])('counts a contributor returning %s as empty, and logs it once', (_label, value) => {
    registry.register(
      ext('malformed', { collectionTreeRoots: () => value as unknown as readonly string[] })
    );

    expect([...registry.collectionTreeRoots()]).toEqual([]);
    registry.collectionTreeRoots();

    expect(log.warn).toHaveBeenCalledTimes(1);
  });

  it('counts a collectionTreeRoots that is not a function as empty and logs it once', () => {
    registry.register(
      ext('not-callable', {
        collectionTreeRoots: 'oops' as unknown as NodespaceExtension['collectionTreeRoots']
      })
    );

    expect([...registry.collectionTreeRoots()]).toEqual([]);
    registry.collectionTreeRoots();

    expect(log.warn).toHaveBeenCalledTimes(1);
  });

  it('counts a throwing collectionTreeRoots getter as empty', () => {
    const throwingGetter = ext('throwing-getter');
    Object.defineProperty(throwingGetter, 'collectionTreeRoots', {
      get() {
        throw new Error('getter failed');
      }
    });
    registry.register(throwingGetter);
    registry.register(roots(['kept']));

    expect([...registry.collectionTreeRoots()]).toEqual(['kept']);
    expect(log.warn).toHaveBeenCalledTimes(1);
  });

  it('does not evaluate when() or load() of any contribution', () => {
    const when = vi.fn(() => true);
    const load = vi.fn(noComponent);
    registry.register(
      ext('with-contributions', {
        chrome: [{ id: 'overlay', slot: 'app-shell-overlay', when, load }],
        collectionTreeRoots: () => ['root']
      })
    );

    registry.collectionTreeRoots();

    expect(when).not.toHaveBeenCalled();
    expect(load).not.toHaveBeenCalled();
  });

  describe('over the fixture extension', () => {
    afterEach(() => {
      uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
      resetTestExtension();
    });

    it('contributes nothing until the fixture flag is set', () => {
      uiExtensionRegistry.register(createTestExtension());

      expect([...uiExtensionRegistry.collectionTreeRoots()]).toEqual([]);
    });

    it('contributes the ids the fixture flag holds', () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.collectionTreeRoots = ['ext-root', 'second-root'];

      expect([...uiExtensionRegistry.collectionTreeRoots()]).toEqual(['ext-root', 'second-root']);
    });

    it('takes a contributor of its own through the overrides', () => {
      uiExtensionRegistry.register(createTestExtension({ collectionTreeRoots: () => ['custom'] }));

      expect([...uiExtensionRegistry.collectionTreeRoots()]).toEqual(['custom']);
    });
  });
});
