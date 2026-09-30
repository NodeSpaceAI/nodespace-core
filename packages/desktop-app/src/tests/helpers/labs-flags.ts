/**
 * Set every Labs flag at once, so a test that needs "all on" or "all off" does
 * not name the flags one by one (and does not miss one added later).
 */
import { labsFlags } from '$lib/stores/labs-flags.svelte';

export function setAllLabsFlags(on: boolean): void {
  const flags = labsFlags.flags;
  for (const name of Object.keys(flags) as (keyof typeof flags)[]) flags[name] = on;
}
