<!--
  PersonSchemaForm - Property form for person nodes

  Provides direct editing of first_name/last_name/email fields stored in
  properties.person.{first_name,last_name,email}. Display identity (the
  inline outline row and node title) is composed by the person schema's
  title_template ("{first_name} {last_name}") — not synced into content
  here; person nodes are read-only inline, like other title_template-driven
  types (see resolveTitleOrContent / node-row.svelte).

  Email carries a store-aware `unique` schema rule (ADR-065, case-insensitive,
  ignores empty): on blur, a colliding value surfaces a dismissible "a person
  with this email already exists" suggestion — adopt-existing (navigate to the
  match) or keep-as-new (dismiss, keep editing this node). This is
  suggest-don't-block by design: the field save above is never gated on the
  lookup, and create-anyway always remains possible.

  A duplicate that slips past this creation-time suggestion (offline write,
  sync convergence) is surfaced instead as a durable `UniqueFieldCollision`
  record in the conflict journal (ADR-068) — see the Conflicts view, not this
  form.
-->

<script lang="ts">
  import { Input } from '$lib/components/ui/input';
  import { Alert, AlertDescription } from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { createLogger } from '$lib/utils/logger';
  import type { Node } from '$lib/types';
  import RelationshipViewerModal from '$lib/components/relationships/relationship-viewer-modal.svelte';
  import { loadNodeRelationshipsView } from '$lib/services/relationship-viewer-service';
  import WaypointsIcon from '@lucide/svelte/icons/waypoints';
  import UserRoundSearchIcon from '@lucide/svelte/icons/user-round-search';

  const log = createLogger('PersonSchemaForm');

  // Relationships viewer entry point — inbound relationships (e.g.
  // tasks assigned to this person) surface here.
  let showRelationships = $state(false);

  let { nodeId }: { nodeId: string } = $props();

  // Gate the Relationships trigger the same way TypedFormShell now gates it for
  // TaskSchemaForm/GenericSchemaForm — shown only when this node's
  // type actually has a typed relationship (outbound declared on its schema, or
  // inbound declared by another schema targeting it), resolved once per nodeId.
  // Default hidden; fail-open on a query error so a transient failure never
  // hides a real feature. PersonSchemaForm doesn't route through TypedFormShell
  // (it stays hardcoded, not schema-driven — see the issue's recorded decision),
  // so this gate is duplicated here rather than shared; it's copied verbatim,
  // not reimplemented, to keep the two in agreement.
  let hasRelationships = $state(false);
  let relCheckedFor = '';
  $effect(() => {
    const id = nodeId;
    if (relCheckedFor === id) return;
    relCheckedFor = id;
    hasRelationships = false;
    loadNodeRelationshipsView(id)
      .then((view) => {
        if (nodeId === id) hasRelationships = view.groups.length > 0;
      })
      .catch((err) => {
        log.error('Failed to check relationships for the trigger gate', err);
        if (nodeId === id) hasRelationships = true;
      });
  });

  const node = $derived(sharedNodeStore.getNode(nodeId));
  const personProps = $derived(
    (node?.properties?.['person'] as Record<string, unknown> | undefined) ?? {}
  );

  const firstName = $derived((personProps['first_name'] as string | undefined) ?? '');
  const lastName = $derived((personProps['last_name'] as string | undefined) ?? '');
  const email = $derived((personProps['email'] as string | undefined) ?? '');

  // Adopt-existing suggestion state (ADR-065). `duplicateMatch` is
  // the existing person the current email collides with, or null when there is
  // none / the suggestion was dismissed. `checkedForEmail` skips re-issuing a
  // lookup for a value already checked (e.g. tabbing through an unchanged
  // field). Staleness itself — whether an in-flight lookup's result is still
  // allowed to land — is decided by `checkGeneration`, NOT by comparing values:
  // two different triggers (a blur check and a badge re-check) can
  // race for the SAME or DIFFERENT email, and a monotonic generation is the
  // only thing that correctly says "only the most recently STARTED lookup may
  // ever write `duplicateMatch`" regardless of which resolves first or what
  // value each was for.
  let duplicateMatch = $state<Node | null>(null);
  let checkedForEmail: string | null = null;
  let checkGeneration = 0;

  // A component instance can be reused across different person nodes (no
  // `{#key nodeId}` at the call site) — reset the suggestion when the node
  // being edited changes, or a suggestion computed for the PREVIOUS person
  // (a different existing-node id, a different email) would linger on screen
  // and "Use existing" would navigate using a match that no longer applies.
  $effect(() => {
    void nodeId;
    duplicateMatch = null;
    checkedForEmail = null;
    checkGeneration++;
  });

  // Routed through sharedNodeStore.updateNode (ADR-049), matching every
  // other property form — NOT backendAdapter.updateNode directly. The store
  // applies the change optimistically and synchronously (so the title,
  // recomputed from title_template, reaches every reader — including
  // BaseNodeViewer — immediately once persistence resolves) and owns
  // persistence + error reporting itself; callers don't need their own
  // try/catch around it, matching generic-schema-form.svelte /
  // task-schema-form.svelte.
  function updateField(field: 'first_name' | 'last_name' | 'email', value: string) {
    if (!node) return;
    const updatedProperties = {
      ...node.properties,
      person: { ...personProps, [field]: value }
    };
    sharedNodeStore.updateNode(
      nodeId,
      { properties: updatedProperties },
      { type: 'viewer', viewerId: 'person-schema-form' }
    );
  }

  function handleFirstNameBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    if (value !== firstName) updateField('first_name', value);
  }

  function handleLastNameBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    if (value !== lastName) updateField('last_name', value);
  }

  async function handleEmailBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    // The store write below is synchronous (it applies optimistically and
    // hands persistence off in the background), so issuing it before
    // awaiting the duplicate lookup already guarantees the check is never
    // gated behind the save landing — since both write and read this node's
    // own email, a check that ran AFTER the save landed would see two rows
    // holding `value` (this node's own freshly-saved copy, plus any real
    // duplicate), and with no ORDER BY on the lookup, could match itself and
    // hide the real duplicate entirely. `excludeId` below closes this
    // structurally regardless of ordering, but issuing the check immediately
    // also means the suggestion isn't held back by an in-flight save.
    if (value !== email) updateField('email', value);
    await checkForDuplicate(value);
  }

  /**
   * Suggest-don't-block uniqueness check (ADR-065): looks up an existing
   * active person with the same (case-insensitive) email, excluding this
   * node itself via `excludeId`. Runs on blur (commit), never on every
   * keystroke — a single indexed lookup, not a per-character scan. Skips the
   * round-trip entirely when re-blurring a value already checked, so tabbing
   * through an unchanged field doesn't re-issue it. Never blocks or reverts
   * the save; a lookup failure is logged and simply surfaces no suggestion.
   */
  async function checkForDuplicate(value: string) {
    if (checkedForEmail === value) return;
    checkedForEmail = value;
    // Claim this attempt's generation BEFORE the await — any earlier
    // in-flight call (whatever value or trigger it was for) is now
    // unconditionally superseded and must not write `duplicateMatch` when it
    // eventually resolves, even if it resolves AFTER this one.
    const generation = ++checkGeneration;
    if (!value.trim()) {
      duplicateMatch = null;
      return;
    }
    try {
      const match = await backendAdapter.findDuplicateFor('person', 'email', value, nodeId);
      // Ignore a superseded response — from either a newer blur check for a
      // different value, or a badge-triggered recheck that
      // started after this one. The backend already excludes this node via
      // excludeId above — the `match.id !== nodeId` check is a defensive
      // backstop, not the primary exclusion mechanism (an earlier version
      // relied on it alone, which could hide a REAL different duplicate
      // whenever this node's own not-yet-excluded row satisfied the query
      // first).
      if (generation !== checkGeneration) return;
      duplicateMatch = match && match.id !== nodeId ? match : null;
    } catch (err) {
      // Same staleness guard as the success path — a slow, now-superseded
      // request that fails after a newer one has already resolved must not
      // clobber that newer (possibly valid) result.
      if (generation !== checkGeneration) return;
      log.error('Duplicate lookup failed (non-blocking)', { err });
      duplicateMatch = null;
    }
  }

  function dismissDuplicateSuggestion() {
    // "Keep as new" — create-anyway. Nothing to undo: the field save already
    // went through above: this only clears the suggestion banner.
    duplicateMatch = null;
  }

  function adoptExisting() {
    if (!duplicateMatch) return;
    // "Use existing" — open the existing person instead. Deliberately
    // non-destructive: this does not delete or merge the current node (full
    // merge machinery is out of scope for this rule), it just gets the user to
    // the record they meant to use. No sourcePaneId is available from this
    // form's props, so with two panes open this resolves against the current
    // active pane rather than necessarily the pane hosting this form — the
    // same fallback other unparented call sites of this navigation helper
    // already accept.
    getNavigationService().navigateToNodeInOtherPane(duplicateMatch.id);
    duplicateMatch = null;
  }

  // duplicateMatch.title is the person schema's title_template-composed display
  // name (server-computed, same rule PersonNodeBehavior::compute_display_name
  // mirrors) — not hand-recomposed from first_name/last_name here.
  const duplicateDisplayName = $derived(duplicateMatch?.title || undefined);
