<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import { cn } from '$lib/utils';
  import { formatAgentList } from '$lib/utils/agent-names';

  const log = createLogger('IntegrationsSettings');

  interface IntegrationStatus {
    pathAlreadyConfigured: boolean;
  }

  interface DetectedAgents {
    /** Agent ids the skill installer would target — see agent-names.ts. */
    agents: string[];
    /** True when detection could not run at all (vs. finding nothing). */
    detectionFailed: boolean;
  }

  interface SkippedAgent {
    agent: string;
    reason: string;
  }

  interface SkillSetupResult {
    success: boolean;
    agentsInstalled: string[];
    agentsSkipped: SkippedAgent[];
    cliOnPath: boolean;
    cliWarning: string | null;
    error: string | null;
  }

  let status = $state<IntegrationStatus | null>(null);
  // Fetched once on mount, separately from `status`: answering it costs a
  // subprocess, and nothing on this panel changes which agents exist on the
  // machine (installing the skill INTO an agent does not create one).
  let detected = $state<DetectedAgents | null>(null);
  let skillResult = $state<SkillSetupResult | null>(null);

  let pathWorking = $state(false);
  let skillWorking = $state(false);
  let pathFeedback = $state<{ ok: boolean; message: string } | null>(null);
  let skillFeedback = $state<{ ok: boolean; message: string } | null>(null);

  async function loadStatus() {
    try {
      status = await invoke<IntegrationStatus>('get_integrations_status');
      skillResult = await invoke<SkillSetupResult>('get_skill_setup_status');
      detected = await invoke<DetectedAgents>('detect_agents');
    } catch (err) {
      log.warn('Could not load integration status', err);
    }
  }

  onMount(loadStatus);

  // ── PATH ──────────────────────────────────────────────────────────────────

  async function addToPath() {
    pathWorking = true;
    pathFeedback = null;
    try {
      await invoke('configure_path');
      status = await invoke<IntegrationStatus>('get_integrations_status');
      pathFeedback = { ok: true, message: '~/.nodespace/bin added to PATH in your shell profiles.' };
    } catch (err) {
      pathFeedback = { ok: false, message: toError(err).message };
    } finally {
      pathWorking = false;
    }
  }

  async function removeFromPath() {
    pathWorking = true;
    pathFeedback = null;
    try {
      await invoke('remove_from_path');
      status = await invoke<IntegrationStatus>('get_integrations_status');
      pathFeedback = { ok: true, message: '~/.nodespace/bin removed from PATH in your shell profiles.' };
    } catch (err) {
      pathFeedback = { ok: false, message: toError(err).message };
    } finally {
      pathWorking = false;
    }
  }

  // ── SKILL ─────────────────────────────────────────────────────────────────

  async function addSkill() {
    skillWorking = true;
    skillFeedback = null;
    try {
      const result = await invoke<SkillSetupResult>('install_skill', { force: true });
      skillResult = result;
      status = await invoke<IntegrationStatus>('get_integrations_status');
      if (result.success) {
        const installed = result.agentsInstalled.length > 0
          ? `Installed into: ${formatAgentList(result.agentsInstalled)}.`
          : 'Already up to date — no changes needed.';
        const skipped = result.agentsSkipped.length > 0
          ? ` ${result.agentsSkipped.map((s) => `${s.agent}: ${s.reason}`).join('; ')}.`
          : '';
        skillFeedback = { ok: true, message: installed + skipped };
      } else {
        skillFeedback = { ok: false, message: result.error ?? 'Skill installation failed.' };
      }
    } catch (err) {
      skillFeedback = { ok: false, message: toError(err).message };
    } finally {
      skillWorking = false;
    }
  }

  async function removeSkill() {
    skillWorking = true;
    skillFeedback = null;
    try {
      await invoke('remove_skill');
      skillResult = await invoke<SkillSetupResult>('get_skill_setup_status');
      status = await invoke<IntegrationStatus>('get_integrations_status');
      skillFeedback = { ok: true, message: 'NodeSpace skill removed from every agent it was installed into.' };
    } catch (err) {
      skillFeedback = { ok: false, message: toError(err).message };
    } finally {
      skillWorking = false;
    }
  }

  const pathIsConfigured = $derived(status?.pathAlreadyConfigured ?? false);
  const skillIsInstalled = $derived(skillResult?.success ?? false);
  const detectedAgents = $derived(detected?.agents ?? []);
  // A detection failure must not present as "no agents installed" — the
  // controls stay enabled so the install attempt can surface a real error.
  const anyAgentDetected = $derived(detectedAgents.length > 0 || detected?.detectionFailed === true);
  const detectedAgentsLabel = $derived(
    detectedAgents.length > 0 ? formatAgentList(detectedAgents) : 'your coding agents'
  );
</script>

