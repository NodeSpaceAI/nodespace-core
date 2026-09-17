<script lang="ts">
  import { Card, CardHeader } from '$lib/components/ui/card';
  import { Switch } from '$lib/components/ui/switch';
  import { labsFlags } from '$lib/stores/labs-flags.svelte';
</script>

<div class="max-w-[640px]">
  <h2 class="text-foreground mb-1.5 text-xl font-semibold">Labs</h2>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    Early looks at features that are still being built. Turn one on if you're brave.
  </p>

  <!-- AI Chat -->
  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center justify-between gap-2.5">
        <div class="flex items-center gap-2.5">
          <span class="text-foreground text-[0.9375rem] font-semibold">AI Chat</span>
        </div>
        <Switch
          checked={labsFlags.aiChatEnabled}
          onCheckedChange={(checked) => (labsFlags.aiChatEnabled = checked)}
          aria-label={labsFlags.aiChatEnabled ? 'Disable AI Chat' : 'Enable AI Chat'}
        />
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Chat with an AI directly in your workspace, in its own node. Experimental — may not work
        correctly.
      </p>
    </CardHeader>
  </Card>

  <!-- Team synchronization -->
  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center justify-between gap-2.5">
        <div class="flex items-center gap-2.5">
          <span class="text-foreground text-[0.9375rem] font-semibold">Team synchronization</span>
        </div>
        <Switch
          checked={labsFlags.syncEnabled}
          onCheckedChange={(checked) => (labsFlags.syncEnabled = checked)}
          aria-label={labsFlags.syncEnabled
            ? 'Disable Team synchronization'
            : 'Enable Team synchronization'}
        />
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Share collections and collaborate with your team across devices. Experimental — may not
        work correctly.
      </p>
    </CardHeader>
  </Card>
</div>
