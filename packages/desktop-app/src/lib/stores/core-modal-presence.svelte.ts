/**
 * Whether one of core's own modals (the first-launch wizard, the delete
 * confirmation, ...) is on screen. Extensions read it through
 * `shell.modalOpen` in `@nodespace/extension-api` so an `app-shell-modal`
 * contribution can wait for core's modals instead of stacking over them.
 *
 * Each modal holds one registration, released exactly once; the signal is on
 * while any registration is held. Registering goes through the `coreModal`
 * action, which releases on destroy, so a modal that unmounts (or whose
 * component throws and is torn down) cannot leave the signal on.
 */

const held = new Set<symbol>();

let count = $state(0);

/** Registers an open core modal. The returned function releases it; extra calls do nothing. */
export function registerCoreModal(): () => void {
  const token = Symbol('core-modal');
  held.add(token);
  count = held.size;
  return () => {
    if (held.delete(token)) count = held.size;
  };
}

/** A reactive read: true while any core modal holds a registration. */
export function isCoreModalOpen(): boolean {
  return count > 0;
}
