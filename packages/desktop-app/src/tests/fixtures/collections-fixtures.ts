/**
 * Test fixtures for collections store tests
 */

import type { CollectionItem, CollectionMember } from '$lib/stores/collections';

export const mockCollections: CollectionItem[] = [
  {
    id: 'col-1',
    name: 'Project Ideas',
    memberCount: 3,
    children: [
      {
        id: 'col-1-1',
        name: 'AI Features and Machine Learning Integration',
        memberCount: 2,
        children: [
          { id: 'col-1-1-1', name: 'Natural Language Processing Research', memberCount: 1 },
          { id: 'col-1-1-2', name: 'Vector Embeddings and Semantic Search', memberCount: 1 }
        ]
      },
      { id: 'col-1-2', name: 'UI Improvements', memberCount: 1 }
    ]
  },
  {
    id: 'col-2',
    name: 'Meeting Notes',
    memberCount: 2,
    children: [
      { id: 'col-2-1', name: '2025 Q1', memberCount: 1 },
      {
        id: 'col-2-2',
        name: '2024 Q4',
        memberCount: 1,
        children: [
          { id: 'col-2-2-1', name: 'Sprint Reviews', memberCount: 1 },
          { id: 'col-2-2-2', name: 'Retrospectives', memberCount: 1 }
        ]
      }
    ]
  },
  { id: 'col-3', name: 'Research Papers', memberCount: 0 },
  { id: 'col-4', name: 'Reading List', memberCount: 4 }
];

export const mockMembers: Record<string, CollectionMember[]> = {
  'col-1': [
    { id: 'node-1', name: 'AI-Powered Note Taking', nodeType: 'text' },
    { id: 'node-2', name: 'Voice Interface Design', nodeType: 'text' },
    { id: 'node-3', name: 'Graph Visualization', nodeType: 'text' }
  ],
  'col-1-1': [
    { id: 'node-10', name: 'GPT Integration Ideas', nodeType: 'text' },
    { id: 'node-11', name: 'Local LLM Research', nodeType: 'text' }
  ],
  'col-1-1-1': [{ id: 'node-12', name: 'Prompt Engineering Notes', nodeType: 'text' }],
  'col-1-1-2': [{ id: 'node-13', name: 'Vector DB Comparison', nodeType: 'text' }],
  'col-1-2': [{ id: 'node-14', name: 'Dark Mode Implementation', nodeType: 'task' }],
  'col-2': [
    { id: 'node-4', name: 'Team Standup 2025-01-15', nodeType: 'date' },
    { id: 'node-5', name: 'Sprint Planning', nodeType: 'text' }
  ],
  'col-2-1': [{ id: 'node-15', name: 'January Kickoff', nodeType: 'date' }],
  'col-2-2': [{ id: 'node-16', name: 'Q4 Summary', nodeType: 'text' }],
  'col-2-2-1': [{ id: 'node-17', name: 'Sprint 24 Review', nodeType: 'text' }],
  'col-2-2-2': [{ id: 'node-18', name: 'Team Improvements', nodeType: 'text' }],
  'col-3': [], // Empty collection
  'col-4': [
    { id: 'node-6', name: 'Designing Data-Intensive Apps', nodeType: 'text' },
    { id: 'node-7', name: 'Clean Architecture', nodeType: 'text' },
    { id: 'node-8', name: 'Review chapter 5', nodeType: 'task' },
    { id: 'node-9', name: 'Domain-Driven Design', nodeType: 'text' }
  ]
};
