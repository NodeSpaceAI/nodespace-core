// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { AddRelationshipParams } from './add-relationship-params';
import type { CreateNodeParams } from './create-node-params';
import type { RejectParams } from './reject-params';
import type { RemoveRelationshipParams } from './remove-relationship-params';
import type { UpdateNodeParams } from './update-node-params';

/**
 * One step of a rule. Tagged on `action_type`, with that action's own
 * `params`.
 *
 * `for_each` runs the action once per node a path reaches
 * (`trigger.node.tasks`, optionally narrowed with `.where(...)`), binding
 * each as `item`. A `reject` has nothing to iterate, so it takes none.
 */
export type Action =
  | { action_type: 'create_node'; params: CreateNodeParams; for_each?: string }
  | { action_type: 'update_node'; params: UpdateNodeParams; for_each?: string }
  | { action_type: 'add_relationship'; params: AddRelationshipParams; for_each?: string }
  | { action_type: 'remove_relationship'; params: RemoveRelationshipParams; for_each?: string }
  | { action_type: 'reject'; params: RejectParams };
