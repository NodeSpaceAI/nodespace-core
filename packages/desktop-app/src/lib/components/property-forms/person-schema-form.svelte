<!--
  PersonSchemaForm - Property form for person nodes

  Edits a person's typed core fields — firstName, lastName, email — read from
  the typed PersonNode and written through the store's typed person update
  (the same per-field write sequencing task's core fields get). Display
  identity (the inline outline row and node title) is composed by the person
  schema's title_template ("{first_name} {last_name}") — not synced into
  content here; person nodes are read-only inline, like other
  title_template-driven types (see resolveTitleOrContent / node-row.svelte).

  What stays person-specific is only presentation: the three inputs'
  example placeholders and email's input type. Everything else is shared:
  - Shell chrome (Collapsible, trigger row, promoted relationship fields, the
    gated Relationships entry point) is owned by TypedFormShell.
  - The duplicate-email suggestion is the schema-driven `unique` rule
    (UniqueFieldCheck, ADR-065), enabled because the loaded person schema
    flags `email` unique — not because this is person.

  Props:
  - nodeId: ID of the person node to display properties for
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { Input } from '$lib/components/ui/input';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { createLogger } from '$lib/utils/logger';
  import { evaluateTitleTemplate } from '$lib/utils/title-template';
  import { pushComputedTitle } from '$lib/utils/title-preview';
  import type { PersonNode, PersonNodeUpdate } from '$lib/types';
  import { type SchemaNode, type SchemaField, isSchemaNode } from '$lib/types/schema-node';
  import { resolveFieldValue, buildFieldWrite } from '$lib/components/schema/schema-field-resolution';
  import TypedFormShell from '$lib/components/schema/typed-form-shell.svelte';
  import UniqueFieldSuggestion from '$lib/components/schema/unique-field-suggestion.svelte';
  import { UniqueFieldCheck, isUniqueField } from '$lib/components/schema/unique-field-check.svelte';

  const log = createLogger('PersonSchemaForm');

  let { nodeId }: { nodeId: string } = $props();

  const node = $derived(sharedNodeStore.getNode(nodeId));
  const person = $derived(node?.nodeType === 'person' ? (node as PersonNode) : undefined);

  const firstName = $derived(person?.firstName ?? '');
  const lastName = $derived(person?.lastName ?? '');
  const email = $derived(person?.email ?? '');

  // Loaded once (constant type). Drives the `unique` rule on email; until it
  // lands, or if it fails, email simply gets no duplicate suggestion.
  let schema = $state<SchemaNode | null>(null);
  onMount(() => {
    backendAdapter
      .getSchema('person')
      .then((schemaNode) => {
        if (isSchemaNode(schemaNode)) schema = schemaNode;
      })
      .catch((error) => log.error('Failed to load schema:', error));
  });

  const emailField = $derived(schema?.fields.find((f) => f.name === 'email'));
  // Rebuilt per node: the instance can be reused across person nodes, and a
  // suggestion computed for the previous one must not linger.
  const emailCheck = $derived.by(() => {
    void nodeId;
    return new UniqueFieldCheck('person', 'email');
  });

  const fieldStats = $derived({
    filled: [firstName, lastName, email].filter((value) => value !== '').length,
    total: 3
  });

  // Routed through the store's typed person update (ADR-049). The store
  // applies the change optimistically and synchronously, and owns
  // persistence + error reporting. An emptied field is cleared rather than
  // stored as "".
  function updateField(field: keyof PersonNodeUpdate, value: string) {
    if (!person) return;
    sharedNodeStore.updatePersonNode(
      nodeId,
      { [field]: value === '' ? null : value },
      { type: 'viewer', viewerId: 'person-schema-form' }
    );
  }

  // Person has no nested (object/array) fields, so the shell's nested-field
  // modal never opens; these back it with the shared schema-field read/write.
  function getFieldValue(fieldName: string): unknown {
    return node ? resolveFieldValue(node, fieldName) : undefined;
  }

  function onFieldChange(fieldName: string, value: unknown) {
    if (!node) return;
    sharedNodeStore.updateNode(nodeId, buildFieldWrite(node, fieldName, value), {
      type: 'viewer',
      viewerId: 'person-schema-form'
    });
  }

  // Mirrors the person schema's title_template ("{first_name} {last_name}",
  // core_schemas.rs) — a literal, since the preview below supplies exactly
  // those two fields. Keep in sync if that template ever changes.
  const PERSON_TITLE_TEMPLATE = '{first_name} {last_name}';

  // Live, in-progress values for the title preview — NOT the same as
  // `firstName`/`lastName`, which track the last COMMITTED (blurred) value. A
  // user can tab from one name field into the other and keep typing before
  // either blurs; pairing one field's keystroke with the other's stale
  // committed value would drop the first field's edit from the preview.
  // Resynced whenever the committed values change (a different node, this
  // form's own commit landing, or a remote update).
  let firstNameDraft = $state('');
  let lastNameDraft = $state('');
  $effect(() => {
    firstNameDraft = firstName;
  });
  $effect(() => {
    lastNameDraft = lastName;
  });

  /**
   * Per ADR-077: the editing client computes its own title instantly from
   * in-progress field values, pushed into the store via `isComputedField`
   * (no persistence, no OCC) so every reader reflects it in the same tick.
   * The backend independently computes and persists the authoritative title
   * on save, which should match this preview exactly.
   */
  function pushTitlePreview() {
    if (!node) return;
    const title = evaluateTitleTemplate(PERSON_TITLE_TEMPLATE, {
      first_name: firstNameDraft,
      last_name: lastNameDraft
    });
    pushComputedTitle(nodeId, node, title, 'person-schema-form');
  }

  function handleFirstNameInput(e: Event) {
    firstNameDraft = (e.currentTarget as HTMLInputElement).value;
    pushTitlePreview();
  }

  function handleLastNameInput(e: Event) {
    lastNameDraft = (e.currentTarget as HTMLInputElement).value;
    pushTitlePreview();
  }

  function handleFirstNameBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    if (value !== firstName) updateField('firstName', value);
  }

  function handleLastNameBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    if (value !== lastName) updateField('lastName', value);
  }

  function handleEmailBlur(e: FocusEvent) {
    const value = (e.currentTarget as HTMLInputElement).value;
    // Save first, then look up: the store write is synchronous, so the save
    // is never gated on the lookup (suggest-don't-block).
    if (value !== email) updateField('email', value);
    if (isUniqueField(emailField)) void emailCheck.check(nodeId, value);
  }
