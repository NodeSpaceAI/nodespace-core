import { describe, it, expect } from 'vitest';
import {
  buildRelationshipsView,
  filterUnlinkedTargets,
  groupDisplayLabel,
  groupAcceptsEdgesHere,
  groupEdgeColumns,
  findGroupByKey,
  findRowByKey,
  groupSupportsEdgeEditing,
  hasModalContent,
  isFormPromoted,
  isTargetLinked,
  linkedTargetIds,
  partitionGroups,
  humanizeName,
  farEndHolders,
  reassignmentPrompt,
  type RawNodeRelationships,
  type RawRelationshipGroup
} from '$lib/services/relationship-grouping';

function makeGroup(overrides: Partial<RawRelationshipGroup> = {}): RawRelationshipGroup {
  return {
    relationshipName: 'assigned_to',
    direction: 'out',
    targetType: 'person',
    reverseName: 'tasks',
    sourceType: 'task',
    cardinality: 'many',
    farCardinality: 'many',
    required: null,
    edgeFields: null,
    description: null,
    related: [],
    count: 0,
    ...overrides
  };
}

describe('relationship-grouping: humanizeName', () => {
  it('title-cases snake and kebab identifiers', () => {
    expect(humanizeName('assigned_to')).toBe('Assigned To');
    expect(humanizeName('billed-to')).toBe('Billed To');
    expect(humanizeName('tasks')).toBe('Tasks');
  });

  it('collapses repeated separators and trims', () => {
    expect(humanizeName('__blocked__by__')).toBe('Blocked By');
  });
});

describe('relationship-grouping: groupDisplayLabel', () => {
  it('uses the humanized relationship name for outbound groups', () => {
    expect(groupDisplayLabel(makeGroup({ direction: 'out', relationshipName: 'assigned_to' }))).toBe(
      'Assigned To'
    );
  });

  it('uses reverseName for inbound groups', () => {
    const group = makeGroup({
      direction: 'in',
      relationshipName: 'assigned_to',
      reverseName: 'tasks',
      sourceType: 'task'
    });
    expect(groupDisplayLabel(group)).toBe('Tasks');
  });

  it('never synthesizes a label from the source type, since reverseName is declared', () => {
    // The schema layer requires reverseName on every relationship, so the
    // inbound label is always the author's chosen name — "Invoices", not the
    // old synthesized "Invoice (Customer)".
    const group = makeGroup({
      direction: 'in',
      relationshipName: 'billed_to',
      reverseName: 'invoices',
      sourceType: 'invoice'
    });
    expect(groupDisplayLabel(group)).toBe('Invoices');
  });
});

describe('relationship-grouping: groupEdgeColumns surfaces undeclared keys', () => {
  it('lists a key stored on an edge that the schema never declared', () => {
    // The panel drives row expansion off this union, so a key present ONLY on
    // stored edge properties still has to appear — otherwise its value would be
    // unreachable in the UI, with no expander to reveal it.
    const group = makeGroup({
      edgeFields: null,
      related: [
        { id: 'p1', nodeType: 'person', title: 'Sarah', contentPreview: '', edgeProperties: { role: 'lead' } }
      ]
    });
    expect(groupEdgeColumns(group)).toEqual(['role']);
  });

  it('puts declared fields first, then undeclared keys', () => {
    const group = makeGroup({
      edgeFields: [{ name: 'role', type: 'string' }],
      related: [
        {
          id: 'p1',
          nodeType: 'person',
          title: 'Sarah',
          contentPreview: '',
          edgeProperties: { note: 'ad hoc', role: 'lead' }
        }
      ]
    });
    expect(groupEdgeColumns(group)).toEqual(['role', 'note']);
  });

  it('is empty for a bare relationship with neither declared fields nor edge data', () => {
    const group = makeGroup({
      edgeFields: null,
      related: [{ id: 'p1', nodeType: 'person', title: 'Sarah', contentPreview: '', edgeProperties: {} }]
    });
    expect(groupEdgeColumns(group)).toEqual([]);
  });
});

