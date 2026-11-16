/**
 * navigation-sidebar.svelte — putting one collection under another, taking it
 * out again, and renaming it, from the Collections list.
 *
 * A collection can be dragged onto another or moved from its row menu (the
 * keyboard route), taken out of a parent from the same menu, and renamed in
 * place. A refused move says why in words instead of doing nothing.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { render, cleanup, fireEvent, waitFor, within, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import { collectionService, type CollectionInfo } from '$lib/services/collection-service';
import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { databaseStore } from '$lib/stores/database.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';

function collection(
  id: string,
  name: string,
  parentCollectionIds: string[] = [],
  version = 1
): CollectionInfo {
  return {
    lifecycleStatus: 'active',
    id,
    content: name,
    nodeType: 'collection',
    createdAt: '',
    modifiedAt: '',
    version,
    properties: {},
    memberCount: 1,
    parentCollectionIds
  };
}

// DeutNet > Company Policies > HR > Onboarding (four levels), and Design on its own.
const COLLECTIONS = [
  collection('deutnet', 'DeutNet'),
  collection('company-policies', 'Company Policies', ['deutnet']),
  collection('hr', 'HR', ['company-policies'], 7),
  collection('onboarding', 'Onboarding', ['hr']),
  collection('design', 'Design')
];

/** The tree row whose name reads `name`. */
function row(container: HTMLElement, name: string): HTMLElement {
  const label = [...container.querySelectorAll<HTMLElement>('.collection-name-btn')].find(
    (el) => el.textContent?.trim() === name
  );
  const found = label?.closest<HTMLElement>('.collection-item');
  if (!found) throw new Error(`No collection row named "${name}"`);
  return found;
}

/** Every tree row whose name reads `name` (a collection in several parents has several). */
function rowsNamed(container: HTMLElement, name: string): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>('.collection-name-btn')]
    .filter((el) => el.textContent?.trim() === name)
    .map((el) => el.closest<HTMLElement>('.collection-item')!);
}

function rowNames(container: HTMLElement): string[] {
  return [...container.querySelectorAll('.collection-item .collection-name-btn')]
    .map((el) => el.textContent?.trim() ?? '')
    .filter((name) => name && name !== '+ New collection');
}

async function renderSidebar() {
  const view = render(NavigationSidebar);
  await waitFor(() => expect(row(view.container, 'Design')).toBeTruthy());
  return view;
}

/** Drag the row named `from` onto the row named `onto`. */
async function drag(container: HTMLElement, from: string, onto: string) {
  const source = row(container, from);
  const target = row(container, onto);
  await fireEvent.dragStart(source);
  await fireEvent.dragEnter(target);
  await fireEvent.dragOver(target);
  await fireEvent.drop(target);
  await fireEvent.dragEnd(source);
}

