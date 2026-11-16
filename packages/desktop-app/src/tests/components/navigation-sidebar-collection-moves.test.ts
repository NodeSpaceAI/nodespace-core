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
      expect(collectionsState.state.expandedCollectionIds.has('design')).toBe(false);

      await drag(container, 'Design', 'HR');

      await waitFor(() => expect(add).toHaveBeenCalledExactlyOnceWith('design', 'hr'));
      expect(collectionsState.state.expandedCollectionIds.has('hr')).toBe(true);
      expect(container.querySelector('[role="alert"]')).toBeNull();
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

      await fireEvent.drop(row(container, 'HR'));

      expect(add).not.toHaveBeenCalled();
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
  });
});
