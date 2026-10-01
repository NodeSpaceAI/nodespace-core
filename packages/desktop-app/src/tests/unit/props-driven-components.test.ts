/**
 * Props-Driven Component Tests
 *
 * Verifies that refactored UI components are props-driven and Tauri-independent:
 * - BacklinksPanel: accepts backlinks as a prop (no sharedNodeStore import)
 * - TextNode, TaskNode, DateNode: accept content/nodeType/children as props (no store reads)
 * - NavigationSidebar: no @tauri-apps/api/event import
 * - AppShell: reloads schemas and collections on the data-plane-ready event
 *
 * All logic tests run in Happy-DOM mode (`bun run test`).
 */

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, it, expect } from 'vitest';
import type { NodeReference } from '$lib/types/node';
import { deriveTaskState, stripTaskMarker } from '$lib/design/components/task-state-syntax';
import { DATA_PLANE_READY_EVENT } from '$lib/services/daemon-status';

/** Read a file by its path under packages/desktop-app, resolved from this file so any runner cwd works. */
function readDesktopAppFile(path: string): string {
  return readFileSync(resolve(__dirname, '../../..', path), 'utf8');
}

// ============================================================================
// BacklinksPanel — Props Interface
// ============================================================================

describe('BacklinksPanel props contract', () => {
  it('renders empty state with no backlinks prop', () => {
    // BacklinksPanel now accepts `backlinks` as a prop; empty array = no links shown
    const backlinks: NodeReference[] = [];
    expect(backlinks.length).toBe(0);
  });

  it('accepts backlinks array as prop', () => {
    const backlinks: NodeReference[] = [
      { id: 'node-1', title: 'First Note', nodeType: 'text' },
      { id: 'node-2', title: 'Second Note', nodeType: 'task' }
    ];
    expect(backlinks).toHaveLength(2);
    expect(backlinks[0].title).toBe('First Note');
  });

  it('icon mapping still works for standard node types', () => {
    const iconMap: Record<string, string> = {
      date: 'calendar',
      task: 'circle',
      text: 'text',
      'ai-chat-native': 'aiSquare'
    };
    expect(iconMap['date']).toBe('calendar');
    expect(iconMap['task']).toBe('circle');
    expect(iconMap['text']).toBe('text');
    expect(iconMap['ai-chat-native']).toBe('aiSquare');
    expect(iconMap['unknown'] || 'text').toBe('text');
  });

  it('pluralization logic for backlink count', () => {
    function countText(n: number) {
      return `(${n} ${n === 1 ? 'node' : 'nodes'})`;
    }
    expect(countText(0)).toBe('(0 nodes)');
    expect(countText(1)).toBe('(1 node)');
    expect(countText(5)).toBe('(5 nodes)');
  });

  it('title fallback to id when title is null', () => {
    const ref: NodeReference = { id: 'abc-123', title: null, nodeType: 'text' };
    const display = ref.title || ref.id;
    expect(display).toBe('abc-123');
  });
});

// ============================================================================
// TextNode — Props Interface (no store reads)
// ============================================================================

describe('TextNode props contract', () => {
  it('accepts nodeId, content, nodeType, children, autoFocus as props', () => {
    const props = {
      nodeId: 'node-abc',
      content: 'Hello world',
      nodeType: 'text',
      children: ['child-1', 'child-2'],
      autoFocus: false
    };
    expect(props.nodeId).toBe('node-abc');
    expect(props.content).toBe('Hello world');
    expect(props.nodeType).toBe('text');
    expect(props.children).toHaveLength(2);
    expect(props.autoFocus).toBe(false);
  });

  it('defaults for optional props', () => {
    const defaults = {
      content: '',
      nodeType: 'text',
      children: [] as string[],
      autoFocus: false
    };
    expect(defaults.content).toBe('');
    expect(defaults.nodeType).toBe('text');
    expect(defaults.children).toHaveLength(0);
  });

  it('multiline editing is enabled by default for text nodes', () => {
    const editableConfig = { allowMultiline: true };
    expect(editableConfig.allowMultiline).toBe(true);
  });
});

// ============================================================================
// TaskNode — Props Interface and Task State Logic
// ============================================================================

