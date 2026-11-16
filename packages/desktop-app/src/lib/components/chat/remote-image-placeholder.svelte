<!--
  RemoteImagePlaceholder Component

  Stands in for a remote image in an AI chat reply. Remote images never load on
  their own (the content security policy and the chat sanitiser both block
  them), so the reply shows what it asked for instead: the alt text, the host
  and the full address as plain text, and a "Load image" button. Pressing the
  button loads that one image through the app process and shows it; nothing is
  requested before the press, and no other image is touched.

  Mounted by chat-markdown.svelte on the span that renderChatImage writes.
-->

<script lang="ts">
  import { loadRemoteImage, describeRemoteImageError, RemoteImageLoadError } from '$lib/services/remote-image';

  let { url, alt = '' }: { url: string; alt?: string } = $props();

  type LoadState = 'idle' | 'loading' | 'loaded' | 'failed';

  let loadState = $state<LoadState>('idle');
  let dataUrl = $state('');
  let failure = $state('');

  /** `https:` is the only scheme that can be loaded. */
  const isHttps = $derived(/^https:\/\//i.test(url));
  const host = $derived.by(() => {
    try {
      return new URL(url).host;
    } catch {
      return '';
    }
  });

  async function load(): Promise<void> {
    loadState = 'loading';
    failure = '';
    try {
      dataUrl = await loadRemoteImage(url);
      loadState = 'loaded';
    } catch (error) {
      failure = describeRemoteImageError(
        error instanceof RemoteImageLoadError ? error.code : 'network'
      );
      loadState = 'failed';
    }
  }
</script>

{#if loadState === 'loaded'}
  <img class="ns-remote-image" src={dataUrl} {alt} />
{:else}
  <span class="ns-remote-image-card">
    <span class="ns-remote-image-alt">{alt || 'Image'}</span>
    <span class="ns-remote-image-url" title={url}>{url}</span>
    {#if isHttps}
      <button
        type="button"
        class="ns-remote-image-load"
        disabled={loadState === 'loading'}
        onclick={load}
      >
        {loadState === 'loading' ? 'Loading…' : 'Load image'}
      </button>
      <span class="ns-remote-image-note">
        Loading sends this full address, including any query string, to {host}, which also sees your IP address.
      </span>
    {:else}
      <span class="ns-remote-image-note">Only https images can be loaded.</span>
    {/if}
    {#if loadState === 'failed'}
      <span class="ns-remote-image-error" role="alert">{failure}</span>
    {/if}
  </span>
{/if}

<style>
  .ns-remote-image-card {
    display: inline-flex;
    flex-direction: column;
    gap: 0.25rem;
    align-items: flex-start;
    max-width: 100%;
    margin: 0.25rem 0;
    padding: 0.5rem 0.75rem;
    border: 1px dashed hsl(var(--border));
    border-radius: 0.5rem;
    background: hsl(var(--muted) / 0.4);
    font-size: 0.85em;
  }

  .ns-remote-image-alt {
    font-weight: 600;
  }

  .ns-remote-image-url {
    max-width: 100%;
    overflow-wrap: anywhere;
    color: hsl(var(--muted-foreground));
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.9em;
  }

  .ns-remote-image-load {
    padding: 0.25rem 0.625rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font: inherit;
    cursor: pointer;
  }

  .ns-remote-image-load:hover:not(:disabled) {
    background: hsl(var(--muted));
  }

  .ns-remote-image-load:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .ns-remote-image-note {
    color: hsl(var(--muted-foreground));
    font-size: 0.9em;
  }

  .ns-remote-image-error {
    color: hsl(var(--destructive));
  }

  .ns-remote-image {
    max-width: 100%;
    height: auto;
    border-radius: 0.375rem;
  }
</style>
