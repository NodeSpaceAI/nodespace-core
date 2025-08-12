/**
 * Simple mock node data for testing
 */
import type { MockNodeData } from '../utils/testUtils';

export const sampleTextNode: MockNodeData = {
  id: 'sample-text-1',
  type: 'text',
  content: 'This is a sample text node for testing.',
  createdAt: new Date('2024-01-01T10:00:00Z'),
  updatedAt: new Date('2024-01-01T10:30:00Z')
};

export const sampleTaskNode: MockNodeData = {
  id: 'sample-task-1',
  type: 'task',
  content: 'Complete the testing setup',
  createdAt: new Date('2024-01-01T11:00:00Z'),
  updatedAt: new Date('2024-01-01T11:15:00Z')
};

export const sampleAIChatNode: MockNodeData = {
  id: 'sample-chat-1',
  type: 'ai-chat',
  content: 'Help me understand this concept',
  createdAt: new Date('2024-01-01T12:00:00Z'),
  updatedAt: new Date('2024-01-01T12:05:00Z')
};

export const sampleNodes: MockNodeData[] = [sampleTextNode, sampleTaskNode, sampleAIChatNode];
