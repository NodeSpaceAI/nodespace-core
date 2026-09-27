/**
 * Task state derivation for TaskNode.
 *
 * `metadata.taskState` (pre-computed by the task plugin's extractMetadata from the
 * backend `status` field) wins; otherwise the state is read from a leading markdown
 * task marker: `[x]` completed, `[~]`/`[o]` in progress, `[ ]` pending.
 */

import type { NodeState } from '$lib/design/icons/registry';

export function deriveTaskState(metadata: Record<string, unknown>, content: string): NodeState {
  if (metadata.taskState) {
    return metadata.taskState as NodeState;
  }

  const trimmed = content.trim();
  if (/^-?\s*\[x\]/i.test(trimmed)) {
    return 'completed';
  }
  if (/^-?\s*\[[~o]\]/i.test(trimmed)) {
    return 'inProgress';
  }
  return 'pending';
}
