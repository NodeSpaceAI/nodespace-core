// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { InlineSelector } from './inline-selector';
import type { SavedQuerySelector } from './saved-query-selector';

/**
 * Which nodes a trigger applies to, said the way a query says it
 * (ADR-086 §11): inline, or by reference to a saved query node.
 */
export type Selector = SavedQuerySelector | InlineSelector;
