// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { TypedCoreField } from './typed-core-field';

/** The typed core fields of each core type that has any, in schema order. */
export const TYPED_CORE_FIELDS: Readonly<Record<string, readonly TypedCoreField[]>> = {
  task: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'due_date', wire: 'dueDate', date: true },
    { storage: 'started_at', wire: 'startedAt', date: true },
    { storage: 'completed_at', wire: 'completedAt', date: true }
  ],
  project: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'start_date', wire: 'startDate', date: true },
    { storage: 'end_date', wire: 'endDate', date: true }
  ],
  person: [
    { storage: 'first_name', wire: 'firstName' },
    { storage: 'last_name', wire: 'lastName' },
    { storage: 'email', wire: 'email' }
  ],
  query: [
    { storage: 'target_type', wire: 'targetType' },
    { storage: 'filters', wire: 'filters', structured: 'array' },
    { storage: 'sorting', wire: 'sorting', structured: 'array' },
    { storage: 'limit', wire: 'limit', structured: 'number' },
    { storage: 'generated_by', wire: 'generatedBy' },
    { storage: 'generator_context', wire: 'generatorContext' },
    { storage: 'execution_count', wire: 'executionCount', structured: 'number', readOnly: true },
    { storage: 'last_executed', wire: 'lastExecuted', readOnly: true },
    { storage: 'view_config', wire: 'viewConfig', structured: 'object' }
  ],
  play: [
    { storage: 'rules', wire: 'rules', structured: 'array' },
    { storage: 'description', wire: 'description' }
  ],
  'ai-chat': [
    { storage: 'agent', wire: 'agent' },
    { storage: 'model', wire: 'model' },
    { storage: 'summary', wire: 'summary' },
    { storage: 'last_active', wire: 'lastActive' }
  ],
  'ai-chat-native': [
    { storage: 'agent', wire: 'agent' },
    { storage: 'model', wire: 'model' },
    { storage: 'summary', wire: 'summary' },
    { storage: 'last_active', wire: 'lastActive' },
    { storage: 'provider', wire: 'provider' },
    { storage: 'turn_status', wire: 'turnStatus' },
    { storage: 'context_tokens', wire: 'contextTokens', structured: 'number' },
    { storage: 'messages', wire: 'messages', structured: 'array' }
  ],
  'ai-chat-pty': [
    { storage: 'agent', wire: 'agent' },
    { storage: 'model', wire: 'model' },
    { storage: 'summary', wire: 'summary' },
    { storage: 'last_active', wire: 'lastActive' },
    { storage: 'session_status', wire: 'sessionStatus' },
    { storage: 'session_id', wire: 'sessionId' },
    { storage: 'transcript', wire: 'transcript' },
    { storage: 'exit_code', wire: 'exitCode', structured: 'number' }
  ]
};

/**
 * The values the backend fills when a stored node has none, by typed key.
 * Consumers copy before use, since a default can be an array.
 */
export const TYPED_CORE_DEFAULTS: Readonly<Record<string, Readonly<Record<string, unknown>>>> = {
  task: { status: 'open' },
  project: { status: 'planning' },
  query: { executionCount: 0, filters: [], generatedBy: 'user', targetType: '*' },
  play: { rules: [] },
  'ai-chat-native': {
    agent: '',
    contextTokens: 0,
    messages: [],
    provider: 'native',
    turnStatus: 'idle'
  },
  'ai-chat-pty': { agent: '', sessionStatus: 'active' }
};
