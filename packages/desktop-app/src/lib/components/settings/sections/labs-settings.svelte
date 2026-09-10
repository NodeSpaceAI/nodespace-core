<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import { Card, CardHeader } from '$lib/components/ui/card';
</script>

<div class="max-w-[640px]">
  <h2 class="text-foreground mb-1.5 text-xl font-semibold">Labs</h2>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    Early looks at features that are still being built. Nothing here is configurable yet.
  </p>

  <!-- AI Chat -->
  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center gap-2.5">
        <span class="text-foreground text-[0.9375rem] font-semibold">AI Chat</span>
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Chat with an AI directly in your workspace, in its own node. There are no AI Chat
        preferences to configure here yet.
      </p>
    </CardHeader>
  </Card>

  <!-- Team synchronization -->
  <Card
    class="mb-4 gap-0 rounded-lg py-0 opacity-60"
    role="group"
    aria-disabled="true"
    aria-label="Team synchronization: In development, not yet available"
  >
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center gap-2.5">
        <span class="text-foreground text-[0.9375rem] font-semibold">Team synchronization</span>
        <Badge variant="secondary">In development</Badge>
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Share collections and collaborate with your team across devices. Not available yet.
      </p>
    </CardHeader>
  </Card>
</div>