describe('relationship-grouping: groupEdgeColumns', () => {
  it('lists declared edge fields first, then undeclared edge keys', () => {
    const group = makeGroup({
      edgeFields: [
        { name: 'role', type: 'string' },
        { name: 'assigned_at', type: 'date' }
      ],
      related: [
        {
          id: 'p1',
          nodeType: 'person',
          title: 'Sarah',
          contentPreview: '',
          edgeProperties: { role: 'lead', priority: 'high' }
        }
      ]
    });
    expect(groupEdgeColumns(group)).toEqual(['role', 'assigned_at', 'priority']);
  });

  it('returns no columns for a chips group', () => {
    const group = makeGroup({ edgeFields: null, related: [] });
    expect(groupEdgeColumns(group)).toEqual([]);
  });
});

describe('relationship-grouping: buildRelationshipsView', () => {
  it('keeps outbound and inbound as separate groups and classifies each', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [
        makeGroup({
          relationshipName: 'assigned_to',
          direction: 'out',
          targetType: 'person',
          edgeFields: [{ name: 'role', type: 'string' }],
          count: 1,
          related: [
            {
              id: 'p1',
              nodeType: 'person',
              title: 'Sarah Chen',
              contentPreview: '',
              edgeProperties: { role: 'lead' }
            }
          ]
        }),
        makeGroup({
          relationshipName: 'belongs_to',
          direction: 'out',
          targetType: 'project',
          edgeFields: null,
          count: 1,
          related: [
            { id: 'pr1', nodeType: 'project', title: 'Apollo', contentPreview: '', edgeProperties: {} }
          ]
        })
      ]
    };

    const view = buildRelationshipsView(raw);
    expect(view.isEmpty).toBe(false);
    expect(view.groups).toHaveLength(2);

    const assigned = view.groups.find((g) => g.key === 'out:assigned_to:person');
    expect(assigned?.edgeColumns).toEqual(['role']);
    // Mutation-driving fields carried onto the view model.
    expect(assigned?.relationshipName).toBe('assigned_to');
    expect(assigned?.required).toBe(false);
    expect(assigned?.edgeFields).toEqual([{ name: 'role', type: 'string' }]);
    expect(assigned?.rows[0]).toEqual({
      id: 'p1',
      nodeType: 'person',
      label: 'Sarah Chen',
      edgeValues: { role: 'lead' }
    });

    const belongs = view.groups.find((g) => g.key === 'out:belongs_to:project');
    expect(belongs?.edgeColumns).toEqual([]);
  });

  it('retains a group with no related nodes but reports isEmpty', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [makeGroup({ related: [], count: 0 })]
    };
    const view = buildRelationshipsView(raw);
    // The empty declared group is kept so the editable modal can add the first
    // edge, but nothing is populated so the read-only view still counts as empty.
    expect(view.groups).toHaveLength(1);
    expect(view.groups[0].rows).toHaveLength(0);
    expect(view.isEmpty).toBe(true);
  });

  it('retains empty declared groups alongside populated ones and is not empty', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [
        makeGroup({
          relationshipName: 'assigned_to',
          direction: 'out',
          targetType: 'person',
          edgeFields: [{ name: 'role', type: 'string' }],
          count: 0,
          related: []
        }),
        makeGroup({
          relationshipName: 'belongs_to',
          direction: 'out',
          targetType: 'project',
          count: 1,
          related: [
            { id: 'pr1', nodeType: 'project', title: 'Apollo', contentPreview: '', edgeProperties: {} }
          ]
        })
      ]
    };
    const view = buildRelationshipsView(raw);
    expect(view.groups).toHaveLength(2);
    expect(view.isEmpty).toBe(false);
    const assigned = view.groups.find((g) => g.key === 'out:assigned_to:person');
    expect(assigned?.rows).toHaveLength(0);
    // A declared edge field still yields its column on an empty group.
    expect(assigned?.edgeColumns).toEqual(['role']);
  });

  it('carries relationshipName, required, and edgeFields onto the group view', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [
        makeGroup({
          relationshipName: 'assigned_to',
          direction: 'out',
          targetType: 'person',
          required: true,
          edgeFields: [
            { name: 'role', type: 'string' },
            { name: 'weight', type: 'number' }
          ],
          count: 1,
          related: [
            { id: 'p1', nodeType: 'person', title: 'Sarah', contentPreview: '', edgeProperties: { role: 'lead' } }
          ]
        })
      ]
    };
    const view = buildRelationshipsView(raw);
    const group = view.groups[0];
    expect(group.relationshipName).toBe('assigned_to');
    expect(group.required).toBe(true);
    expect(group.edgeFields).toEqual([
      { name: 'role', type: 'string' },
      { name: 'weight', type: 'number' }
    ]);
  });

  it('defaults required to false and edgeFields to [] when the group omits them', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [
        makeGroup({
          relationshipName: 'belongs_to',
          required: null,
          edgeFields: null,
          count: 1,
          related: [
            { id: 'pr1', nodeType: 'project', title: 'Apollo', contentPreview: '', edgeProperties: {} }
          ]
        })
      ]
    };
    const view = buildRelationshipsView(raw);
    expect(view.groups[0].required).toBe(false);
    expect(view.groups[0].edgeFields).toEqual([]);
  });

  it('labels an inbound group by reverseName and falls back to the row id when a target has no title', () => {
    const raw: RawNodeRelationships = {
      nodeId: 'person-1',
      nodeType: 'person',
      groups: [
        makeGroup({
          relationshipName: 'assigned_to',
          direction: 'in',
          reverseName: 'tasks',
          sourceType: 'task',
          targetType: 'task',
          count: 1,
          related: [{ id: 't-42', nodeType: 'task', title: null, contentPreview: '', edgeProperties: {} }]
        })
      ]
    };
    const view = buildRelationshipsView(raw);
    expect(view.groups[0].label).toBe('Tasks');
    expect(view.groups[0].rows[0].label).toBe('t-42');
  });
});

