<script lang="ts">
  import { Card, CardHeader } from '$lib/components/ui/card';
  import { Switch } from '$lib/components/ui/switch';
  import { labsFlags } from '$lib/stores/labs-flags.svelte';
  import ReplaceableSlotOutlet from '$lib/plugins/replaceable-slot-outlet.svelte';
  import TeamCollaborationCard from './team-collaboration-card.svelte';
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

  <!-- Playbooks -->
  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center justify-between gap-2.5">
        <div class="flex items-center gap-2.5">
          <span class="text-foreground text-[0.9375rem] font-semibold">Playbooks</span>
        </div>
        <Switch
          checked={labsFlags.playbooksEnabled}
          onCheckedChange={(checked) => (labsFlags.playbooksEnabled = checked)}
          aria-label={labsFlags.playbooksEnabled ? 'Disable Playbooks' : 'Enable Playbooks'}
        />
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Show the Plays section in the sidebar, which lists your plays and whether each is on.
        Plays keep running whether or not this is on.
      </p>
    </CardHeader>
  </Card>

  <!-- Team collaboration: core's contact card, unless an extension replaces it. -->
  <ReplaceableSlotOutlet name="collaboration.entry">
    {#snippet defaultContent()}
      <TeamCollaborationCard />
    {/snippet}
  </ReplaceableSlotOutlet>
</div>
