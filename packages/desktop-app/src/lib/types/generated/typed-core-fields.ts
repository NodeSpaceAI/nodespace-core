// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { TypedCoreField } from './typed-core-field';

/** The typed core fields of each core type that has any, in schema order. */
export const TYPED_CORE_FIELDS: Readonly<Record<string, readonly TypedCoreField[]>> = {
  task: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'due_date', wire: 'dueDate', date: true },
    { storage: 'started_at', wire: 'startedAt', date: true },
    { storage: 'completed_at', wire: 'completedAt', date: true },
    { storage: 'pull_request', wire: 'pullRequest', structured: 'object' },
    { storage: 'commits', wire: 'commits', structured: 'array' }
  ],
  project: [
    { storage: 'status', wire: 'status' },
    { storage: 'priority', wire: 'priority' },
    { storage: 'start_date', wire: 'startDate', date: true },
    { storage: 'end_date', wire: 'endDate', date: true },
    { storage: 'repository', wire: 'repository', structured: 'object' }
  ],
  spec: [
    { storage: 'objective', wire: 'objective' },
    { storage: 'boundaries', wire: 'boundaries' },
    { storage: 'spec_status', wire: 'specStatus' }
  ],
  plan: [
    { storage: 'approach', wire: 'approach' },
    { storage: 'risks', wire: 'risks' },
    { storage: 'plan_status', wire: 'planStatus' }
  ],
  decision: [{ storage: 'decision_status', wire: 'decisionStatus' }],
  person: [
    { storage: 'first_name', wire: 'firstName' },
    { storage: 'last_name', wire: 'lastName' },
    { storage: 'email', wire: 'email' }
  ],
  collection: [{ storage: 'description', wire: 'description' }],
  skill: [
    { storage: 'description', wire: 'description' },
    { storage: 'exclusion', wire: 'exclusion' },
    { storage: 'tool_whitelist', wire: 'toolWhitelist', structured: 'array' },
    { storage: 'max_iterations', wire: 'maxIterations', structured: 'number' }
  ],
  'database-settings': [
    { storage: 'required_extensions', wire: 'requiredExtensions', structured: 'array' }
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
    { storage: 'description', wire: 'description' },
    { storage: 'enabled', wire: 'enabled', structured: 'boolean' },
    { storage: 'suspended_reason', wire: 'suspendedReason', readOnly: true },
    { storage: 'suspended_message', wire: 'suspendedMessage', readOnly: true },
    { storage: 'suspended_at', wire: 'suspendedAt', readOnly: true }
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
    { storage: 'context_tokens', wire: 'contextTokens', structured: 'number' }
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
  ],
  'ai-chat-message': [
    { storage: 'role', wire: 'role' },
    { storage: 'timestamp', wire: 'timestamp' },
    { storage: 'reasoning', wire: 'reasoning' },
    { storage: 'outcome', wire: 'outcome' },
    { storage: 'options', wire: 'options', structured: 'array' }
  ]
};

/**
 * The values the backend fills when a stored node has none, by typed key.
 * Consumers copy before use, since a default can be an array.
 */
export const TYPED_CORE_DEFAULTS: Readonly<Record<string, Readonly<Record<string, unknown>>>> = {
  task: { status: 'open' },
  project: { status: 'planning' },
  spec: { specStatus: 'draft' },
  plan: { planStatus: 'draft' },
  decision: { decisionStatus: 'proposed' },
  skill: { description: '', maxIterations: 2, toolWhitelist: [] },
  'database-settings': { requiredExtensions: [] },
  query: { executionCount: 0, filters: [], generatedBy: 'user', targetType: '*' },
  play: { enabled: true, rules: [] },
  'ai-chat-native': { agent: '', contextTokens: 0, provider: 'native', turnStatus: 'idle' },
  'ai-chat-pty': { agent: '', sessionStatus: 'active' },
  'ai-chat-message': { role: 'user' }
};