describe('relationship-grouping: partitionGroups', () => {
  const relatedNode = (id: string) => ({
    id,
    nodeType: 'adr',
    title: id,
    contentPreview: '',
    edgeProperties: {}
  });

  function viewOf(groups: RawRelationshipGroup[]) {
    return buildRelationshipsView({ nodeId: 'adr-1', nodeType: 'adr', groups }).groups;
  }

  it('gives a section only to groups that have edges, in either direction', () => {
    const groups = viewOf([
      makeGroup({ relationshipName: 'supersedes', count: 1, related: [relatedNode('adr-2')] }),
      makeGroup({
        relationshipName: 'supersedes',
        direction: 'in',
        reverseName: 'superseded_by',
        count: 1,
        related: [relatedNode('adr-9')]
      })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated).toHaveLength(2);
    expect(addable).toHaveLength(0);
  });

  it('moves a group out of `populated` when its last edge goes, while the group remains', () => {
    // The distinction the panel's rail depends on: removing a relationship's last
    // edge does NOT remove the group — it stays declared, which is what makes it
    // addable again. So a selection held as a group key goes on RESOLVING against
    // the view after the rail entry it pointed at is gone. A rail that re-selects
    // by "does this key still resolve?" therefore strands an empty detail pane;
    // it has to ask whether the key is still in `populated`.
    const before = viewOf([
      makeGroup({ relationshipName: 'supersedes', count: 1, related: [relatedNode('adr-2')] }),
      makeGroup({ relationshipName: 'decided_by', count: 1, related: [relatedNode('adr-3')] })
    ]);
    const key = before[0].key;
    expect(partitionGroups(before).populated.map((g) => g.key)).toContain(key);

    const after = viewOf([
      makeGroup({ relationshipName: 'supersedes', count: 0, related: [] }),
      makeGroup({ relationshipName: 'decided_by', count: 1, related: [relatedNode('adr-3')] })
    ]);
    const { populated, addable } = partitionGroups(after);

    // Still resolvable by key...
    expect(findGroupByKey(after, key)).not.toBeNull();
    // ...but no longer a rail entry, and now offered for re-adding instead.
    expect(populated.map((g) => g.key)).not.toContain(key);
    expect(addable.map((g) => g.key)).toContain(key);
  });

  it('folds empty OUTBOUND groups into the add chooser rather than sections', () => {
    const groups = viewOf([
      makeGroup({ relationshipName: 'supersedes' }),
      makeGroup({ relationshipName: 'depends_on' })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated).toHaveLength(0);
    expect(addable.map((g) => g.relationshipName)).toEqual(['supersedes', 'depends_on']);
  });

  it('folds an empty bare INBOUND many group into the add chooser, labeled by reverseName', () => {
    // A task's `Blocked By` is the same many-to-many edge as another task's
    // `Blocks`; recording it must not require opening the other task.
    const groups = viewOf([
      makeGroup({
        relationshipName: 'blocks',
        direction: 'in',
        targetType: 'task',
        reverseName: 'blocked_by'
      })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated).toHaveLength(0);
    expect(addable.map((g) => [g.direction, g.relationshipName, g.label])).toEqual([
      ['in', 'blocks', 'Blocked By']
    ]);
  });

  it('drops an empty inbound group that declares edge fields — authored from the declaring end', () => {
    const groups = viewOf([
      makeGroup({
        relationshipName: 'has_access_to',
        direction: 'in',
        reverseName: 'members',
        edgeFields: [{ name: 'access', type: 'string' }]
      })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated).toHaveLength(0);
    expect(addable).toHaveLength(0);
  });

  it('renders the six-declared-relationships-no-edges case as zero sections and one chooser', () => {
    // An `adr` with four outbound and two inbound declared relationships and no
    // edges must not produce six empty sections — every one is a chooser entry.
    const groups = viewOf([
      makeGroup({ relationshipName: 'supersedes' }),
      makeGroup({ relationshipName: 'depends_on' }),
      makeGroup({ relationshipName: 'implements' }),
      makeGroup({ relationshipName: 'authored_by' }),
      makeGroup({ relationshipName: 'supersedes', direction: 'in', reverseName: 'superseded_by' }),
      makeGroup({ relationshipName: 'relates_to', direction: 'in', reverseName: 'related_from' })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated).toHaveLength(0);
    expect(addable).toHaveLength(6);
  });

  it('offers a populated outbound group its own section AND keeps other empty types addable', () => {
    const groups = viewOf([
      makeGroup({ relationshipName: 'supersedes', count: 1, related: [relatedNode('adr-2')] }),
      makeGroup({ relationshipName: 'depends_on' })
    ]);
    const { populated, addable } = partitionGroups(groups);
    expect(populated.map((g) => g.relationshipName)).toEqual(['supersedes']);
    expect(addable.map((g) => g.relationshipName)).toEqual(['depends_on']);
  });
});

describe('relationship-grouping: single-valued groups are promoted to the form', () => {
  const person = {
    id: 'person-1',
    nodeType: 'person',
    title: 'Sam Lee',
    contentPreview: '',
    edgeProperties: {}
  };

  // A task's view: `assignee`/`project` are the inbound `one` ends of person's
  // and project's outbound `tasks`; `blocks`/`blocked_by` are both ends of
  // task's own many-to-many.
  function taskView(overrides: { assigned?: boolean } = {}) {
    return buildRelationshipsView({
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [
        makeGroup({ relationshipName: 'blocks', targetType: 'task', reverseName: 'blocked_by' }),
        makeGroup({
          relationshipName: 'blocks',
          direction: 'in',
          targetType: 'task',
          reverseName: 'blocked_by'
        }),
        makeGroup({
          relationshipName: 'tasks',
          direction: 'in',
          targetType: 'person',
          sourceType: 'person',
          reverseName: 'assignee',
          cardinality: 'one',
          farCardinality: 'many',
          related: overrides.assigned ? [person] : [],
          count: overrides.assigned ? 1 : 0
        }),
        makeGroup({
          relationshipName: 'tasks',
          direction: 'in',
          targetType: 'project',
          sourceType: 'project',
          reverseName: 'project',
          cardinality: 'one'
        })
      ]
    }).groups;
  }

  it('promotes every local-`one` group, populated or empty, whatever its direction', () => {
    const outboundOne = buildRelationshipsView({
      nodeId: 'invoice-1',
      nodeType: 'invoice',
      groups: [makeGroup({ relationshipName: 'billed_to', cardinality: 'one' })]
    }).groups;
    expect(isFormPromoted(outboundOne[0])).toBe(true);

    const { promoted } = partitionGroups(taskView({ assigned: true }));
    expect(promoted.map((g) => g.label)).toEqual(['Assignee', 'Project']);
  });

  it('keeps promoted groups out of the modal, so no edge has two controls', () => {
    const { populated, addable } = partitionGroups(taskView({ assigned: true }));
    // The assigned (populated, inbound) assignee does not become a rail section...
    expect(populated).toHaveLength(0);
    // ...and only task's own many-to-many remains addable — from both ends.
    expect(addable.map((g) => g.label)).toEqual(['Blocks', 'Blocked By']);
  });

  it('leaves a `one` group that declares edge fields in the modal, where they can be edited', () => {
    const [group] = buildRelationshipsView({
      nodeId: 'person-1',
      nodeType: 'person',
      groups: [
        makeGroup({
          relationshipName: 'employed_by',
          cardinality: 'one',
          farCardinality: 'many',
          edgeFields: [{ name: 'role', type: 'string' }]
        })
      ]
    }).groups;
    expect(isFormPromoted(group)).toBe(false);
    expect(partitionGroups([group]).addable).toEqual([group]);
  });

  it('gates the modal on what is left for it once promoted groups have moved out', () => {
    expect(hasModalContent(partitionGroups(taskView()))).toBe(true);

    // Only promoted groups and an empty inbound group this end cannot author:
    // the modal would be empty.
    const onlyPromoted = taskView().filter((g) => g.relationshipName !== 'blocks');
    const withEmptyInbound = [
      ...onlyPromoted,
      ...buildRelationshipsView({
        nodeId: 'task-1',
        nodeType: 'task',
        groups: [
          makeGroup({
            direction: 'in',
            relationshipName: 'watches',
            reverseName: 'watchers',
            edgeFields: [{ name: 'since', type: 'date' }]
          })
        ]
      }).groups
    ];
    expect(hasModalContent(partitionGroups(withEmptyInbound))).toBe(false);
  });
});

describe('relationship-grouping: groupAcceptsEdgesHere', () => {
  function groupView(overrides: Partial<RawRelationshipGroup>) {
    return buildRelationshipsView({
      nodeId: 'n-1',
      nodeType: 'task',
      groups: [makeGroup(overrides)]
    }).groups[0];
  }

  it('is true for an outbound group, with or without edge fields', () => {
    expect(groupAcceptsEdgesHere(groupView({}))).toBe(true);
    expect(
      groupAcceptsEdgesHere(groupView({ edgeFields: [{ name: 'access', type: 'string' }] }))
    ).toBe(true);
  });

  it('is true for a bare inbound group — the same edge from the other end', () => {
    expect(
      groupAcceptsEdgesHere(groupView({ direction: 'in', reverseName: 'blocked_by' }))
    ).toBe(true);
  });

  it('is false for an inbound group that declares edge fields', () => {
    const group = groupView({
      direction: 'in',
      reverseName: 'members',
      edgeFields: [{ name: 'access', type: 'string' }]
    });
    expect(groupAcceptsEdgesHere(group)).toBe(false);
  });
});

describe('relationship-grouping: groupSupportsEdgeEditing', () => {
  function groupView(overrides: Partial<RawRelationshipGroup>) {
    return buildRelationshipsView({
      nodeId: 'n-1',
      nodeType: 'collection',
      groups: [makeGroup(overrides)]
    }).groups[0];
  }

  it('is true for an outbound group whose schema declares edge fields', () => {
    const group = groupView({
      edgeFields: [{ name: 'access', type: 'string' }]
    });
    expect(groupSupportsEdgeEditing(group)).toBe(true);
  });

  it('is false when the schema declares no edge fields — nothing to edit', () => {
    expect(groupSupportsEdgeEditing(groupView({ edgeFields: null }))).toBe(false);
    expect(groupSupportsEdgeEditing(groupView({ edgeFields: [] }))).toBe(false);
  });

  it('is false for an inbound group even when it declares edge fields', () => {
    // The edge belongs to the other node's schema; it is edited from there.
    const group = groupView({
      direction: 'in',
      reverseName: 'members',
      edgeFields: [{ name: 'access', type: 'string' }]
    });
    expect(groupSupportsEdgeEditing(group)).toBe(false);
  });

  it('is false for ad-hoc edge keys with no declared field to render an input from', () => {
    const group = groupView({
      edgeFields: null,
      count: 1,
      related: [
        {
          id: 'p-1',
          nodeType: 'person',
          title: 'Ada',
          contentPreview: '',
          edgeProperties: { note: 'ad-hoc' }
        }
      ]
    });
    // The undeclared key is still surfaced for display, but offers no editor.
    expect(group.edgeColumns).toContain('note');
    expect(groupSupportsEdgeEditing(group)).toBe(false);
  });
});

describe('relationship-grouping: findGroupByKey / findRowByKey', () => {
  const related = (id: string, edge: Record<string, unknown> = {}) => ({
    id,
    nodeType: 'adr',
    title: id,
    contentPreview: '',
    edgeProperties: edge
  });

  function viewOf(groups: RawRelationshipGroup[]) {
    return buildRelationshipsView({ nodeId: 'adr-1', nodeType: 'adr', groups }).groups;
  }

  const populated = () =>
    viewOf([
      makeGroup({
        relationshipName: 'supersedes',
        edgeFields: [{ name: 'reason', type: 'string' }],
        count: 2,
        related: [related('adr-2', { reason: 'first' }), related('adr-3', { reason: 'second' })]
      })
    ]);

  it('resolves a group by its stable key', () => {
    const groups = populated();
    const key = groups[0].key;
    expect(findGroupByKey(groups, key)).toBe(groups[0]);
  });

  it('returns null for a null key or a key no longer present', () => {
    const groups = populated();
    expect(findGroupByKey(groups, null)).toBeNull();
    expect(findGroupByKey(groups, 'out:removed:adr')).toBeNull();
    expect(findGroupByKey([], groups[0].key)).toBeNull();
  });

  it('resolves a (group key, row id) pair to the CURRENT objects, not a snapshot', () => {
    // The staleness this guards: a reload rebuilds the whole object graph, so a
    // resolver must hand back the new row carrying the new edge values.
    const before = populated();
    const key = before[0].key;

    const after = viewOf([
      makeGroup({
        relationshipName: 'supersedes',
        edgeFields: [{ name: 'reason', type: 'string' }],
        count: 2,
        related: [related('adr-2', { reason: 'edited' }), related('adr-3', { reason: 'second' })]
      })
    ]);

    const resolved = findRowByKey(after, key, 'adr-2');
    expect(resolved?.row.edgeValues.reason).toBe('edited');
    // Identity check: it is the post-reload row, not the one we started from.
    expect(resolved?.row).not.toBe(before[0].rows[0]);
    expect(resolved?.group).toBe(after[0]);
  });

  it('returns null once the row it points at is gone, so an open editor closes', () => {
    const groups = populated();
    const key = groups[0].key;
    // The edge was removed by another action while the editor was open.
    const afterRemoval = viewOf([
      makeGroup({
        relationshipName: 'supersedes',
        edgeFields: [{ name: 'reason', type: 'string' }],
        count: 1,
        related: [related('adr-3', { reason: 'second' })]
      })
    ]);
    expect(findRowByKey(afterRemoval, key, 'adr-2')).toBeNull();
  });

  it('returns null when the whole group is gone, not just the row', () => {
    const groups = populated();
    expect(findRowByKey([], groups[0].key, 'adr-2')).toBeNull();
  });

  it('returns null for a missing key or row id', () => {
    const groups = populated();
    expect(findRowByKey(groups, null, 'adr-2')).toBeNull();
    expect(findRowByKey(groups, groups[0].key, null)).toBeNull();
  });

  it('does not confuse same-named groups facing opposite directions', () => {
    // Both groups share `relationshipName`; only the key distinguishes them, and
    // an editor keyed to the outbound one must never resolve to the inbound.
    const groups = viewOf([
      makeGroup({
        relationshipName: 'supersedes',
        count: 1,
        related: [related('adr-2')]
      }),
      makeGroup({
        relationshipName: 'supersedes',
        direction: 'in',
        reverseName: 'superseded_by',
        count: 1,
        related: [related('adr-9')]
      })
    ]);
    const [outbound, inbound] = groups;
    expect(outbound.key).not.toBe(inbound.key);
    expect(findGroupByKey(groups, outbound.key)?.direction).toBe('out');
    expect(findGroupByKey(groups, inbound.key)?.direction).toBe('in');
    // The outbound group holds adr-2, the inbound adr-9 — no cross-resolution.
    expect(findRowByKey(groups, outbound.key, 'adr-9')).toBeNull();
    expect(findRowByKey(groups, inbound.key, 'adr-2')).toBeNull();
  });
});

describe('relationship-grouping: row keys are reproducible, so stale keys must be dropped', () => {
  const related = (id: string, edge: Record<string, unknown> = {}) => ({
    id,
    nodeType: 'adr',
    title: id,
    contentPreview: '',
    edgeProperties: edge
  });

  function viewOf(groups: RawRelationshipGroup[]) {
    return buildRelationshipsView({ nodeId: 'adr-1', nodeType: 'adr', groups }).groups;
  }

  const withTargets = (targets: string[]) =>
    viewOf([
      makeGroup({
        relationshipName: 'supersedes',
        edgeFields: [{ name: 'reason', type: 'string' }],
        count: targets.length,
        related: targets.map((id) => related(id))
      })
    ]);

  /**
   * A row id is the TARGET NODE's id and a group key is
   * `direction:name:targetType` — neither is unique to a particular edge. So
   * deleting an edge and re-adding the same target reproduces a row that a
   * previously-held key resolves against again.
   *
   * This is why the modal must forget `editingKey` the moment resolution
   * misses, rather than relying on the key staying unresolvable: otherwise a
   * re-add silently reopens the editor over a brand-new edge, carrying the
   * deleted edge's draft.
   */
  it('resolves a stale key again once the same target is re-added', () => {
    const before = withTargets(['adr-2', 'adr-3']);
    const groupKey = before[0].key;
    expect(findRowByKey(before, groupKey, 'adr-2')).not.toBeNull();

    // adr-2's edge is deleted — the key stops resolving.
    const afterDelete = withTargets(['adr-3']);
    expect(findRowByKey(afterDelete, groupKey, 'adr-2')).toBeNull();

    // The same target is linked again. The key is reproducible, so it resolves
    // once more — to a DIFFERENT row object representing a different edge.
    const afterReAdd = withTargets(['adr-3', 'adr-2']);
    const resolved = findRowByKey(afterReAdd, groupKey, 'adr-2');
    expect(resolved).not.toBeNull();
    expect(resolved?.row).not.toBe(before[0].rows[0]);
  });

  it('keeps a group key stable when the group moves between partitions', () => {
    // The key must survive the empty→populated promotion, so an in-flight
    // search for a group that just gained its first edge is not discarded.
    const empty = viewOf([makeGroup({ relationshipName: 'supersedes' })]);
    const populated = withTargets(['adr-2']);
    expect(populated[0].key).toBe(empty[0].key);
    expect(partitionGroups(empty).addable).toHaveLength(1);
    expect(partitionGroups(populated).populated).toHaveLength(1);
  });
});

describe('relationship-grouping: linkedTargetIds / isTargetLinked / filterUnlinkedTargets', () => {
  const relatedNode = (id: string) => ({
    id,
    nodeType: 'person',
    title: id,
    contentPreview: '',
    edgeProperties: {}
  });

  function groupWithRows(ids: string[]) {
    return buildRelationshipsView({
      nodeId: 'task-1',
      nodeType: 'task',
      groups: [makeGroup({ related: ids.map(relatedNode), count: ids.length })]
    }).groups[0];
  }

  it('collects the ids of every row currently linked into the group', () => {
    const group = groupWithRows(['p1', 'p2']);
    expect(linkedTargetIds(group)).toEqual(new Set(['p1', 'p2']));
  });

  it('is empty for a group with no rows yet', () => {
    const group = groupWithRows([]);
    expect(linkedTargetIds(group)).toEqual(new Set());
  });

  it('isTargetLinked is true only for an id already among the group rows', () => {
    const group = groupWithRows(['p1']);
    expect(isTargetLinked(group, 'p1')).toBe(true);
    expect(isTargetLinked(group, 'p2')).toBe(false);
  });

  it('filterUnlinkedTargets drops candidates already linked, keeping the rest in order', () => {
    const group = groupWithRows(['p1', 'p2']);
    const candidates = [{ id: 'p1' }, { id: 'p3' }, { id: 'p2' }];
    expect(filterUnlinkedTargets(group, candidates)).toEqual([{ id: 'p3' }]);
  });

  it('filterUnlinkedTargets is a no-op when no candidate overlaps the linked set', () => {
    const group = groupWithRows(['p1']);
    const candidates = [{ id: 'p9' }];
    expect(filterUnlinkedTargets(group, candidates)).toEqual(candidates);
  });

  it('isTargetLinked recognizes a differently-cased id of an already-linked node', () => {
    // Comparisons must be case-insensitive: a caller may hold an id in a
    // different case than the backend's canonical (lowercase) form, and that
    // must still be recognized as the same linked node.
    const group = groupWithRows(['550e8400-e29b-41d4-a716-446655440000']);
    expect(isTargetLinked(group, '550E8400-E29B-41D4-A716-446655440000')).toBe(true);
  });

  it('filterUnlinkedTargets drops a differently-cased duplicate of an already-linked id', () => {
    const group = groupWithRows(['550e8400-e29b-41d4-a716-446655440000']);
    const candidates = [{ id: '550E8400-E29B-41D4-A716-446655440000' }, { id: 'p9' }];
    expect(filterUnlinkedTargets(group, candidates)).toEqual([{ id: 'p9' }]);
  });
});

describe('relationship-grouping: reassignment detection', () => {
  const related = (id: string, title: string) => ({
    id,
    nodeType: 'person',
    title,
    contentPreview: '',
    edgeProperties: {}
  });
  const view = (groups: RawRelationshipGroup[]) =>
    buildRelationshipsView({ nodeId: 'x', nodeType: 'x', groups });

  // Bob's inbound `assigned_to`: many tasks per person, one person per task.
  const bobsTasks = view([
    makeGroup({ direction: 'in', cardinality: 'many', farCardinality: 'one' })
  ]).groups[0];

  it("names the candidate's current holder on the mirror group", () => {
    const task = view([
      makeGroup({ cardinality: 'one', related: [related('alice', 'Alice')], count: 1 })
    ]);
    expect(farEndHolders(task, bobsTasks, 'bob').map((row) => row.label)).toEqual(['Alice']);
  });

  it('ignores the node itself, case-insensitively', () => {
    const task = view([
      makeGroup({ cardinality: 'one', related: [related('BOB', 'Bob')], count: 1 })
    ]);
    expect(farEndHolders(task, bobsTasks, 'bob')).toEqual([]);
  });

  it('finds nothing when the far end is many', () => {
    const many = view([makeGroup({ direction: 'in', farCardinality: 'many' })]).groups[0];
    const task = view([
      makeGroup({ cardinality: 'one', related: [related('alice', 'Alice')], count: 1 })
    ]);
    expect(farEndHolders(task, many, 'bob')).toEqual([]);
  });

  it('ignores same-direction and differently named groups', () => {
    const task = view([
      makeGroup({ direction: 'in', cardinality: 'one', related: [related('a', 'A')], count: 1 }),
      makeGroup({
        relationshipName: 'reviewer',
        cardinality: 'one',
        related: [related('b', 'B')],
        count: 1
      })
    ]);
    expect(farEndHolders(task, bobsTasks, 'bob')).toEqual([]);
  });

  it("ignores another schema's same-named relationship", () => {
    // A task's single project is also declared as `tasks` (by project), with
    // a different reverse name; linking an assignee never evicts it.
    const task = view([
      makeGroup({
        reverseName: 'project',
        cardinality: 'one',
        related: [related('proj', 'Project P')],
        count: 1
      }),
      makeGroup({ cardinality: 'one', related: [related('alice', 'Alice')], count: 1 })
    ]);
    expect(farEndHolders(task, bobsTasks, 'bob').map((row) => row.label)).toEqual(['Alice']);
  });

  it('builds a prompt only when something would be displaced', () => {
    const alice = { id: 'alice', nodeType: 'person', label: 'Alice', edgeValues: {} };
    const old = { id: 'old', nodeType: 'person', label: 'Old', edgeValues: {} };
    expect(reassignmentPrompt('Ship it', [])).toBeNull();
    const prompt = reassignmentPrompt('Ship it', [alice], [old]);
    expect(prompt).toContain('"Ship it" is currently linked to "Alice"');
    expect(prompt).toContain('"Old" will be replaced');
  });
});
