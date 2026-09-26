/**
 * UniqueFieldCheck — the schema-driven `unique` rule's suggestion state
 * (ADR-065), shared by every property form.
 *
 * A schema field declaring `unique` (optionally `uniqueCaseInsensitive`) gets
 * a suggest-don't-block interaction: on blur, a value colliding with another
 * active node of the same type surfaces a dismissible "already exists"
 * suggestion offering adopt-existing (open the match) or keep-as-new
 * (dismiss). The field's save is never gated on the lookup, and
 * create-anyway always remains possible — this class only ever reads.
 *
 * Case-insensitivity and the empty-value rule are resolved by the backend
 * from the same schema flags (`find_duplicate_for`), so this class needs only
 * to know WHETHER a field is unique (see `isUniqueField`), not how it compares.
 *
 * A duplicate that slips past this creation-time suggestion (offline write,
 * sync convergence) surfaces instead as a durable `UniqueFieldCollision`
 * record in the conflict journal (ADR-068), not here.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { getNavigationService } from '$lib/services/navigation-service';
import type { Node } from '$lib/types';
import type { SchemaField } from '$lib/types/schema-node';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UniqueFieldCheck');

/**
 * Whether a schema field carries the `unique` rule in a form this check can
 * serve: a text field, the only control that commits on blur.
 */
export function isUniqueField(field: SchemaField | undefined): boolean {
  return field?.unique === true && (field.type === 'string' || field.type === 'text');
}

export class UniqueFieldCheck {
  /**
   * The existing node the checked value collides with, or null when there is
   * none, the value is empty, or the suggestion was dismissed.
   */
  match = $state<Node | null>(null);

  // Skips re-issuing a lookup for a value already checked (e.g. tabbing
  // through an unchanged field). Staleness — whether an in-flight lookup's
  // result may still land — is decided by `generation`, NOT by comparing
  // values: two checks can race for the same or different values, and a
  // monotonic generation is the only thing that correctly says "only the
  // most recently STARTED lookup may ever write `match`", regardless of which
  // resolves first or what value each was for.
  private checkedValue: string | null = null;
  private generation = 0;

  // One instance per (node, field): a form editing a different node builds a
  // fresh one, so nothing computed for the previous node can linger.
  constructor(
    private readonly nodeType: string,
    private readonly fieldName: string
  ) {}

  /**
   * Look up an existing active node of this type holding `value`, excluding
   * `nodeId` itself. Runs on commit (blur), never per keystroke. Never blocks
   * or reverts a save: callers issue their write first and then call this. A
   * lookup failure is logged and simply surfaces no suggestion.
   */
  async check(nodeId: string, value: string): Promise<void> {
    if (this.checkedValue === value) return;
    this.checkedValue = value;
    // Claimed BEFORE the await: any earlier in-flight check is now
    // superseded and must not write `match` when it eventually resolves.
    const generation = ++this.generation;
    if (!value.trim()) {
      this.match = null;
      return;
    }
    try {
      const found = await backendAdapter.findDuplicateFor(this.nodeType, this.fieldName, value, nodeId);
      if (generation !== this.generation) return;
      // The backend already excludes `nodeId` via excludeId; the id check is
      // a defensive backstop, not the primary exclusion mechanism.
      this.match = found && found.id !== nodeId ? found : null;
    } catch (err) {
      if (generation !== this.generation) return;
      log.error('Duplicate lookup failed (non-blocking)', { err });
      this.match = null;
    }
  }

  /** "Keep as new" — create-anyway. The save already went through; this only clears the suggestion. */
  dismiss(): void {
    this.match = null;
  }

  /**
   * "Use existing" — open the existing node instead. Deliberately
   * non-destructive: nothing is deleted or merged. No source pane is known
   * here, so with two panes open this resolves against the active pane — the
   * same fallback other unparented callers of this navigation helper accept.
   */
  adopt(): void {
    if (!this.match) return;
    getNavigationService().navigateToNodeInOtherPane(this.match.id);
    this.match = null;
  }
}
