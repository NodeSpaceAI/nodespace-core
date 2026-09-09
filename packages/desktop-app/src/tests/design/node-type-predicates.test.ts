/**
 * Node type predicates — frontend-integration classification.
 *
 * These predicates answer "what frontend integration does this type have?" by asking the
 * plugin registry, never by consulting a hardcoded core-type list. The distinction matters:
 * `project` is a core type with no plugin registration at all, while `person` and
 * `document` are registered plugins that a core-type list never contained.
 */

import { describe, it, expect } from 'vitest';
import {
  computeHeaderDisplayValue,
  hasInlineNodeComponent,
  rendersAsEntityRow,
  hasEntityNounName,
  needsGenericSchemaForm
} from '$lib/design/components/node-type-predicates';

describe('node-type-predicates', () => {
  describe('hasInlineNodeComponent / rendersAsEntityRow', () => {
    it('treats types with a registered node component as inline-editable', () => {
      expect(hasInlineNodeComponent('text')).toBe(true);
      expect(hasInlineNodeComponent('task')).toBe(true);
      expect(rendersAsEntityRow('text')).toBe(false);
      expect(rendersAsEntityRow('task')).toBe(false);
    });

    it('treats a core type with no plugin registration as an entity row', () => {
      // `project` ships as a core type but registers no frontend plugin — it must get the
      // entity-row treatment (open button, skipped by arrow nav), not be assumed integrated.
      expect(hasInlineNodeComponent('project')).toBe(false);
      expect(rendersAsEntityRow('project')).toBe(true);
    });

    it('treats a user-defined schema type as an entity row', () => {
      expect(rendersAsEntityRow('7b1c2d3e-4f56-7890-abcd-ef1234567890')).toBe(true);
    });

    it('does not classify by core-list membership', () => {
      // `person` is registered with a node component but was absent from the old core list;
      // classifying by that list made it an entity row, which is wrong.
      expect(hasInlineNodeComponent('person')).toBe(true);
      expect(rendersAsEntityRow('person')).toBe(false);
    });

    it('pins the full classification for every registered core type', () => {
      // This predicate drives five behaviors — open button, arrow-nav skipping, slash-command
      // pane navigation, and viewer routing — so a change here is a UX change. Asserted as a
      // whole so any future registration that flips a type has to update this list knowingly.
      const entityRows = ['project', 'collection', 'schema', 'user', 'document', 'ai-chat'];
      const inlineEditable = [
        'text',
        'task',
        'person',
        'header',
        'code-block',
        'quote-block',
        'ordered-list',
        'horizontal-line',
        'table',
        'checkbox',
        'date',
        'query'
      ];

      for (const nodeType of entityRows) {
        expect(rendersAsEntityRow(nodeType), `${nodeType} should be an entity row`).toBe(true);
      }
      for (const nodeType of inlineEditable) {
        expect(rendersAsEntityRow(nodeType), `${nodeType} should be inline-editable`).toBe(false);
      }
    });
  });

  describe('hasEntityNounName', () => {
    it('is true for person, despite person also rendering inline (not an entity row)', () => {
      // The exact combination rendersAsEntityRow cannot see on its own: person has both an
      // inline node component AND a plugin `name` ("Person") that is a real entity noun.
      expect(rendersAsEntityRow('person')).toBe(false);
      expect(hasEntityNounName('person')).toBe(true);
    });

    it('is false for inline-editable content primitives, whose plugin name is a registry label', () => {
      // "Text Node" / "Header Node" / "Task Node" are labels for the plugin registry and
      // slash-command menu, not nouns a user would call the thing — must not opt in.
      expect(hasEntityNounName('text')).toBe(false);
      expect(hasEntityNounName('header')).toBe(false);
      expect(hasEntityNounName('task')).toBe(false);
    });

    it('is false for an unregistered/unknown node type', () => {
      expect(hasEntityNounName('not-a-real-type')).toBe(false);
    });

    it('does not imply, and is not implied by, rendersAsEntityRow', () => {
      // ai-chat is an entity row (no inline component) but was never opted into
      // entityNoun — the two predicates are independent, not two views of one flag.
      expect(rendersAsEntityRow('ai-chat')).toBe(true);
      expect(hasEntityNounName('ai-chat')).toBe(false);
    });
  });

  describe('needsGenericSchemaForm', () => {
    it('is false for types with a hardcoded schema form', () => {
      expect(needsGenericSchemaForm('task')).toBe(false);
      expect(needsGenericSchemaForm('person')).toBe(false);
    });

    it('is true for a core type with no hardcoded form', () => {
      // The bug this fixes: `project` was denied the generic form because it is core.
      expect(needsGenericSchemaForm('project')).toBe(true);
    });

    it('is true for user-defined schema types', () => {
      expect(needsGenericSchemaForm('7b1c2d3e-4f56-7890-abcd-ef1234567890')).toBe(true);
    });
  });
});

describe('computeHeaderDisplayValue', () => {
  describe('non-template types (title is content-derived)', () => {
    it('shows current content, not a stale cached title', () => {
      // The traced repro: `title` was computed for the transient `text`-typed node whose
      // content was a lone `/`, and never refreshed after the conversion to `task`.
      // The unfocused header must still show the content the user actually typed.
      expect(computeHeaderDisplayValue({ title: '/', content: 'Another Task' }, false)).toBe(
        'Another Task'
      );
    });

    it('does not fall back to a stale title when content is empty', () => {
      // An emptied header reads as empty, rather than resurrecting the previous title.
      expect(computeHeaderDisplayValue({ title: '/', content: '' }, false)).toBe('');
    });

    it('strips markdown header syntax from content', () => {
      expect(computeHeaderDisplayValue({ content: '## My Heading' }, false)).toBe('My Heading');
    });
  });

  describe('titleTemplate-driven schemas (title is property-computed)', () => {
    it('shows the computed title, ignoring raw content', () => {
      expect(
        computeHeaderDisplayValue({ title: 'Acme Corp — Invoice', content: 'raw' }, true)
      ).toBe('Acme Corp — Invoice');
    });

    it('renders empty rather than leaking content when the title has not resolved', () => {
      // The read-only header shows the titleTemplate placeholder in this state,
      // matching node-row.svelte's inline rendering.
      expect(computeHeaderDisplayValue({ title: '', content: 'raw content' }, true)).toBe('');
    });

    it('treats a title with no word characters as unresolved', () => {
      // e.g. "{first_name} {last_name}" with both fields empty resolves to " ".
      // node-row.svelte guards this with /\w/; without the same guard here the header
      // would render a blank-looking value instead of the titleTemplate placeholder.
      expect(computeHeaderDisplayValue({ title: ' ', content: 'raw' }, true)).toBe('');
      expect(computeHeaderDisplayValue({ title: ' — ', content: 'raw' }, true)).toBe('');
    });

    it('does not strip markdown from a computed title', () => {
      // A template-built title is assembled from property values; a leading '#' is a
      // literal part of the value, not header syntax.
      expect(computeHeaderDisplayValue({ title: '#1 Priority Account' }, true)).toBe(
        '#1 Priority Account'
      );
    });
  });

  it('handles a missing node', () => {
    expect(computeHeaderDisplayValue(undefined, false)).toBe('');
    expect(computeHeaderDisplayValue(undefined, true)).toBe('');
    // currentViewedNode is null before a node resolves
    expect(computeHeaderDisplayValue(null, false)).toBe('');
    expect(computeHeaderDisplayValue(null, true)).toBe('');
  });
});
