<!--
  LinkFieldControl — shows and edits one `link` value: a title and a URL.

  Controlled and presentational, like SchemaFieldLeaf: it never touches the
  store. A link is stored whole, and a partial one is refused, so the two
  inputs are a draft that is handed to `onChange` only when it is a complete
  link (or blank, which clears the field as `null`).

  Shown: the link's title, which opens the URL in the system browser. Only
  `http` and `https` are opened; a link with any other scheme is plain text
  with its URL beside it.

  Props:
  - value: the current value (a link, or unset)
  - fieldId: id for the control a `<label for>` points at
  - onChange: receives the new link, or `null` to clear
  - addLabel: text of the button shown when there is no link yet
-->
<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import type { LinkValue } from '$lib/types/generated';
  import { openUrl } from '$lib/utils/external-links';
  import { asLinkValue, isOpenableLink, linkLabel, resolveLinkDraft } from '$lib/utils/link-values';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('LinkFieldControl');

  let {
    value,
    fieldId,
    onChange,
    addLabel = 'Add link'
  }: {
    value: unknown;
    fieldId: string;
    onChange: (_value: LinkValue | null) => void;
    addLabel?: string;
  } = $props();

  const link = $derived(asLinkValue(value));

  let editing = $state(false);
  let draftTitle = $state('');
  let draftUrl = $state('');
  let error = $state('');

  function startEditing() {
    draftTitle = link?.title ?? '';
    draftUrl = link?.url ?? '';
    error = '';
    editing = true;
  }

  function save() {
    const result = resolveLinkDraft(draftTitle, draftUrl);
    if (result.kind === 'invalid') {
      error = result.message;
      return;
    }
    editing = false;
    // Leaving an unset link blank changes nothing.
    if (result.kind === 'clear' && !link) return;
    onChange(result.kind === 'link' ? result.link : null);
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      event.preventDefault();
      save();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      editing = false;
    }
  }

  function open(url: string) {
    openUrl(url).catch((err: unknown) => {
      log.error('Failed to open link', err);
    });
  }
</script>

{#if editing}
  <div class="grid gap-2" data-testid="link-editor">
    <Input
      id={fieldId}
      type="text"
      placeholder="Title"
      aria-label="Link title"
      value={draftTitle}
      oninput={(e) => (draftTitle = e.currentTarget.value)}
      onkeydown={onKeydown}
    />
    <Input
      type="url"
      placeholder="https://example.com"
      aria-label="Link URL"
      aria-invalid={error ? 'true' : undefined}
      value={draftUrl}
      oninput={(e) => {
        draftUrl = e.currentTarget.value;
        error = '';
      }}
      onkeydown={onKeydown}
    />
    {#if error}
      <p class="text-xs text-destructive" role="alert">{error}</p>
    {/if}
    <div class="flex gap-2">
      <Button type="button" size="sm" onclick={save}>Save</Button>
      <Button type="button" size="sm" variant="ghost" onclick={() => (editing = false)}>
        Cancel
      </Button>
    </div>
  </div>
{:else if link}
  <div class="flex h-10 items-center justify-between gap-2" data-testid="link-display">
    {#if isOpenableLink(link.url)}
      <button
        type="button"
        id={fieldId}
        class="min-w-0 truncate text-left text-sm font-medium text-primary hover:underline"
        title={link.url}
        onclick={() => open(link.url)}
      >
        {linkLabel(link)}
      </button>
    {:else}
      <span id={fieldId} class="min-w-0 truncate text-sm" data-testid="link-text">
        {link.title.trim() ? `${link.title.trim()} (${link.url})` : link.url}
      </span>
    {/if}
    <Button type="button" size="sm" variant="ghost" class="shrink-0" onclick={startEditing}>
      Edit
    </Button>
  </div>
{:else}
  <div class="flex h-10 items-center">
    <Button type="button" id={fieldId} size="sm" variant="ghost" onclick={startEditing}>
      {addLabel}
    </Button>
  </div>
{/if}