</script>

<div class="person-schema-form">
  <div class="field">
    <label for="person-first-name">First name</label>
    <Input
      id="person-first-name"
      type="text"
      value={firstName}
      placeholder="Jane"
      onblur={handleFirstNameBlur}
    />
  </div>
  <div class="field">
    <label for="person-last-name">Last name</label>
    <Input
      id="person-last-name"
      type="text"
      value={lastName}
      placeholder="Doe"
      onblur={handleLastNameBlur}
    />
  </div>
  <div class="field">
    <label for="person-email">Email</label>
    <Input
      id="person-email"
      type="email"
      value={email}
      placeholder="email@example.com"
      onblur={handleEmailBlur}
    />
  </div>

  {#if duplicateMatch}
    <Alert variant="warning">
      <UserRoundSearchIcon class="h-4 w-4" />
      <AlertDescription class="duplicate-message">
        A person with this email already exists{duplicateDisplayName
          ? `: ${duplicateDisplayName}`
          : ''} — use them instead?
      </AlertDescription>
      <!-- AlertDescription renders a <p>, which cannot contain block content
           (another <p>, a <div>) without the browser silently restructuring
           the DOM — so the action buttons are a sibling, not a child. -->
      <div class="duplicate-actions">
        <Button type="button" size="sm" variant="outline" onclick={adoptExisting}>
          Use existing
        </Button>
        <Button type="button" size="sm" variant="ghost" onclick={dismissDuplicateSuggestion}>
          Keep as new
        </Button>
      </div>
    </Alert>
  {/if}

  <!-- Relationships entry point, gated on the type actually having
       typed relationships (outbound declared or inbound) — see hasRelationships above. -->
  {#if hasRelationships}
    <button
      type="button"
      class="flex w-full items-center gap-2 py-2 text-sm font-medium text-muted-foreground transition-all hover:opacity-80"
      onclick={() => (showRelationships = true)}
    >
      <WaypointsIcon class="h-4 w-4" />
      <span>Relationships</span>
    </button>
  {/if}
</div>

<RelationshipViewerModal bind:open={showRelationships} {nodeId} />

<style>
  .person-schema-form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.5rem 0;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  label {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
  }

  /* `class` on <AlertDescription> is forwarded to that component's own
     internal element rather than applied to one in THIS template, so the
     Svelte compiler can't see the usage and would otherwise warn this
     selector unused. */
  :global(.duplicate-message) {
    margin: 0 0 0.5rem 0;
  }

  .duplicate-actions {
    display: flex;
    gap: 0.5rem;
  }
</style>
