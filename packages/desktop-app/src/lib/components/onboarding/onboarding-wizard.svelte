<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { focusTrap } from '$lib/actions/focus-trap';
  import { splitFullName } from '$lib/utils/split-full-name';
  import { displayAgentName, formatAgentList } from '$lib/utils/agent-names';

  const log = createLogger('OnboardingWizard');

  let {
    open = false,
    onClose,
    identityOnly = false,
  }: {
    open: boolean;
    onClose: () => void;
    /**
     * Backfill nudge: an already-onboarded install whose seeded
     * local person is still blank gets ONLY the identity step (no
     * path/skill/summary) — a lightweight, dismissible one-time reminder
     * rather than re-running the whole first-launch wizard.
     */
    identityOnly?: boolean;
  } = $props();

  // ── types ──────────────────────────────────────────────────────────────────

  type WizardStep = 'identity' | 'path' | 'skill' | 'summary';

  interface OnboardingStatus {
    completed: boolean;
    pathConfigured: boolean;
    skillConfigured: boolean;
    pathAlreadyConfigured: boolean;
  }

  interface DetectedAgents {
    /** Agent ids the skill installer would target — see agent-names.ts. */
    agents: string[];
    /**
     * True when detection could not run at all, as opposed to running and
     * finding nothing. The skill step is still offered in that case (with
     * generic wording) rather than silently vanishing — the install attempt
     * itself then surfaces a real, actionable error.
     */
    detectionFailed: boolean;
  }

  interface LocalIdentity {
    nodeId: string;
    firstName: string;
    lastName: string;
    email: string;
    isBlank: boolean;
  }

  interface IdentityPrefill {
    name: string | null;
    email: string | null;
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
    failureIsNew: boolean;
  }

  // ── state ──────────────────────────────────────────────────────────────────

  let currentStep = $state<WizardStep>('path');
  let isLoading = $state(false);
  let stepSuccess = $state(false);
  let stepError = $state<string | null>(null);

  // ── identity step state ──────────────────────────────────────────────────

  // Whether to include the 'identity' step at all — true only while the
  // seeded local person is still blank; a node that already has a name/email
  // (e.g. this wizard re-running after a prior backfill save) is left alone.
  let showIdentity = $state(false);
  let identityFirstName = $state('');
  let identityLastName = $state('');
  let identityEmail = $state('');
  let identityPrefilled = $state(false);
  let identityDone = $state(false);
  // Set when the main wizard's identity step was shown and the user
  // explicitly clicked Skip (never set for identityOnly mode, which has its
  // own dismiss path — see skipIdentity below). Threaded through to
  // `complete_onboarding` so declining here also suppresses the separate
  // backfill nudge on the next launch, instead of re-asking the very next
  // thing the user just said no to.
  let identitySkipped = $state(false);

  // The dialog content element — the focus-trap target and the focus anchor for
  // step transitions (see the $effect below).
  let dialogEl = $state<HTMLElement>();

  // Which steps are active (some may be skipped if prerequisites missing)
  let showSkill = $state(false);

  // Which agents the install will actually target, known BEFORE the install
  // runs — so the question the skill step asks ("Add NodeSpace to ...?") names
  // the same agents the success message afterwards confirms. Empty until
  // detect_agents resolves, and whenever detection failed; the wording falls
  // back to a generic form in that case rather than guessing.
  let detectedAgents = $state<string[]>([]);

  // What was actually configured (for summary)
  let pathDone = $state(false);
  let skillDone = $state(false);

  // The real result of the skill install -- which agents actually got
  // files, and which were detected but had nothing to install. Without
  // this the wizard could only ever say "Claude Code skill" regardless of
  // what actually happened, so a correct multi-agent install (e.g. Claude
  // Code AND Antigravity CLI both installed) read as if only Claude Code was
  // handled at all.
  let skillResult = $state<SkillSetupResult | null>(null);

  // Whether the PATH export was already present before we ran
  let pathWasAlreadyConfigured = $state(false);

  // ── derived step sequence ──────────────────────────────────────────────────

  const stepSequence = $derived(
    (() => {
      if (identityOnly) return ['identity'] as WizardStep[];
      const steps: WizardStep[] = [];
      if (showIdentity) steps.push('identity');
      steps.push('path');
      if (showSkill) steps.push('skill');
      steps.push('summary');
      return steps;
    })()
  );

  function nextStep() {
    const seq = stepSequence;
    const idx = seq.indexOf(currentStep);
    if (idx !== -1 && idx < seq.length - 1) {
      currentStep = seq[idx + 1];
      stepSuccess = false;
      stepError = null;
    }
  }

  // ── mount: probe environment ───────────────────────────────────────────────

  onMount(() => {
    if (identityOnly) {
      currentStep = 'identity';
    } else {
      invoke<OnboardingStatus>('check_onboarding_status')
        .then((status) => {
          pathWasAlreadyConfigured = status.pathAlreadyConfigured;
          log.debug('Onboarding status loaded', {
            pathAlreadyConfigured: status.pathAlreadyConfigured,
          });
        })
        .catch((err) => {
          log.warn('Could not load onboarding status', err);
        });

      // Separate from the status call above: answering this costs a
      // subprocess, and only the skill step needs it.
      invoke<DetectedAgents>('detect_agents')
        .then((result) => {
          detectedAgents = result?.agents ?? [];
          // Offer the step when agents were found, and also when detection
          // could not run — the user may well have an agent installed, and a
          // question with generic wording beats silently dropping the step.
          showSkill = detectedAgents.length > 0 || result?.detectionFailed === true;
          log.debug('Agent detection complete', {
            detectedAgents,
            detectionFailed: result?.detectionFailed,
            showSkill,
          });
        })
        .catch((err) => {
          // The command itself failing is the same situation as detection
          // failing inside it: offer the step with generic wording.
          showSkill = true;
          log.warn('Could not detect agents', err);
        });
    }
    loadIdentity();
  });

  /**
   * Load the seeded local person's current identity. When it's
   * still blank, include the 'identity' step (asked first, before
   * path/skill) and fetch a best-effort git/OS prefill to show for
   * confirmation — never written until the user explicitly saves.
   */
  async function loadIdentity() {
    try {
      const identity = await invoke<LocalIdentity | null>('get_local_identity');
      if (!identity) {
        log.debug('No local person node to prompt an identity for');
        return;
      }
      if (!identity.isBlank) {
        // Already filled in (e.g. re-opening after a prior save) — nothing to ask.
        return;
      }
      showIdentity = true;
      if (!identityOnly && currentStep === 'path') {
        currentStep = 'identity';
      }
      try {
        const prefill = await invoke<IdentityPrefill>('get_identity_prefill');
        if (prefill.name) {
          const { firstName, lastName } = splitFullName(prefill.name);
          identityFirstName = firstName;
          identityLastName = lastName;
        }
        if (prefill.email) identityEmail = prefill.email;
        identityPrefilled = Boolean(prefill.name || prefill.email);
      } catch (err) {
        log.debug('No identity prefill available', err);
      }
    } catch (err) {
      log.warn('Could not load local identity', err);
    }
  }

  async function handleSaveIdentity() {
    isLoading = true;
    stepError = null;
    try {
      await invoke('set_local_identity', {
        firstName: identityFirstName.trim(),
        lastName: identityLastName.trim(),
        email: identityEmail.trim(),
      });
      identityDone = true;
      log.info('Local identity saved');
      if (identityOnly) {
        await invoke('dismiss_identity_backfill_prompt').catch((err) =>
          log.debug('Could not persist identity-prompt dismissal', err)
        );
        onClose();
        return;
      }
      stepSuccess = true;
    } catch (err) {
      stepError = toError(err).message;
      log.error('Failed to save local identity', err);
    } finally {
      isLoading = false;
    }
  }

  function skipIdentity() {
    log.debug('Skipped identity step');
    if (identityOnly) {
      invoke('dismiss_identity_backfill_prompt').catch((err) =>
        log.debug('Could not persist identity-prompt dismissal', err)
      );
      onClose();
      return;
    }
    identitySkipped = true;
    nextStep();
  }

  // ── step actions ───────────────────────────────────────────────────────────

  async function handleConfigurePath() {
    isLoading = true;
    stepError = null;
    try {
      await invoke('configure_path');
      pathDone = true;
      stepSuccess = true;
      log.info('PATH configured successfully');
    } catch (err) {
      stepError = toError(err).message;
      log.error('Failed to configure PATH', err);
    } finally {
      isLoading = false;
    }
  }

  async function handleConfigureSkill() {
    isLoading = true;
    stepError = null;
    try {
      skillResult = await invoke<SkillSetupResult>('configure_skill');
      skillDone = true;
      stepSuccess = true;
      log.info('Skill configured successfully', { agentsInstalled: skillResult.agentsInstalled });
    } catch (err) {
      stepError = toError(err).message;
      log.error('Failed to configure skill', err);
    } finally {
      isLoading = false;
    }
  }

  // The agent list as it appears in the skill step's question. Falls back to
  // "your coding agents" when detection came back empty (a failed or
  // not-yet-resolved probe) — vague, but never wrong, which a guess at one
  // specific agent would not be.
  const detectedAgentsLabel = $derived(
    detectedAgents.length > 0 ? formatAgentList(detectedAgents) : 'your coding agents'
  );

  // Whether Claude Code is among the targets — the plugin-marketplace caveat
  // in the body below is specific to it (no other harness has one), so it is
  // shown only when it can actually apply.
  const claudeCodeTargeted = $derived(detectedAgents.includes('claude-code'));

  function skipCurrentStep() {
    log.debug('Skipped step', { step: currentStep });
    nextStep();
  }

  async function finishWizard() {
    try {
      await invoke('complete_onboarding', {
        pathConfigured: pathDone,
        skillConfigured: skillDone,
        identitySkipped,
      });
      log.info('Onboarding completed', { pathDone, skillDone, identitySkipped });
    } catch (err) {
      log.warn('Could not persist onboarding completion', err);
    }
    onClose();
  }

  // ── multi-step focus management ────────────────────────────────────────────

  // `focusTrap` moves focus into the dialog on open, traps Tab, and handles
  // Escape — but it does so once, on mount. This wizard swaps its primary action
  // button on every transition (advancing a step, or a configure step
  // succeeding), which unmounts the button that had focus and drops focus to
  // <body>, where the trap's key handler can no longer see Escape/Tab. Re-anchor
  // focus on the new step's primary action after each such transition. Keyed on
  // exactly the state that changes which primary button is shown, so it never
  // steals focus mid-interaction (there are no text inputs to interrupt).
  $effect(() => {
    const refocusKey = `${currentStep}:${stepSuccess}:${pathWasAlreadyConfigured}`;
    if (!dialogEl) return;
    log.debug('Re-anchoring wizard focus', { refocusKey });
    (dialogEl.querySelector<HTMLElement>('.primary-button') ?? dialogEl).focus();
  });
