<!--
  TeamCollaborationCard — the Labs page's contact entry point (ADR-084 §1), and
  core's default content for the `collaboration.entry` replaceable slot.

  It opens the contact link through `openUrl` and does nothing else: it reads no
  account state, sends nothing and calls no service.
-->
<script lang="ts">
  import { Card, CardHeader } from '$lib/components/ui/card';
  import { TEAM_COLLABORATION_CONTACT_URL } from '$lib/constants/contact';
  import { openUrl } from '$lib/utils/external-links';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('TeamCollaborationCard');

  // The anchor keeps its href for copy-link and accessibility, but the click
  // goes through `openUrl` so the system mail client opens instead of the
  // webview navigating.
  function openContact(event: MouseEvent) {
    event.preventDefault();
    openUrl(TEAM_COLLABORATION_CONTACT_URL).catch((error: unknown) => {
      log.error('Failed to open the contact link', { error });
    });
  }
</script>

<Card class="mb-4 gap-0 rounded-lg py-0">
  <CardHeader class="p-5 pb-4">
    <div class="mb-1.5 flex items-center gap-2.5">
      <span class="text-foreground text-[0.9375rem] font-semibold">Team synchronization</span>
    </div>
    <p class="text-muted-foreground m-0 text-sm leading-relaxed">
      Want team collaboration? <a
        href={TEAM_COLLABORATION_CONTACT_URL}
        onclick={openContact}
        class="text-foreground font-medium underline underline-offset-2 hover:text-primary"
        >Contact us</a
      >
    </p>
  </CardHeader>
</Card>