<div class="max-w-[640px]">
  <h2 class="text-foreground mb-1.5 text-xl font-semibold">Integrations</h2>
  <p class="text-muted-foreground mb-6 text-sm leading-relaxed">
    Manage how NodeSpace integrates with your shell and AI agents.
  </p>

  <!-- CLI PATH -->
  <Card class="mb-4 gap-0 rounded-lg py-0">
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center gap-2.5">
        <span class="text-foreground text-[0.9375rem] font-semibold">CLI PATH</span>
        {#if status === null}
          <Badge variant="secondary">Checking…</Badge>
        {:else if pathIsConfigured}
          <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Configured</Badge>
        {:else}
          <Badge class="border-amber-500/25 bg-amber-500/10 text-amber-700">Not configured</Badge>
        {/if}
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        Adds <code class="bg-muted text-foreground rounded px-1 py-0.5 text-[0.8125rem]">~/.nodespace/bin</code> to your PATH so you can run
        <code class="bg-muted text-foreground rounded px-1 py-0.5 text-[0.8125rem]">nodespace</code> from any terminal session.
      </p>
    </CardHeader>
    <CardContent class="px-5 pb-5">
      {#if pathFeedback !== null}
        <div class={pathFeedback.ok
          ? 'mb-4 rounded-md border border-green-500/25 bg-green-500/10 px-3.5 py-2.5 text-sm leading-relaxed text-green-700'
          : 'border-destructive/30 bg-destructive/10 text-destructive-foreground mb-4 rounded-md border px-3.5 py-2.5 text-sm leading-relaxed'
        }>
          {pathFeedback.message}
        </div>
      {/if}
      <div class="flex gap-3">
        {#if pathIsConfigured}
          <Button variant="outline" size="sm" onclick={removeFromPath} disabled={pathWorking}>
            {pathWorking ? 'Removing…' : 'Remove from PATH'}
          </Button>
        {:else}
          <Button size="sm" onclick={addToPath} disabled={pathWorking}>
            {pathWorking ? 'Adding…' : 'Add to PATH'}
          </Button>
        {/if}
      </div>
    </CardContent>
  </Card>

  <!-- Agent Skill -->
  <Card class={cn('mb-4 gap-0 rounded-lg py-0', !anyAgentDetected && 'opacity-60')}>
    <CardHeader class="p-5 pb-4">
      <div class="mb-1.5 flex items-center gap-2.5">
        <span class="text-foreground text-[0.9375rem] font-semibold">Agent Skill</span>
        {#if status === null || detected === null}
          <Badge variant="secondary">Checking…</Badge>
        {:else if !anyAgentDetected}
          <Badge variant="secondary">No agents detected</Badge>
        {:else if skillIsInstalled}
          <Badge class="border-green-500/25 bg-green-500/10 text-green-700">Installed</Badge>
        {:else}
          <Badge class="border-amber-500/25 bg-amber-500/10 text-amber-700">Not installed</Badge>
        {/if}
      </div>
      <p class="text-muted-foreground m-0 text-sm leading-relaxed">
        {#if !anyAgentDetected}
          No supported coding agent is installed. Install one to enable NodeSpace tools in its CLI
          sessions.
        {:else}
          NodeSpace tools available in {detectedAgentsLabel} sessions via a
          <code class="bg-muted text-foreground rounded px-1 py-0.5 text-[0.8125rem]">SKILL.md</code>
          in each agent's skills directory.
        {/if}
      </p>
    </CardHeader>
    <CardContent class="px-5 pb-5">
      {#if anyAgentDetected && skillResult?.cliWarning}
        <div class="mb-4 flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/10 px-3.5 py-2.5 text-sm leading-relaxed text-amber-700">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="15" height="15" class="mt-0.5 shrink-0" aria-hidden="true">
            <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
          <span>{skillResult.cliWarning}</span>
        </div>
      {/if}

      {#if skillFeedback !== null}
        <div class={skillFeedback.ok
          ? 'mb-4 rounded-md border border-green-500/25 bg-green-500/10 px-3.5 py-2.5 text-sm leading-relaxed text-green-700'
          : 'border-destructive/30 bg-destructive/10 text-destructive-foreground mb-4 rounded-md border px-3.5 py-2.5 text-sm leading-relaxed'
        }>
          {skillFeedback.message}
        </div>
      {/if}

      <div class="flex gap-3">
        {#if !anyAgentDetected}
          <Button size="sm" disabled>Add Skill</Button>
        {:else if skillIsInstalled}
          <Button size="sm" onclick={addSkill} disabled={skillWorking}>
            {skillWorking ? 'Reinstalling…' : 'Reinstall Skill'}
          </Button>
          <Button variant="outline" size="sm" onclick={removeSkill} disabled={skillWorking}>
            {skillWorking ? 'Removing…' : 'Remove Skill'}
          </Button>
        {:else}
          <Button size="sm" onclick={addSkill} disabled={skillWorking}>
            {skillWorking ? 'Installing…' : 'Add Skill'}
          </Button>
        {/if}
      </div>
    </CardContent>
  </Card>
</div>
