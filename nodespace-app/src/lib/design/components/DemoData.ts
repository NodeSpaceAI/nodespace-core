/**
 * Minimal demo data for BaseNodeViewer
 * Replace with real node loading in production
 */

import { v4 as uuidv4 } from 'uuid';

export const demoNodes = [
  {
    id: uuidv4(),
    type: 'text',
    autoFocus: true, // Focus first node for immediate editing
    content: '# Welcome to NodeSpace',
    inheritHeaderLevel: 1,
    children: [
      {
        id: uuidv4(),
        type: 'text',
        autoFocus: false,
        content: '## This is a child node',
        inheritHeaderLevel: 2,
        children: [
          {
            id: uuidv4(),
            type: 'text',
            autoFocus: false,
            content: 'Test *__bold__* and **_italic_** text, plus __bold__ and _italic_ for testing',
            inheritHeaderLevel: 0,
            children: [],
            expanded: true
          }
        ],
        expanded: true
      },
      {
        id: uuidv4(),
        type: 'text',
        autoFocus: false,
        content: 'This is another child node',
        inheritHeaderLevel: 0,
        children: [],
        expanded: true
      }
    ],
    expanded: true
  }
];