describe('NavigationSidebar — moving and renaming collections', () => {
  let add: MockInstance<typeof collectionService.addNodeToCollection>;
  let remove: MockInstance<typeof collectionService.removeNodeFromCollection>;
  let rename: MockInstance<typeof collectionService.renameCollection>;

  beforeEach(() => {
    vi.spyOn(databaseStore, 'load').mockResolvedValue();
    vi.spyOn(collectionService, 'getAllCollections').mockResolvedValue(COLLECTIONS);
    add = vi.spyOn(collectionService, 'addNodeToCollection').mockResolvedValue();
    remove = vi.spyOn(collectionService, 'removeNodeFromCollection').mockResolvedValue();
    rename = vi
      .spyOn(collectionService, 'renameCollection')
      .mockResolvedValue({} as Awaited<ReturnType<typeof collectionService.renameCollection>>);
    layoutStore.state = { ...layoutStore.state, sidebarCollapsed: false, collectionsExpanded: true };
    for (const id of ['deutnet', 'company-policies', 'hr']) {
      collectionsState.toggleCollectionExpanded(id);
    }
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    collectionsData.reset();
    collectionsState.reset();
    layoutStore.state = { ...layoutStore.state, collectionsExpanded: false };
    localStorage.clear();
  });

  it('shows collections nested deeper than three levels', async () => {
    const { container } = await renderSidebar();

    expect(rowNames(container)).toEqual([
      'Design',
      'DeutNet',
      'Company Policies',
      'HR',
      'Onboarding'
    ]);
  });

  describe('dragging a collection onto another', () => {
    it('adds the target as a parent and opens it', async () => {
      const { container } = await renderSidebar();
      // Onboarding is not open to start with (the others are).
      expect(collectionsState.state.expandedCollectionIds.has('onboarding')).toBe(false);

      await drag(container, 'Design', 'Onboarding');

      await waitFor(() => expect(add).toHaveBeenCalledExactlyOnceWith('design', 'onboarding'));
      await waitFor(() =>
        expect(collectionsState.state.expandedCollectionIds.has('onboarding')).toBe(true)
      );
      expect(container.querySelector('[role="alert"]')).toBeNull();
    });

    it('accepts the drop of a dragged collection, and only then', async () => {
      const { container } = await renderSidebar();
      const target = row(container, 'HR');

      // Nothing dragged: the row does not take the drop.
      expect(await fireEvent.dragOver(target)).toBe(true);
      await fireEvent.dragStart(row(container, 'Design'));
      // A collection dragged from the list: the drop is accepted (the event is cancelled).
      expect(await fireEvent.dragOver(target)).toBe(false);
      await fireEvent.dragEnd(row(container, 'Design'));
    });

    it('refuses DeutNet onto Company Policies with the reason, and calls nothing', async () => {
      const { container } = await renderSidebar();

      await drag(container, 'DeutNet', 'Company Policies');

      const alert = await waitFor(() => {
        const found = container.querySelector('[role="alert"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(alert.textContent).toContain(
        '“DeutNet” can\'t go into “Company Policies” because “Company Policies” is already inside “DeutNet”.'
      );
      expect(add).not.toHaveBeenCalled();
    });

    it('says why when the backend refuses, for example without rights on the target', async () => {
      add.mockRejectedValueOnce({
        message: 'Failed to add node to collection: You do not have access to “HR”',
        code: 'COLLECTION_ERROR'
      });
      const { container } = await renderSidebar();

      await drag(container, 'Design', 'HR');

      const alert = await waitFor(() => {
        const found = container.querySelector('[role="alert"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(alert.textContent).toContain(
        'Couldn\'t move “Design” into “HR”: You do not have access to “HR”'
      );
    });

    it('marks the row it is over as the drop target and clears it afterwards', async () => {
      const { container } = await renderSidebar();
      const target = row(container, 'HR');

      await fireEvent.dragStart(row(container, 'Design'));
      await fireEvent.dragOver(target);
      expect(target.classList.contains('drop-target')).toBe(true);
      await fireEvent.dragLeave(target);
      expect(target.classList.contains('drop-target')).toBe(false);
      await fireEvent.dragEnd(row(container, 'Design'));
    });

    it('does nothing for a drop of something that is not a collection from this list', async () => {
      const { container } = await renderSidebar();

      // Not handled (the event is not cancelled), nothing called, nothing said.
      expect(await fireEvent.drop(row(container, 'HR'))).toBe(true);

      expect(add).not.toHaveBeenCalled();
      expect(container.querySelector('[role="alert"]')).toBeNull();
    });

    it('can dismiss the message', async () => {
      const { container } = await renderSidebar();
      await drag(container, 'DeutNet', 'Company Policies');
      await waitFor(() => expect(container.querySelector('[role="alert"]')).toBeTruthy());

      await fireEvent.click(within(container).getByLabelText('Dismiss message'));

      expect(container.querySelector('[role="alert"]')).toBeNull();
    });
  });

  describe('the row menu', () => {
    async function openMenu(container: HTMLElement, name: string) {
      const user = userEvent.setup();
      await user.click(within(row(container, name)).getByLabelText(`Actions for ${name}`));
      return user;
    }

    it('offers Rename, Move to collection and one Remove from per parent', async () => {
      const { container } = await renderSidebar();
      await openMenu(container, 'HR');

      expect(await screen.findByRole('menuitem', { name: 'Rename' })).toBeTruthy();
      expect(screen.getByRole('menuitem', { name: 'Move to collection…' })).toBeTruthy();
      expect(screen.getByRole('menuitem', { name: 'Remove from Company Policies' })).toBeTruthy();
    });

    it('offers no Remove from for a collection that is in none', async () => {
      const { container } = await renderSidebar();
      await openMenu(container, 'Design');

      await screen.findByRole('menuitem', { name: 'Rename' });
      expect(screen.queryByRole('menuitem', { name: /^Remove from/ })).toBeNull();
    });

    it('takes the collection out of that parent', async () => {
      const { container } = await renderSidebar();
      const user = await openMenu(container, 'HR');

      await user.click(await screen.findByRole('menuitem', { name: 'Remove from Company Policies' }));

      await waitFor(() =>
        expect(remove).toHaveBeenCalledExactlyOnceWith('hr', 'company-policies')
      );
    });

    it('moves a collection through the dialog, listing only collections it can go into', async () => {
      const { container } = await renderSidebar();
      const user = await openMenu(container, 'Design');

      await user.click(await screen.findByRole('menuitem', { name: 'Move to collection…' }));

      const select = (await screen.findByLabelText('Move into')) as HTMLSelectElement;
      // Design is on its own: it can go into any other collection.
      expect([...select.options].map((o) => o.textContent?.trim())).toEqual([
        'Company Policies',
        'DeutNet',
        'HR',
        'Onboarding'
      ]);
      await user.selectOptions(select, 'hr');
      await user.click(screen.getByRole('button', { name: 'Move' }));

      await waitFor(() => expect(add).toHaveBeenCalledExactlyOnceWith('design', 'hr'));
    });

    it('leaves out of the dialog the collection itself, its parents and what is inside it', async () => {
      const { container } = await renderSidebar();
      const user = await openMenu(container, 'Company Policies');

      await user.click(await screen.findByRole('menuitem', { name: 'Move to collection…' }));

      const select = (await screen.findByLabelText('Move into')) as HTMLSelectElement;
      // Not DeutNet (its parent), not itself, not HR or Onboarding (inside it).
      expect([...select.options].map((o) => o.textContent?.trim())).toEqual(['Design']);
    });
  });

  describe('renaming in place', () => {
    function nameField(container: HTMLElement): HTMLInputElement {
      return container.querySelector<HTMLInputElement>('.collection-rename-input')!;
    }

    it('opens a field with the current name on a double click, and saves on Enter', async () => {
      const { container } = await renderSidebar();

      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = nameField(container);
      expect(field.value).toBe('HR');
      await fireEvent.input(field, { target: { value: '  People  ' } });
      await fireEvent.keyDown(field, { key: 'Enter' });

      // The collection's current version goes with the rename.
      await waitFor(() => expect(rename).toHaveBeenCalledExactlyOnceWith('hr', 7, 'People'));
      await waitFor(() => expect(nameField(container)).toBeNull());
    });

    it('opens from the row menu too', async () => {
      const { container } = await renderSidebar();
      const user = userEvent.setup();
      await user.click(within(row(container, 'HR')).getByLabelText('Actions for HR'));

      await user.click(await screen.findByRole('menuitem', { name: 'Rename' }));

      await waitFor(() => expect(nameField(container)).toBeTruthy());
      expect(nameField(container).value).toBe('HR');
    });

    it('abandons the rename on Escape, without a call', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = nameField(container);
      await fireEvent.input(field, { target: { value: 'People' } });

      await fireEvent.keyDown(field, { key: 'Escape' });

      expect(nameField(container)).toBeNull();
      expect(rename).not.toHaveBeenCalled();
    });

    it('abandons the rename when the field loses focus, without a call', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = nameField(container);
      await fireEvent.input(field, { target: { value: 'People' } });

      await fireEvent.blur(field);

      expect(nameField(container)).toBeNull();
      expect(rename).not.toHaveBeenCalled();
    });

    it('keeps the field open and says why when the name is taken', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = nameField(container);
      await fireEvent.input(field, { target: { value: 'design' } });

      await fireEvent.keyDown(field, { key: 'Enter' });

      const alert = await waitFor(() => {
        const found = container.querySelector('[role="alert"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(alert.textContent).toContain('A collection named “Design” already exists.');
      expect(nameField(container)).toBeTruthy();
      expect(nameField(container).value).toBe('design');
      expect(rename).not.toHaveBeenCalled();
    });

    it('shows the new name once the list reloads', async () => {
      const { container } = await renderSidebar();
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, content: 'People', version: 8 } : c))
      );
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = nameField(container);
      await fireEvent.input(field, { target: { value: 'People' } });
      await fireEvent.keyDown(field, { key: 'Enter' });

      await waitFor(() => expect(row(container, 'People')).toBeTruthy());
    });

    it('keeps the row menu in place, hidden, while the field is open', async () => {
      const { container } = await renderSidebar();

      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));

      // Unmounting the menu would return focus to its trigger and end the rename.
      const renamingRow = container
        .querySelector('.collection-rename-input')
        ?.closest<HTMLElement>('.collection-item');
      const trigger = within(renamingRow!).getByLabelText('Actions for HR');
      expect(trigger.closest('.collection-menu')?.classList.contains('hidden')).toBe(true);
    });
  });

  describe('a collection in several collections', () => {
    // Shared is in Alpha and in Beta, which are unrelated to each other.
    const SHARED = [
      collection('alpha', 'Alpha'),
      collection('beta', 'Beta'),
      collection('shared', 'Shared', ['alpha', 'beta'])
    ];

    it('is listed under each of its parents, with a Remove from entry for each', async () => {
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(SHARED);
      collectionsState.toggleCollectionExpanded('alpha');
      collectionsState.toggleCollectionExpanded('beta');
      const view = render(NavigationSidebar);
      await waitFor(() => expect(rowsNamed(view.container, 'Shared')).toHaveLength(2));

      const user = userEvent.setup();
      await user.click(within(rowsNamed(view.container, 'Shared')[0]).getByLabelText('Actions for Shared'));

      expect(await screen.findByRole('menuitem', { name: 'Remove from Alpha' })).toBeTruthy();
      expect(screen.getByRole('menuitem', { name: 'Remove from Beta' })).toBeTruthy();
    });

    it('shows a nested collection under the new parent after it is dragged there', async () => {
      const { container } = await renderSidebar();
      expect(rowsNamed(container, 'HR')).toHaveLength(1);
      // After the move the backend lists HR in Design as well as in Company Policies.
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, parentCollectionIds: ['company-policies', 'design'] } : c))
      );

      await drag(container, 'HR', 'Design');

      await waitFor(() => expect(add).toHaveBeenCalledExactlyOnceWith('hr', 'design'));
      await waitFor(() => expect(rowsNamed(container, 'HR')).toHaveLength(2));
      // The new parent is open, so the person sees it where they dropped it.
      expect(collectionsState.state.expandedCollectionIds.has('design')).toBe(true);
      expect(container.querySelector('[role="status"]')).toBeNull();
    });

    it('puts focus on the moved collection under the new parent, not on its first copy', async () => {
      const list = [
        collection('alpha', 'Alpha'),
        collection('gamma', 'Gamma'),
        collection('shared', 'Shared', ['alpha'])
      ];
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(list);
      collectionsState.toggleCollectionExpanded('alpha');
      const { container } = render(NavigationSidebar);
      await waitFor(() => expect(rowsNamed(container, 'Shared')).toHaveLength(1));
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        list.map((c) => (c.id === 'shared' ? { ...c, parentCollectionIds: ['alpha', 'gamma'] } : c))
      );

      await drag(container, 'Shared', 'Gamma');

      await waitFor(() => expect(rowsNamed(container, 'Shared')).toHaveLength(2));
      // Alpha > Shared comes first in the list; Gamma > Shared is the one just made.
      const [underAlpha, underGamma] = rowsNamed(container, 'Shared');
      await waitFor(() =>
        expect(document.activeElement).toBe(underGamma.querySelector('.collection-name-btn'))
      );
      expect(document.activeElement).not.toBe(underAlpha.querySelector('.collection-name-btn'));
    });

    it('says so when the new parent holds the collection only through a parent it already has', async () => {
      const { container } = await renderSidebar();
      // DeutNet is an ancestor of Company Policies: HR stays listed under Company Policies only.
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, parentCollectionIds: ['company-policies', 'deutnet'] } : c))
      );

      await drag(container, 'HR', 'DeutNet');

      const notice = await waitFor(() => {
        const found = container.querySelector('[role="status"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(notice.textContent).toContain(
        '“HR” is now also in “DeutNet”. It stays listed under “Company Policies”, which is inside “DeutNet”.'
      );
      expect(rowsNamed(container, 'HR')).toHaveLength(1);
    });

    it('says so when a collection is taken out of a parent it was not listed under', async () => {
      const both = COLLECTIONS.map((c) =>
        c.id === 'hr' ? { ...c, parentCollectionIds: ['company-policies', 'deutnet'] } : c
      );
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(both);
      const view = render(NavigationSidebar);
      await waitFor(() => expect(row(view.container, 'HR')).toBeTruthy());
      const user = userEvent.setup();
      await user.click(within(row(view.container, 'HR')).getByLabelText('Actions for HR'));

      await user.click(await screen.findByRole('menuitem', { name: 'Remove from DeutNet' }));

      const notice = await waitFor(() => {
        const found = view.container.querySelector('[role="status"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(notice.textContent).toContain(
        '“HR” is no longer in “DeutNet”. It was listed under “Company Policies”, which is inside “DeutNet”, so the list looks the same.'
      );
      expect(remove).toHaveBeenCalledExactlyOnceWith('hr', 'deutnet');
    });
  });

  describe('drag state', () => {
    it('is cleared when the dragged row disappears from the list mid-drag', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dragStart(row(container, 'Design'));
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.filter((c) => c.id !== 'design')
      );

      await collectionsData.loadCollections();
      await waitFor(() => expect(rowsNamed(container, 'Design')).toHaveLength(0));

      // Nothing is being dragged any more: a row does not take a drop.
      expect(await fireEvent.dragOver(row(container, 'HR'))).toBe(true);
      expect(container.querySelector('.drop-target')).toBeNull();
    });

    it('is cleared when the drag ends without a drop', async () => {
      const { container } = await renderSidebar();
      const source = row(container, 'Design');

      await fireEvent.dragStart(source);
      expect(source.classList.contains('dragging')).toBe(true);
      await fireEvent.dragEnd(source);

      expect(source.classList.contains('dragging')).toBe(false);
      expect(await fireEvent.dragOver(row(container, 'HR'))).toBe(true);
    });
  });

  describe('renaming against a stale version', () => {
    it('says the collection changed, refreshes the list and lets the retry go through with the new version', async () => {
      rename.mockRejectedValueOnce({
        message: 'Failed to rename collection: Version conflict on hr: expected 7, got 8',
        code: 'UPDATE_ERROR',
        details: 'Aborted'
      });
      const { container } = await renderSidebar();
      // What the reload after the failure returns: HR at its current version.
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, version: 8 } : c))
      );
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = container.querySelector<HTMLInputElement>('.collection-rename-input')!;
      await fireEvent.input(field, { target: { value: 'People' } });

      await fireEvent.keyDown(field, { key: 'Enter' });

      const alert = await waitFor(() => {
        const found = container.querySelector('[role="alert"]');
        expect(found).toBeTruthy();
        return found as HTMLElement;
      });
      expect(alert.textContent).toContain('“HR” was changed somewhere else.');
      // The field is still there, with what was typed, and has focus.
      const open = container.querySelector<HTMLInputElement>('.collection-rename-input')!;
      expect(open.value).toBe('People');
      await waitFor(() => expect(document.activeElement).toBe(open));

      await fireEvent.keyDown(open, { key: 'Enter' });

      await waitFor(() => expect(rename).toHaveBeenCalledTimes(2));
      expect(rename).toHaveBeenNthCalledWith(1, 'hr', 7, 'People');
      expect(rename).toHaveBeenNthCalledWith(2, 'hr', 8, 'People');
    });
  });

  describe('focus', () => {
    it('lands on the collection name after Enter saves a rename', async () => {
      const { container } = await renderSidebar();
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, content: 'People', version: 8 } : c))
      );
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = container.querySelector<HTMLInputElement>('.collection-rename-input')!;
      await fireEvent.input(field, { target: { value: 'People' } });

      await fireEvent.keyDown(field, { key: 'Enter' });

      await waitFor(() =>
        expect(document.activeElement).toBe(
          within(row(container, 'People')).getByText('People').closest('button')
        )
      );
    });

    it('lands on the collection, or its parent, after Remove from', async () => {
      const { container } = await renderSidebar();
      // After the removal HR is on its own at the top level.
      vi.mocked(collectionService.getAllCollections).mockResolvedValue(
        COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, parentCollectionIds: [] } : c))
      );
      const user = userEvent.setup();
      await user.click(within(row(container, 'HR')).getByLabelText('Actions for HR'));

      await user.click(await screen.findByRole('menuitem', { name: 'Remove from Company Policies' }));

      await waitFor(() =>
        expect(document.activeElement).toBe(
          within(row(container, 'HR')).getByText('HR').closest('button')
        )
      );
    });
  });

  describe('dismissing a message', () => {
    it('does not close a rename field the message left open', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = container.querySelector<HTMLInputElement>('.collection-rename-input')!;
      await fireEvent.input(field, { target: { value: 'design' } });
      await fireEvent.keyDown(field, { key: 'Enter' });
      await waitFor(() => expect(container.querySelector('[role="alert"]')).toBeTruthy());

      const user = userEvent.setup();
      await user.click(within(container).getByLabelText('Dismiss message'));

      expect(container.querySelector('[role="alert"]')).toBeNull();
      expect(container.querySelector('.collection-rename-input')).toBeTruthy();
    });
  });

  describe('the tree for assistive technology', () => {
    it('is a labelled tree of treeitems with levels, open state and selection', async () => {
      const { container } = await renderSidebar();

      const tree = container.querySelector('[role="tree"]')!;
      expect(tree.getAttribute('aria-label')).toBe('Collections');
      const items = [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')];
      expect(items.map((el) => [el.dataset.collectionId, el.getAttribute('aria-level')])).toEqual([
        ['design', '1'],
        ['deutnet', '1'],
        ['company-policies', '2'],
        ['hr', '3'],
        ['onboarding', '4']
      ]);
      // Open state only on a collection that has collections in it.
      expect(row(container, 'DeutNet').getAttribute('aria-expanded')).toBe('true');
      expect(row(container, 'Onboarding').hasAttribute('aria-expanded')).toBe(false);
      expect(row(container, 'Design').hasAttribute('aria-expanded')).toBe(false);
      // The form and the messages are not tree children.
      expect(tree.querySelector('.new-collection-btn')).toBeNull();

      await fireEvent.click(within(row(container, 'Design')).getByText('Design'));
      await waitFor(() => expect(row(container, 'Design').getAttribute('aria-selected')).toBe('true'));
      expect(row(container, 'HR').getAttribute('aria-selected')).toBe('false');
    });

    it('moves focus with the arrow keys, opens and closes with Left and Right', async () => {
      const { container } = await renderSidebar();
      const nameOf = (name: string) => within(row(container, name)).getByText(name).closest('button')!;
      nameOf('Design').focus();

      await fireEvent.keyDown(nameOf('Design'), { key: 'ArrowDown' });
      expect(document.activeElement).toBe(nameOf('DeutNet'));
      await fireEvent.keyDown(nameOf('DeutNet'), { key: 'ArrowRight' }); // open: enters it
      expect(document.activeElement).toBe(nameOf('Company Policies'));
      await fireEvent.keyDown(nameOf('Company Policies'), { key: 'ArrowLeft' }); // open: closes it
      await waitFor(() => expect(rowsNamed(container, 'HR')).toHaveLength(0));
      await fireEvent.keyDown(nameOf('Company Policies'), { key: 'ArrowLeft' }); // closed: goes to parent
      expect(document.activeElement).toBe(nameOf('DeutNet'));
      await fireEvent.keyDown(nameOf('DeutNet'), { key: 'End' });
      expect(document.activeElement).toBe(nameOf('Company Policies'));
      await fireEvent.keyDown(nameOf('Company Policies'), { key: 'Home' });
      expect(document.activeElement).toBe(nameOf('Design'));
      await fireEvent.keyDown(nameOf('Design'), { key: 'ArrowUp' });
      expect(document.activeElement).toBe(nameOf('Design'));
      await fireEvent.keyDown(nameOf('Design'), { key: 'ArrowRight' }); // a leaf: nothing
      expect(document.activeElement).toBe(nameOf('Design'));
    });

    it('leaves the arrow keys alone inside the rename field', async () => {
      const { container } = await renderSidebar();
      await fireEvent.dblClick(within(row(container, 'HR')).getByText('HR'));
      const field = container.querySelector<HTMLInputElement>('.collection-rename-input')!;
      field.focus();

      const notHandled = await fireEvent.keyDown(field, { key: 'ArrowDown' });

      expect(notHandled).toBe(true);
      expect(document.activeElement).toBe(field);
    });
  });

  describe('work per render', () => {
    it("works out each collection's parents once per change of the list, not once per render", async () => {
      const parentsOf = vi.spyOn(collectionsData, 'parentsOf');
      const { container } = await renderSidebar();
      const afterFirstRender = parentsOf.mock.calls.length;
      expect(afterFirstRender).toBeLessThanOrEqual(COLLECTIONS.length * 2);

      // Opening and closing rows re-renders the tree but does not change the list.
      collectionsState.toggleCollectionExpanded('deutnet');
      collectionsState.toggleCollectionExpanded('deutnet');
      await waitFor(() => expect(row(container, 'DeutNet')).toBeTruthy());

      expect(parentsOf.mock.calls.length).toBe(afterFirstRender);
    });
  });

  describe('a collection that is still being created', () => {
    it('cannot be renamed, moved or dragged yet', async () => {
      const { container } = await renderSidebar();
      collectionsData.state = {
        ...collectionsData.state,
        collections: [...collectionsData.state.collections, collection('pending-collection-1', 'Draft')],
        locallyCreatedIds: new Set(['pending-collection-1']),
        pendingIds: new Set(['pending-collection-1'])
      };
      await waitFor(() => expect(row(container, 'Draft')).toBeTruthy());
      const draft = row(container, 'Draft');

      await fireEvent.dblClick(within(draft).getByText('Draft'));

      expect(container.querySelector('.collection-rename-input')).toBeNull();
      expect(within(draft).queryByLabelText('Actions for Draft')).toBeNull();
      expect(draft.getAttribute('draggable')).toBe('false');
    });
  });
});
