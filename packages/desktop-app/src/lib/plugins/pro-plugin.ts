/**
 * Built-in Pro UI extension
 * =================================
 *
 * The single {@link NodespaceExtension} for the Pro-sync surface. Each
 * contribution's `when()` selects the variants of the two-signal state machine
 * (see `pro-sync-variant.svelte.ts`) it renders for:
 *
 *   | variant     | modal              | collection tab       |
 *   |-------------|--------------------|----------------------|
 *   | teaser      | —                  | — (none)             |
 *   | sign-in     | —                  | collaboration-locked |
 *   | consent     | first-pro-consent  | collaboration-locked |
 *   | relogin     | pro-relogin-slot   | collaboration-tab    |
 *   | connected   | pro-relogin-slot   | collaboration-tab    |
 *
 * The four variants that show a Collaboration tab are ONE contribution
 * (`collaboration`): the host keys a tab by its contribution key, so keeping the
 * key constant keeps the selected tab across a variant change (for example
 * "Turn on sync", then consent accepted, then connected). The locked-vs-live
 * choice lives inside `collaboration-tab.svelte`. The modal has one contribution
 * per variant so the re-login slot still remounts when the variant moves between
 * `relogin` and `connected`.
 *
 * There is no `app-shell-overlay` chrome anymore: the top-right sync-status
 * pill, the community-build upsell teaser, and the turn-on-sync nudge were all
 * removed. Account access now lives in Settings → Account (signed-in email, sign
 * out, opening Invitations), sign-in lives in Settings → Database → "Add synced
 * database…", and reopening the publish-consent modal after a decline lives in
 * `collaboration-locked.svelte`'s "Turn on sync" button (shown for the
 * `consent` variant) — so no top-right pill is needed for any of the three.
 *
 * The flow is still sign-in-first: `sign-in` starts OAuth from Settings →
 * Database, once authenticated the database becomes `consent`, where the
 * first-Pro modal asks for the public-workspace publish decision (merge flips
 * `sync_enabled`); after that it is `connected` (or `relogin` if the session
 * later lapses).
 *
 * Every component is referenced only through `() => import(...)`, so nothing Pro
 * is imported eagerly — the shared shell stays free of Pro component imports.
 * Existing Pro components (`pro-relogin-modal`, `collaboration-view`) are
 * mounted unchanged; the small wrappers (`pro-relogin-slot`,
 * `collaboration-tab`) exist only to move their mounting behind the registry.
 *
 * The default export is the extension list a build entry hands to
 * `registerExtensions()`. Until builds inject extensions that way, this module
 * also registers itself as a load-time side effect, so the contributions are
 * present before the first render (the registry is static config, not reactive
 * state).
 */

import { uiExtensionRegistry, type NodespaceExtension } from './ui-extensions';
import { resolveProSyncVariant, type ProSyncVariant } from './pro-sync-variant.svelte';

/** A `when()` that holds while the resolved variant is one of `variants`. */
const whenVariant =
  (...variants: ProSyncVariant[]) =>
  () =>
    variants.includes(resolveProSyncVariant());

export const proSyncExtension: NodespaceExtension = {
  id: 'pro-sync',
  apiVersion: 1,
  chrome: [
    // First-Pro data-sharing consent modal — shown once the user has signed in but
    // not yet opted into sync. The gate that keeps local data from reaching the
    // public workspace without an explicit, irreversible choice.
    {
      id: 'consent-modal',
      slot: 'app-shell-modal',
      when: whenVariant('consent'),
      load: () => import('$lib/components/first-pro-consent-slot.svelte')
    },
    // Re-login modal — only meaningful once sync is enabled for the database; the
    // wrapper itself only shows the modal on an AUTH_REQUIRED transition.
    {
      id: 'relogin-modal-relogin',
      slot: 'app-shell-modal',
      when: whenVariant('relogin'),
      load: () => import('$lib/components/pro-relogin-slot.svelte')
    },
    {
      id: 'relogin-modal-connected',
      slot: 'app-shell-modal',
      when: whenVariant('connected'),
      load: () => import('$lib/components/pro-relogin-slot.svelte')
    }
  ],
  viewerTabs: [
    // Collaboration tab. Locked placeholder while sync is disabled for this
    // database (Pro daemon, sync_enabled: false — whether or not signed in); the
    // live view once enabled. The wrapper picks which.
    {
      id: 'collaboration',
      nodeType: 'collection',
      label: 'Collaboration',
      when: whenVariant('sign-in', 'consent', 'relogin', 'connected'),
      load: () => import('$lib/components/collaboration/collaboration-tab.svelte')
    }
  ]
};

export default [proSyncExtension];

// Transitional self-registration; removed once builds register extensions themselves.
uiExtensionRegistry.register(proSyncExtension);
