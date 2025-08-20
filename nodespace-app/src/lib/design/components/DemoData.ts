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
    content: 'Welcome to NodeSpace',
    inheritHeaderLevel: 0,
    children: [],
    expanded: true
  }
];