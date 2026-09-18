/**
 * Methodology recipes — listing what is on offer, and installing one.
 *
 * Both the onboarding wizard and the Settings action call through here rather
 * than invoking Tauri directly. Onboarding is expected to be reworked, so the
 * execution path deliberately does not live inside it: a Settings install and
 * a wizard install are the same call, and the wizard can be rewritten around
 * this without the install logic moving with it.
 *
 * The install itself (step order, id-collision re-keying, per-step reporting)
 * is in `packages/core`, beside the recipe it executes. This module only moves
 * a choice one way and a report back.
 */

import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('MethodologyService');

/** A recipe on offer, for the picker. */
export interface Methodology {
  id: string;
  name: string;
  description: string;
}

/**
 * What one install step did.
 *
 * `suffixed` is the interesting one: the requested id was taken, so the step
 * landed under `created` instead. It is reported rather than silently
 * resolved — see `summarizeReport`.
 */
export type StepOutcome =
  | { kind: 'created'; id: string }
  | { kind: 'suffixed'; requested: string; created: string }
  | { kind: 'skipped' }
  | { kind: 'failed'; message: string };

export interface StepReport {
  label: string;
  outcome: StepOutcome;
}

export interface InstallReport {
  recipeId: string;
  steps: StepReport[];
  /** Whether every step landed. False means the install stopped partway. */
  success: boolean;
}

/**
 * The recipes this build ships.
 *
 * Normalizes a missing or malformed response to an empty list rather than
 * letting it through. Callers branch on `length` to decide whether to offer
 * the choice at all, and "no recipes" is the honest answer when the backend
 * did not give us any — an `undefined` here would surface as a crash in the
 * wizard rather than a step that quietly does not appear.
 */
export async function listMethodologies(): Promise<Methodology[]> {
  const result = await invoke<Methodology[]>('list_methodologies');
  return Array.isArray(result) ? result : [];
}

/**
 * Install a recipe by id.
 *
 * Resolves with a report even when a step failed — `success` says whether
 * everything landed. Only a transport failure rejects.
 */
export async function installMethodology(methodologyId: string): Promise<InstallReport> {
  const report = await invoke<InstallReport>('install_methodology', { methodologyId });
  log.info('Methodology install finished', {
    methodologyId,
    success: report.success,
    steps: report.steps.length,
    renamed: renamedIds(report).length
  });
  return report;
}

/** Ids that were re-keyed because the recipe's preferred id was taken. */
export function renamedIds(report: InstallReport): Array<{ requested: string; created: string }> {
  return report.steps
    .map((s) => s.outcome)
    .filter((o): o is Extract<StepOutcome, { kind: 'suffixed' }> => o.kind === 'suffixed')
    .map(({ requested, created }) => ({ requested, created }));
}

/** The first failure's message, if the install stopped early. */
export function failureMessage(report: InstallReport): string | null {
  for (const step of report.steps) {
    if (step.outcome.kind === 'failed') {
      return `${step.label}: ${step.outcome.message}`;
    }
  }
  return null;
}

/**
 * A one-or-two sentence summary of what landed, for the banner.
 *
 * Re-keying is stated plainly rather than buried: a user whose existing
 * `cycle` was left alone needs to know their new one is called something else,
 * or they will go looking for fields on the wrong type.
 */
export function summarizeReport(report: InstallReport): string {
  const failure = failureMessage(report);
  if (failure) {
    const landed = report.steps.filter(
      (s) => s.outcome.kind === 'created' || s.outcome.kind === 'suffixed'
    ).length;
    return landed === 0
      ? `Nothing was installed — ${failure}`
      : `Stopped after ${landed} of ${report.steps.length} steps — ${failure}`;
  }

  const renamed = renamedIds(report);
  const base = `Installed ${report.steps.length} items.`;
  if (renamed.length === 0) {
    return base;
  }

  const list = renamed.map((r) => `${r.requested} → ${r.created}`).join(', ');
  return `${base} Some names were already taken, so these were added under new ones: ${list}. Your existing types were left unchanged.`;
}
