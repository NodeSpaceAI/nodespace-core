<script lang="ts">
  import { onMount } from 'svelte';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardContent } from '$lib/components/ui/card';
  import {
    installMethodology,
    listMethodologies,
    summarizeReport,
    type Methodology
  } from '$lib/services/methodology-service';

  const log = createLogger('MethodologySettings');

  let methodologies = $state<Methodology[]>([]);
  let loading = $state(true);
  let installing = $state<string | null>(null);
  let feedback = $state<{ ok: boolean; message: string } | null>(null);

  onMount(async () => {
    try {
      methodologies = await listMethodologies();
    } catch (err) {
      log.warn('Could not load methodologies', err);
      feedback = { ok: false, message: toError(err).message };
    } finally {
      loading = false;
    }
  });

  async function install(methodology: Methodology) {
    installing = methodology.id;
    feedback = null;
    try {
      const report = await installMethodology(methodology.id);
      // A partial install resolves rather than throwing, so `success` is what
      // decides how this reads — not whether we reached the catch.
      feedback = { ok: report.success, message: summarizeReport(report) };
    } catch (err) {
      log.error('Methodology install failed', err);
      feedback = { ok: false, message: toError(err).message };
    } finally {
      installing = null;
    }
  }
</script>

<div>
  <h2 class="text-foreground mb-1 text-lg font-semibold">Work Tracking</h2>
  <p class="text-muted-foreground mb-5 text-sm leading-relaxed">
    Install a ready-made setup for a workflow you already know. Each one adds node types, a few
    automations and some guidance — all ordinary content you can inspect, edit or delete
    afterwards.
  </p>

  {#if feedback !== null}
    <div
      class={feedback.ok
        ? 'mb-4 rounded-md border border-green-500/25 bg-green-500/10 px-3.5 py-2.5 text-sm leading-relaxed text-green-700'
        : 'border-destructive/30 bg-destructive/10 text-destructive mb-4 rounded-md border px-3.5 py-2.5 text-sm leading-relaxed'}
      data-testid="methodology-feedback"
    >
      {feedback.message}
    </div>
  {/if}

  {#if loading}
    <p class="text-muted-foreground text-sm">Loading…</p>
  {:else if methodologies.length === 0}
    <p class="text-muted-foreground text-sm">No methodologies are available in this build.</p>
  {:else}
    {#each methodologies as methodology (methodology.id)}
      <Card class="mb-4 gap-0 rounded-lg py-0">
        <CardHeader class="p-5 pb-4">
          <div class="mb-1.5 flex items-center gap-2.5">
            <span class="text-foreground text-[0.9375rem] font-semibold">{methodology.name}</span>
          </div>
          <p class="text-muted-foreground m-0 text-sm leading-relaxed">
            {methodology.description}
          </p>
        </CardHeader>
        <CardContent class="px-5 pb-5">
          <Button
            size="sm"
            onclick={() => install(methodology)}
            disabled={installing !== null}
            data-testid={`install-${methodology.id}`}
          >
            {installing === methodology.id ? 'Installing…' : 'Install'}
          </Button>
        </CardContent>
      </Card>
    {/each}

    <p class="text-muted-foreground mt-4 text-xs leading-relaxed">
      Installing twice is safe. If a name is already taken, the new type is added under a
      different one and named in the result — nothing you already have is changed or replaced.
    </p>
  {/if}
</div>