</script>

{#if open}
  <div class="onboarding-backdrop" onclick={onClose} role="presentation" tabindex="-1">
    <div
      class="onboarding-dialog"
      bind:this={dialogEl}
      use:focusTrap={{ onEscape: onClose }}
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      role="dialog"
      aria-modal="true"
      aria-label="First-launch setup"
      tabindex="0"
    >
      <!-- Close button -->
      <button class="close-button" onclick={onClose} aria-label="Close dialog">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="18" height="18">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>

      <!-- Step indicator (skipped for the single-step backfill nudge) -->
      {#if !identityOnly}
        <div class="step-indicator" aria-hidden="true">
          {#each stepSequence as step (step)}
            <span
              class="step-dot"
              class:active={currentStep === step}
              class:done={stepSequence.indexOf(step) < stepSequence.indexOf(currentStep)}
            ></span>
          {/each}
        </div>
      {/if}

      <!-- ── Identity step ─────────────────────────────────────────────── -->
      {#if currentStep === 'identity'}
        <div class="onboarding-header">
          <h2>Who are you?</h2>
          <p>
            This identifies you as the owner of this database and is used as the default
            assignee for new tasks. You can change it any time in
            <strong>Settings → Database</strong>. Skipping leaves it blank — nothing here is
            required.
          </p>
        </div>

        {#if identityPrefilled && !identityDone}
          <div class="info-banner">
            Suggested from your git/OS account. Review and edit below — nothing is saved until
            you click Save.
          </div>
        {/if}

        {#if identityDone && !identityOnly}
          <div class="success-banner">Saved. You're recorded as the owner of this database.</div>
          <div class="step-actions">
            <button class="primary-button" onclick={nextStep}>Next</button>
          </div>
        {:else}
          {#if stepError}
            <div class="error-banner">{stepError}</div>
          {/if}
          <div class="field">
            <label for="identity-first-name">First name</label>
            <input
              id="identity-first-name"
              type="text"
              class="text-input"
              bind:value={identityFirstName}
              placeholder="First name"
              disabled={isLoading}
            />
          </div>
          <div class="field">
            <label for="identity-last-name">Last name</label>
            <input
              id="identity-last-name"
              type="text"
              class="text-input"
              bind:value={identityLastName}
              placeholder="Last name"
              disabled={isLoading}
            />
          </div>
          <div class="field">
            <label for="identity-email">Email</label>
            <input
              id="identity-email"
              type="email"
              class="text-input"
              bind:value={identityEmail}
              placeholder="you@example.com"
              disabled={isLoading}
            />
          </div>
          <div class="step-actions">
            <button class="primary-button" onclick={handleSaveIdentity} disabled={isLoading}>
              {isLoading ? 'Saving…' : 'Save'}
            </button>
            <button class="skip-button" onclick={skipIdentity} disabled={isLoading}>Skip</button>
          </div>
        {/if}
      {/if}

      <!-- ── PATH step ──────────────────────────────────────────────────── -->
      {#if currentStep === 'path'}
        <div class="onboarding-header">
          <h2>Add NodeSpace to your terminal?</h2>
          <p>
            Adds <code>~/.nodespace/bin</code> to your <code>PATH</code> so you can run
            <code>nodespace</code> from any terminal.
          </p>
        </div>

        {#if pathWasAlreadyConfigured}
          <div class="info-banner">
            Already configured — your shell profile already includes the NodeSpace path.
          </div>
          <div class="step-actions">
            <button class="primary-button" onclick={nextStep}>Next</button>
          </div>
        {:else if stepSuccess}
          <div class="success-banner">
            Added to <code>~/.zshrc</code> and/or <code>~/.bash_profile</code>. Open a new terminal
            to apply.
          </div>
          <div class="step-actions">
            <button class="primary-button" onclick={nextStep}>Next</button>
          </div>
        {:else}
          {#if stepError}
            <div class="error-banner">{stepError}</div>
          {/if}
          <div class="step-actions">
            <button class="primary-button" onclick={handleConfigurePath} disabled={isLoading}>
              {isLoading ? 'Configuring…' : 'Add to PATH'}
            </button>
            <button class="skip-button" onclick={skipCurrentStep} disabled={isLoading}>Skip</button>
          </div>
        {/if}
      {/if}

      <!-- ── Skill step ─────────────────────────────────────────────────── -->
      {#if currentStep === 'skill'}
        <div class="onboarding-header">
          <h2>Add NodeSpace to {detectedAgentsLabel}?</h2>
          <p>
            Installs a <code>SKILL.md</code> file into each agent's skills directory so
            {detectedAgentsLabel} know{detectedAgents.length === 1 ? 's' : ''} how to interact with
            your knowledge graph. This copy updates each time NodeSpace itself updates.
            {#if claudeCodeTargeted}
              If you already have the skill in Claude Code via
              <code>/plugin install nodespace@...</code>, that copy stays in charge — it tracks its
              own marketplace updates, and this step won't overwrite it.
            {/if}
          </p>
        </div>

        {#if stepSuccess}
          <div class="success-banner">
            {#if skillResult && skillResult.agentsInstalled.length > 0}
              Skill installed into: {formatAgentList(skillResult.agentsInstalled)}. Picked up
              automatically on each agent's next session.
            {:else if skillResult && skillResult.agentsSkipped.length > 0}
              Nothing new to install — see below.
            {:else}
              Agent integration is set up.
            {/if}
          </div>
          {#if skillResult && skillResult.agentsSkipped.length > 0}
            <div class="info-banner">
              {#each skillResult.agentsSkipped as skipped (skipped.agent)}
                {displayAgentName(skipped.agent)}: {skipped.reason}<br />
              {/each}
            </div>
          {/if}
          <div class="step-actions">
            <button class="primary-button" onclick={nextStep}>Next</button>
          </div>
        {:else}
          {#if stepError}
            <div class="error-banner">{stepError}</div>
          {/if}
          <div class="step-actions">
            <button class="primary-button" onclick={handleConfigureSkill} disabled={isLoading}>
              {isLoading ? 'Installing…' : 'Add Skill'}
            </button>
            <button class="skip-button" onclick={skipCurrentStep} disabled={isLoading}>Skip</button>
          </div>
        {/if}
      {/if}

      <!-- ── Summary step ───────────────────────────────────────────────── -->
      {#if currentStep === 'summary'}
        <div class="onboarding-header">
          <h2>You're all set!</h2>
          <p>Here's a summary of what was configured.</p>
        </div>

        <ul class="summary-list">
          {#if showIdentity}
            <li class:configured={identityDone} class:skipped={!identityDone}>
              <span class="summary-icon">
                {#if identityDone}
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                {:else}
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                {/if}
              </span>
              <span>
                Your identity
                {#if !identityDone}<span class="summary-note">(skipped)</span>{/if}
              </span>
            </li>
          {/if}

          <li class:configured={pathDone || pathWasAlreadyConfigured} class:skipped={!pathDone && !pathWasAlreadyConfigured}>
            <span class="summary-icon">
              {#if pathDone || pathWasAlreadyConfigured}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                  <polyline points="20 6 9 17 4 12" />
                </svg>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              {/if}
            </span>
            <span>
              Terminal PATH
              {#if pathWasAlreadyConfigured && !pathDone}
                <span class="summary-note">(already configured)</span>
              {:else if !pathDone}
                <span class="summary-note">(skipped)</span>
              {/if}
            </span>
          </li>

          {#if showSkill}
            <li class:configured={skillDone} class:skipped={!skillDone}>
              <span class="summary-icon">
                {#if skillDone}
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                {:else}
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" width="14" height="14">
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                {/if}
              </span>
              <span>
                {#if skillDone && skillResult && skillResult.agentsInstalled.length > 0}
                  NodeSpace skill — {formatAgentList(skillResult.agentsInstalled)}
                {:else}
                  NodeSpace skill
                {/if}
                {#if !skillDone}<span class="summary-note">(skipped)</span>{/if}
              </span>
            </li>
          {/if}
        </ul>

        <p class="settings-hint">
          You can revisit these integrations at any time in
          <strong>Settings &rarr; Integrations</strong>.
        </p>

        <div class="step-actions">
          <button class="primary-button" onclick={finishWizard}>Open NodeSpace</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .onboarding-backdrop {
    position: fixed;
    inset: 0;
    background: hsl(0 0% 0% / 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    padding: 1rem;
  }

  .onboarding-dialog {
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.75rem;
    padding: 2rem;
    max-width: 30rem;
    width: 100%;
    position: relative;
    max-height: 85vh;
    overflow-y: auto;
    box-shadow: 0 12px 40px hsl(0 0% 0% / 0.15);
  }

  .close-button {
    position: absolute;
    top: 0.75rem;
    right: 0.75rem;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    padding: 0.25rem;
    border-radius: 0.25rem;
    display: flex;
    align-items: center;
  }

  .close-button:hover {
    color: hsl(var(--foreground));
  }

  /* Step dots */
  .step-indicator {
    display: flex;
    gap: 0.375rem;
    margin-bottom: 1.5rem;
  }

  .step-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: hsl(var(--muted-foreground) / 0.3);
    transition: background 0.15s;
  }

  .step-dot.active {
    background: hsl(var(--primary));
    width: 18px;
    border-radius: 3px;
  }

  .step-dot.done {
    background: hsl(var(--primary) / 0.5);
  }

  /* Header */
  .onboarding-header {
    margin-bottom: 1.5rem;
  }

  .onboarding-header h2 {
    font-size: 1.25rem;
    font-weight: 600;
    margin: 0 0 0.375rem;
    color: hsl(var(--foreground));
  }

  .onboarding-header p {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    margin: 0;
    line-height: 1.5;
  }

  .onboarding-header code {
    font-size: 0.8125rem;
    background: hsl(var(--muted));
    padding: 0.1em 0.3em;
    border-radius: 3px;
    color: hsl(var(--foreground));
  }

  /* Banners */
  .success-banner {
    font-size: 0.875rem;
    color: hsl(142 76% 30%);
    background: hsl(142 76% 36% / 0.1);
    border: 1px solid hsl(142 76% 36% / 0.25);
    border-radius: 0.375rem;
    padding: 0.625rem 0.875rem;
    margin-bottom: 1.25rem;
    line-height: 1.5;
  }

  .success-banner code {
    font-size: 0.8125rem;
    background: hsl(142 76% 36% / 0.12);
    padding: 0.1em 0.3em;
    border-radius: 3px;
  }

  .info-banner {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted) / 0.5);
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    padding: 0.625rem 0.875rem;
    margin-bottom: 1.25rem;
    line-height: 1.5;
  }

  .error-banner {
    font-size: 0.875rem;
    color: hsl(var(--destructive-foreground));
    background: hsl(var(--destructive) / 0.1);
    border: 1px solid hsl(var(--destructive) / 0.3);
    border-radius: 0.375rem;
    padding: 0.625rem 0.875rem;
    margin-bottom: 1.25rem;
    line-height: 1.5;
  }

  /* Actions */
  /* Identity step fields */
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin-bottom: 0.875rem;
  }

  .field label {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
  }

  .text-input {
    font-size: 0.875rem;
    padding: 0.5rem 0.625rem;
    border-radius: 0.375rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .text-input:disabled {
    opacity: 0.6;
  }

  .step-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .primary-button {
    padding: 0.5rem 1.25rem;
    border-radius: 0.375rem;
    border: none;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .primary-button:hover:not(:disabled) {
    opacity: 0.9;
  }

  .primary-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .skip-button {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    padding: 0.5rem 0.25rem;
  }

  .skip-button:hover:not(:disabled) {
    color: hsl(var(--foreground));
  }

  .skip-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Summary */
  .summary-list {
    list-style: none;
    margin: 0 0 1.25rem;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
  }

  .summary-list li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .summary-list li.configured {
    color: hsl(var(--foreground));
  }

  .summary-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }

  .summary-list li.configured .summary-icon {
    color: hsl(142 76% 36%);
  }

  .summary-list li.skipped .summary-icon {
    color: hsl(var(--muted-foreground) / 0.5);
  }

  .summary-note {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin-left: 0.25rem;
  }

  .settings-hint {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    margin: 0 0 1.5rem;
    line-height: 1.5;
  }
</style>
