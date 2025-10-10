/**
 * Node Reference Autocomplete - Complete Flow Integration Tests
 *
 * Tests the full user flow from typing "@" to inserting a node reference.
 * Verifies integration between ContentEditableController, NodeAutocomplete,
 * BaseNode, and event dispatching.
 *
 * Test Coverage:
 * - Trigger detection (3 tests)
 * - Query filtering (3 tests)
 * - Selection flow (4 tests)
 * - Component event integration (3 tests)
 * - Focus management (2 tests)
 * - Content integrity (2 tests)
 *
 * Related: Issue #155
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { waitForEffects } from '../helpers';
import BaseNode from '$lib/design/components/base-node.svelte';

describe('Node Reference Autocomplete - Complete Flow', () => {
  beforeEach(() => {
    // Clean up DOM between tests
    document.body.innerHTML = '';
  });

  describe('Trigger Detection', () => {
    it('should show autocomplete when @ typed', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');
      expect(editor).toBeInTheDocument();

      // Type "@" to trigger autocomplete
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      // Autocomplete should appear
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      expect(screen.getByLabelText('Node reference autocomplete')).toBeInTheDocument();
    });

    it('should show autocomplete when @ typed after space', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Some text ',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');
      expect(editor).toBeInTheDocument();

      // Position cursor at end and type "@"
      await user.click(editor!);
      await user.keyboard('{End}');
      await user.keyboard('@');
      await waitForEffects();

      // Autocomplete should appear
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });
    });

    it('should NOT show autocomplete when @ typed mid-word', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'email',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');
      expect(editor).toBeInTheDocument();

      // Position cursor after "email" and type "@"
      // This simulates "email@" (mid-word scenario)
      await user.click(editor!);
      await user.keyboard('{End}');
      await user.keyboard('@');
      await waitForEffects(100);

      // Autocomplete should NOT appear (@ is part of email address)
      expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    });
  });

  describe('Query Filtering', () => {
    it('should show all results when @ typed with no query', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type just "@"
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      // Wait for autocomplete to appear
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Should show results (mock data should have multiple nodes)
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(0);
    });

    it('should filter results as user types query', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@task" (matches "Task List" in mock data)
      await user.click(editor!);
      await user.keyboard('@task');
      await waitForEffects();

      // Wait for autocomplete with filtered results
      await waitFor(() => {
        const listbox = screen.getByRole('listbox');
        expect(listbox).toBeInTheDocument();
      });

      // Should show filtered results containing "task" in title
      // Mock data includes "Task List" which should match
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(0);

      // Verify at least one result contains "task" (case insensitive)
      const hasTaskResult = options.some((option) =>
        option.textContent?.toLowerCase().includes('task')
      );
      expect(hasTaskResult).toBe(true);
    });

    it('should show empty state when no matches found', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type query that won't match any mock data
      await user.click(editor!);
      await user.keyboard('@zzzznonexistent999');
      await waitForEffects();

      // Autocomplete should still be visible
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Should show "Create new" option instead of "No nodes found"
      expect(screen.getByText(/create new "zzzznonexistent999" node/i)).toBeInTheDocument();
    });
  });

  describe('Selection Flow', () => {
    it('should insert reference when Enter pressed', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" to trigger autocomplete
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      // Wait for autocomplete to appear
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Press Enter to select first result
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Reference should be inserted in markdown format [Title](nodespace://id)
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.*\]\(nodespace:\/\/.*\)/);
      });
    });

    it('should insert reference when result clicked', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" to trigger
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      // Wait for autocomplete
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Click on first result
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(0);

      await user.click(options[0]);
      await waitForEffects();

      // Reference should be inserted
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.*\]\(nodespace:\/\/.*\)/);
      });
    });

    it('should close autocomplete on Escape without inserting', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Before ',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');
      const originalContent = 'Before ';

      // Type "@"
      await user.click(editor!);
      await user.keyboard('{End}@');
      await waitForEffects();

      // Wait for autocomplete
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Press Escape
      await user.keyboard('{Escape}');
      await waitForEffects();

      // Modal should close
      await waitFor(() => {
        expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
      });

      // Content should be unchanged (just "Before @")
      const content = editor!.textContent || '';
      expect(content).toBe(originalContent + '@');
    });

    it('should update query as user continues typing', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@t"
      await user.click(editor!);
      await user.keyboard('@t');
      await waitForEffects();

      // Wait for autocomplete
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Continue typing "est"
      await user.keyboard('est');
      await waitForEffects();

      // Autocomplete should still be visible with updated query
      expect(screen.getByRole('listbox')).toBeInTheDocument();

      // Should show filtered results for "test"
      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(0);
    });
  });

  describe('Event Emission Verification', () => {
    it('should emit event data through DOM on selection (Svelte 5 pattern)', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" and select
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Enter}');
      await waitForEffects();

      // In Svelte 5, we verify the event's effect on DOM rather than catching the event directly
      // The reference should be inserted, which proves the event chain worked
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      });

      // Verify the inserted reference contains valid data structure
      const content = editor!.textContent || '';
      const match = content.match(/\[(.+)\]\(nodespace:\/\/(.+)\)/);
      expect(match).toBeTruthy();

      const insertedTitle = match![1];
      const insertedId = match![2];

      // Verify data integrity - this proves the event carried correct data
      expect(insertedTitle).toBeTruthy();
      expect(insertedTitle.length).toBeGreaterThan(0);
      expect(insertedId).toBeTruthy();
      expect(insertedId).toMatch(/^mock-node-\d+$/); // Matches mock data format
    });

    it('should NOT emit event when Escape pressed (no DOM mutation)', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" then Escape
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Escape}');
      await waitForEffects();

      // Give time for any events to fire
      await waitForEffects(100);

      // Verify no reference was inserted (event wasn't fired)
      const content = editor!.textContent || '';
      expect(content).not.toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      expect(content).toBe('@'); // Only the @ character should remain
    });

    it('should emit event with nodeId matching selected option', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" and select
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Get the first option's text to verify correlation later
      const firstOption = screen.getAllByRole('option')[0];
      const optionText = firstOption.textContent || '';

      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify event data by checking DOM mutation
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      });

      const content = editor!.textContent || '';
      const match = content.match(/\[(.+)\]\(nodespace:\/\/(.+)\)/);
      expect(match).toBeTruthy();

      const insertedTitle = match![1];
      const insertedId = match![2];

      // Verify the event carried correct data by checking inserted values
      expect(insertedId).toMatch(/^mock-node-\d+$/);
      expect(optionText).toContain(insertedTitle); // Title matches what was selected
    });
  });

  describe('Component Event Integration', () => {
    it('should insert reference with correct nodeId when selection made', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" and select
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Enter}');
      await waitForEffects();

      // Reference should be inserted with correct format
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      });

      // Verify the reference contains a valid node ID
      const content = editor!.textContent || '';
      const match = content.match(/\[(.+)\]\(nodespace:\/\/(.+)\)/);
      expect(match).toBeTruthy();
      expect(match![2]).toBeTruthy(); // Node ID exists
      expect(match![2].startsWith('mock-node-')).toBe(true); // Mock node ID format
    });

    it('should NOT insert reference when autocomplete closed via Escape', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" then Escape
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Escape}');
      await waitForEffects();

      // Reference should NOT be inserted
      const content = editor!.textContent || '';
      expect(content).not.toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      expect(content).toBe('@'); // Only the @ character should remain
    });

    it('should insert reference with correct title and nodeId', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" to trigger
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Get the first option's title to verify later
      const firstOption = screen.getAllByRole('option')[0];
      const optionText = firstOption.textContent || '';

      await user.keyboard('{Enter}');
      await waitForEffects();

      // Reference inserted with data matching selected node
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      });

      const content = editor!.textContent || '';
      const match = content.match(/\[(.+)\]\(nodespace:\/\/(.+)\)/);
      expect(match).toBeTruthy();

      const insertedTitle = match![1];
      const insertedId = match![2];

      expect(insertedTitle).toBeTruthy();
      expect(insertedId).toBeTruthy();

      // Title should match the selected option
      expect(optionText).toContain(insertedTitle);
    });
  });

  describe('Focus Management', () => {
    it('should maintain editor focus while autocomplete is open', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@"
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      // Wait for autocomplete
      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      // Editor should still be focused (or body if focus not strictly maintained)
      // In contenteditable scenarios, focus typically stays on the editor
      expect(document.activeElement).toBe(editor);
    });

    it('should return focus to editor after selection', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" and select
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Enter}');
      await waitForEffects();

      // After selection, focus should return to editor
      await waitFor(() => {
        expect(document.activeElement).toBe(editor);
      });
    });
  });

  describe('Content Integrity', () => {
    it('should insert reference in correct markdown format', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Type "@" and select
      await user.click(editor!);
      await user.keyboard('@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify markdown format: [Title](nodespace://id)
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/\[.+\]\(nodespace:\/\/.+\)/);
      });

      // Extract and verify format components
      const content = editor!.textContent || '';
      const markdownRegex = /\[(.+)\]\(nodespace:\/\/(.+)\)/;
      const match = content.match(markdownRegex);

      expect(match).toBeTruthy();
      expect(match![1]).toBeTruthy(); // Title exists
      expect(match![2]).toBeTruthy(); // ID exists
    });

    it.skip('should preserve existing content around insertion (TODO: fix after @mention system rework)', async () => {
      const user = userEvent.setup();

      const { container } = render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Before ',
        autoFocus: true
      });

      const editor = container.querySelector('[contenteditable="true"]');

      // Position at end, type "@", select, then add " After"
      await user.click(editor!);
      await user.keyboard('{End}@');
      await waitForEffects();

      await waitFor(() => {
        expect(screen.getByRole('listbox')).toBeInTheDocument();
      });

      await user.keyboard('{Enter}');
      await waitForEffects();

      // Type more content after the reference
      await user.keyboard(' After');
      await waitForEffects();

      // Content should be: "Before [Title](nodespace://id) After"
      await waitFor(() => {
        const content = editor!.textContent || '';
        expect(content).toMatch(/Before.*\[.+\]\(nodespace:\/\/.+\).*After/);
      });
    });

    it.skip('should support undo/redo (not yet implemented)', () => {
      // TODO: Implement when undo/redo functionality is added to ContentEditableController
      // Test should verify:
      // - Ctrl+Z undoes reference insertion
      // - Ctrl+Shift+Z redoes insertion
      // - Content state is properly restored
    });
  });
});
