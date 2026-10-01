// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EdgeField } from './edge-field';
import type { RelationshipCardinality } from './relationship-cardinality';
import type { RelationshipDirection } from './relationship-direction';

export type SchemaRelationship = {
  name: string;
  targetType?: string;
  direction: RelationshipDirection;
  cardinality: RelationshipCardinality;
  required?: boolean;
  /**
   * The name this edge reads by from the target's end. REQUIRED.
   *
   * A relationship is declared once — the storage model keeps a single
   * `relationship` row between the two schema nodes — but it is read from
   * both ends. Leaving this unset left that one stored edge only
   * half-declared: the target's side fell back to a synthesized
   * `"{SourceType} ({Relationship Name})"` label, so an invoice declaring
   * `billed_to → customer` surfaced on the customer as
   * "Invoice (Customer)" rather than "Invoices".
   *
   * Naming the inverse is a modeling decision only the author can make, so
   * it is required rather than derived. See the type-level note on
   * [`SchemaRelationship::reverse_cardinality`] for why both live in the
   * type rather than in a validator alone.
   */
  reverseName: string;
  /**
   * The cardinality governing the target's end — how many sources may point
   * at one target. REQUIRED, and the counterpart to
   * [`SchemaRelationship::reverse_name`].
   *
   * Unset, the inbound group carried no cardinality at all, so nothing
   * downstream could reason about how many sources may point at a node.
   *
   * Carrying both halves in the type — not merely in a validator — is what
   * makes "every stored edge is named from both ends" an invariant every
   * reader can rely on instead of a convention that holds only on the paths
   * which happen to route through validation. `handle_create_schema` and
   * `handle_update_schema` reject a payload missing either field before
   * serde sees it, so a caller gets an actionable error naming the field
   * rather than a bare `missing field` message.
   */
  reverseCardinality: RelationshipCardinality;
  edgeFields?: Array<EdgeField>;
  description?: string;
};
