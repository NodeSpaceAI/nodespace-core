<!--
  Your identity (ADR-037) — edit path for the seeded local-user
  PersonNode's name/email, and the backfill target for an install whose
  seeded person was never filled in (see OnboardingWizard's identity step,
  which points here: "Settings → Database").

  Lives in the Database settings section because the identity IS the
  database's owner (`has_role` edge to the DatabaseSettingsNode singleton) —
  the same person `seed_database_settings_if_needed` wires ownership to.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import { Input } from '$lib/components/ui/input';

  const log = createLogger('IdentityCard');

  interface LocalIdentity {
    nodeId: string;
    firstName: string;
    lastName: string;
    email: string;
    isBlank: boolean;
  }

  let identity = $state<LocalIdentity | null>(null);
  let firstName = $state('');
  let lastName = $state('');
  let email = $state('');
  let saving = $state(false);
  let feedback = $state<{ ok: boolean; message: string } | null>(null);

  async function loadIdentity() {
    try {
      identity = await invoke<LocalIdentity | null>('get_local_identity');
      if (identity) {
        firstName = identity.firstName;
        lastName = identity.lastName;
        email = identity.email;
      }
    } catch (err) {
      log.warn('Could not load local identity', err);
    }
  }

  onMount(loadIdentity);

  async function save() {
    saving = true;
    feedback = null;
    try {
      identity = await invoke<LocalIdentity>('set_local_identity', {
        firstName: firstName.trim(),
        lastName: lastName.trim(),
        email: email.trim(),
      });
      firstName = identity.firstName;
      lastName = identity.lastName;
      email = identity.email;
      feedback = { ok: true, message: 'Saved.' };
    } catch (err) {
      feedback = { ok: false, message: toError(err).message };
    } finally {
      saving = false;
    }
  }

  const isBlank = $derived(identity?.isBlank ?? false);
</script>

<Card class="mb-4 gap-0 rounded-lg py-0">
  <CardHeader class="p-5 pb-4">
    <div class="mb-1.5 flex items-center gap-2.5">
      <span class="text-foreground text-[0.9375rem] font-semibold">Your identity</span>
      {#if identity === null}
        <Badge variant="secondary">Loading…</Badge>
      {:else if isBlank}
        <Badge class="border-amber-500/25 bg-amber-500/10 text-amber-700">Not set</Badge>
      {:else}
        <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Set</Badge>
      {/if}
    </div>
    <p class="text-muted-foreground m-0 text-sm leading-relaxed">
      Identifies you as the owner of this database and the default assignee for new tasks.
    </p>
  </CardHeader>
  <CardContent class="px-5 pb-5">
    {#if feedback !== null}
      <div
        class={feedback.ok
          ? 'mb-4 rounded-md border border-green-500/25 bg-green-500/10 px-3.5 py-2.5 text-sm leading-relaxed text-green-700'
          : 'border-destructive/30 bg-destructive/10 text-destructive-foreground mb-4 rounded-md border px-3.5 py-2.5 text-sm leading-relaxed'}
      >
        {feedback.message}
      </div>
    {/if}
    <div class="mb-3 flex flex-col gap-1.5">
      <label for="identity-card-first-name" class="text-muted-foreground text-xs font-medium"
        >First name</label
      >
      <Input
        id="identity-card-first-name"
        type="text"
        bind:value={firstName}
        placeholder="First name"
      />
    </div>
    <div class="mb-3 flex flex-col gap-1.5">
      <label for="identity-card-last-name" class="text-muted-foreground text-xs font-medium"
        >Last name</label
      >
      <Input
        id="identity-card-last-name"
        type="text"
        bind:value={lastName}
        placeholder="Last name"
      />
    </div>
    <div class="mb-4 flex flex-col gap-1.5">
      <label for="identity-card-email" class="text-muted-foreground text-xs font-medium"
        >Email</label
      >
      <Input
        id="identity-card-email"
        type="email"
        bind:value={email}
        placeholder="you@example.com"
      />
    </div>
    <Button size="sm" onclick={save} disabled={saving}>
      {saving ? 'Saving…' : 'Save'}
    </Button>
  </CardContent>
</Card>