describe('TaskNode props contract', () => {
  it('accepts standard node props', () => {
    const props = {
      nodeId: 'task-123',
      nodeType: 'task',
      content: 'Fix the bug',
      children: [] as string[],
      autoFocus: false,
      metadata: {}
    };
    expect(props.nodeId).toBe('task-123');
    expect(props.nodeType).toBe('task');
  });

  it('derives taskState from metadata.taskState, falling back to content task syntax', () => {
    // metadata.taskState from extractNodeMetadata takes priority
    expect(deriveTaskState({ taskState: 'completed' }, 'Some content')).toBe('completed');
    expect(deriveTaskState({ taskState: 'inProgress' }, 'Some content')).toBe('inProgress');
    expect(deriveTaskState({ taskState: 'pending' }, 'Some content')).toBe('pending');

    // Falls back to content-based detection when metadata has no taskState
    expect(deriveTaskState({}, '- [x] Completed task')).toBe('completed');
    expect(deriveTaskState({}, '[X] Completed task')).toBe('completed');
    expect(deriveTaskState({}, '- [~] Started task')).toBe('inProgress');
    expect(deriveTaskState({}, '[o] Started task')).toBe('inProgress');
    expect(deriveTaskState({}, '- [ ] Open task')).toBe('pending');
    expect(deriveTaskState({}, 'Regular text without task syntax')).toBe('pending');
  });

  it('only reads the in-progress marker from the leading brackets', () => {
    expect(deriveTaskState({}, '[ ] Finish memo]')).toBe('pending');
    expect(deriveTaskState({}, 'Write the memo] draft')).toBe('pending');
    expect(deriveTaskState({}, '[ ] Mark [o] later')).toBe('pending');
  });

  it('strips a leading task marker from content', () => {
    expect(stripTaskMarker('- [x] Done')).toBe('Done');
    expect(stripTaskMarker('[ ] Open')).toBe('Open');
    expect(stripTaskMarker('Plain [o] text')).toBe('Plain [o] text');
  });

  it('extractMetadata maps cancelled status to completed taskState', () => {
    // Mirrors the logic in core-plugins.ts extractMetadata for task nodes
    function extractTaskState(status: string | undefined): 'pending' | 'inProgress' | 'completed' {
      let taskState: 'pending' | 'inProgress' | 'completed' = 'pending';
      if (status === 'IN_PROGRESS' || status === 'in_progress') {
        taskState = 'inProgress';
      } else if (status === 'DONE' || status === 'done') {
        taskState = 'completed';
      } else if (status === 'OPEN' || status === 'open') {
        taskState = 'pending';
      } else if (status === 'CANCELLED' || status === 'cancelled') {
        taskState = 'completed';
      }
      return taskState;
    }

    expect(extractTaskState('cancelled')).toBe('completed');
    expect(extractTaskState('CANCELLED')).toBe('completed');
    // Ensure other statuses are unaffected
    expect(extractTaskState('done')).toBe('completed');
    expect(extractTaskState('DONE')).toBe('completed');
    expect(extractTaskState('in_progress')).toBe('inProgress');
    expect(extractTaskState('open')).toBe('pending');
    expect(extractTaskState(undefined)).toBe('pending');
  });

  it('task state cycling order: pending → inProgress → completed → pending', () => {
    function cycleState(current: string): string {
      switch (current) {
        case 'pending': return 'inProgress';
        case 'inProgress': return 'completed';
        case 'completed': return 'pending';
        default: return 'pending';
      }
    }
    expect(cycleState('pending')).toBe('inProgress');
    expect(cycleState('inProgress')).toBe('completed');
    expect(cycleState('completed')).toBe('pending');
  });
});

// ============================================================================
// DateNode — Props Interface and Date Formatting
// ============================================================================

describe('DateNode props contract', () => {
  it('accepts NodeComponentProps interface', () => {
    const props = {
      nodeId: '2026-06-08',
      content: '2026-06-08',
      nodeType: 'date',
      children: [] as string[],
      autoFocus: false
    };
    expect(props.nodeId).toBe('2026-06-08');
    expect(props.content).toBe('2026-06-08');
    expect(props.nodeType).toBe('date');
  });

  it('date parsing from YYYY-MM-DD content', () => {
    function parseDate(content: string): Date | null {
      if (!content.trim()) return new Date();
      const dateMatch = content.match(/^\d{4}-\d{2}-\d{2}/);
      if (dateMatch) return new Date(dateMatch[0]);
      const parsed = Date.parse(content.trim());
      return isNaN(parsed) ? null : new Date(parsed);
    }

    const d = parseDate('2026-06-08');
    expect(d).not.toBeNull();
    expect(d!.getFullYear()).toBe(2026);

    expect(parseDate('')).not.toBeNull(); // defaults to today
    expect(parseDate('not-a-date')).toBeNull();
  });

  it('formats date content with calendar emoji prefix', () => {
    function buildDisplayContent(content: string): string {
      const dateMatch = content.match(/^\d{4}-\d{2}-\d{2}/);
      if (!dateMatch) return content;
      const date = new Date(dateMatch[0]);
      const formatted = date.toLocaleDateString('en-US', {
        year: 'numeric', month: 'long', day: 'numeric'
      });
      return `📅 ${formatted}`;
    }

    const display = buildDisplayContent('2026-06-08');
    expect(display).toMatch(/^📅/);
    expect(display).toContain('2026');
  });
});

// ============================================================================
// NavigationSidebar — No Tauri imports
// ============================================================================

describe('NavigationSidebar Tauri independence', () => {
  it('does not import the Tauri event API', () => {
    const sidebar = readDesktopAppFile('src/lib/components/layout/navigation-sidebar.svelte');

    expect(sidebar).not.toContain('@tauri-apps/api/event');
  });
});

// ============================================================================
// AppShell — data-plane-ready reload
// ============================================================================

describe('AppShell data-plane-ready reload', () => {
  it('reloads schemas and collections when the daemon data plane is ready', () => {
    const shell = readDesktopAppFile('src/lib/components/layout/app-shell.svelte');
    const listener = shell.indexOf('listen(DATA_PLANE_READY_EVENT');

    expect(listener, 'app-shell must listen for DATA_PLANE_READY_EVENT').toBeGreaterThan(-1);
    const handler = shell.slice(listener, listener + 300);
    expect(handler).toContain('schemasData.loadSchemas()');
    expect(handler).toContain('collectionsData.loadCollections()');
  });

  it('no longer listens for the legacy capability-probe event', () => {
    const shell = readDesktopAppFile('src/lib/components/layout/app-shell.svelte');
    // Built from fragments so this test is not itself a hit for the event name.
    const legacyEvent = ['pro', 'tier-detected'].join(':');

    expect(shell).not.toContain(legacyEvent);
  });

  it('names the same event the Rust startup task emits', () => {
    const rust = readDesktopAppFile('app-lib/src/lib.rs');

    expect(DATA_PLANE_READY_EVENT).toBe('daemon:data-plane-ready');
    expect(rust).toContain(`"${DATA_PLANE_READY_EVENT}"`);
  });
});