</script>

{#if person}
  <TypedFormShell {nodeId} {fieldStats} autoOpen {getFieldValue} {onFieldChange}>
    {#snippet fields(_openNestedModal: (_field: SchemaField) => void)}
      <div class="grid grid-cols-2 gap-4">
        <div class="space-y-2">
          <label for="person-first-name" class="text-sm font-medium">First name</label>
          <Input
            id="person-first-name"
            type="text"
            value={firstName}
            placeholder="Jane"
            oninput={handleFirstNameInput}
            onblur={handleFirstNameBlur}
          />
        </div>
        <div class="space-y-2">
          <label for="person-last-name" class="text-sm font-medium">Last name</label>
          <Input
            id="person-last-name"
            type="text"
            value={lastName}
            placeholder="Doe"
            oninput={handleLastNameInput}
            onblur={handleLastNameBlur}
          />
        </div>
        <div class="space-y-2">
          <label for="person-email" class="text-sm font-medium">Email</label>
          <Input
            id="person-email"
            type="email"
            value={email}
            placeholder="email@example.com"
            onblur={handleEmailBlur}
          />
        </div>
        {#if emailField}
          <div class="col-span-2 empty:hidden">
            <UniqueFieldSuggestion check={emailCheck} field={emailField} />
          </div>
        {/if}
      </div>
    {/snippet}
  </TypedFormShell>
{/if}
