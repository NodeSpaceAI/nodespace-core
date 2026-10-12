<!--
  The one way a viewer offers the chat bound to its node (ADR-100 §1).

  A click opens a new chat bound to `nodeId` beside the viewer, which stays in
  view. Each click makes a new chat. Why a click opened none is in `error`,
  for the viewer to show where its layout has room; it is empty otherwise.
-->
<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { aiChatsData } from '$lib/stores/ai-chats.svelte';

  let {
    nodeId,
    label = 'Edit',
    class: className = '',
    error = $bindable('')
  }: {
    nodeId: string;
    label?: string;
    class?: string;
    error?: string;
  } = $props();

  async function open() {
    error = '';
    const chat = await aiChatsData.openBoundChat(nodeId);
    if (!chat) {
      error = aiChatsData.createError;
      return;
    }
    await getNavigationService().navigateToNodeInOtherPane(chat.id);
  }
</script>

<Button
  class={className}
  variant="outline"
  size="sm"
  disabled={aiChatsData.createBusy}
  onclick={open}
>
  {label}
</Button>
