/**
 * Built-in Pro UI extension
 * =================================
 *
 * The single {@link UiExtensionDefinition} for the Pro-sync surface. It maps each
 * variant of the two-signal state machine (see `ui-extensions.svelte.ts`) to the
 * component that renders it:
 *
 *   | variant     | modal              | collection tab       |
 *   |-------------|--------------------|----------------------|
 *   | teaser      | —                  | — (none)             |
 *   | sign-in     | —                  | collaboration-locked |
 *   | consent     | first-pro-consent  | collaboration-locked |
 *   | relogin     | pro-relogin-slot   | collaboration-tab    |
 *   | connected   | pro-relogin-slot   | collaboration-tab    |
 *
 * There is no `app-shell-overlay` chrome anymore: the top-right sync-status
 * pill (`pro-sync-pill`), the community-build upsell teaser (`pro-teaser-pill`),
 * and the turn-on-sync nudge (`enable-sync-pill`) were all removed. Account
 * access now lives in Settings → Account (signed-in email, sign out, opening
 * Invitations), sign-in lives in Settings → Database → "Add synced database…",
 * and reopening the publish-consent modal after a decline lives in
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
 * Registration is a module-load side effect so the contributions are present
 * before the first render (the registry is static config, not reactive state).
 */

import { uiExtensionRegistry, type UiExtensionDefinition } from './ui-extensions';

const COLLABORATION_TAB = { appliesTo: 'collection', id: 'collaboration', label: 'Collaboration' };

export const proSyncUiExtension: UiExtensionDefinition = {
  id: 'pro-sync',
  name: 'NodeSpace Pro Sync',
  version: '1.0.0',
  chrome: [
    // First-Pro data-sharing consent modal — shown once the user has signed in but
    // not yet opted into sync. The gate that keeps local data from reaching the
    // public workspace without an explicit, irreversible choice.
    {
      slot: 'app-shell-modal',
      variant: 'consent',
      lazyLoad: () => import('$lib/components/first-pro-consent-slot.svelte')
    },
    // Re-login modal — only meaningful once sync is enabled for the database; the
    // wrapper itself only shows the modal on an AUTH_REQUIRED transition.
    {
      slot: 'app-shell-modal',
      variant: 'relogin',
      lazyLoad: () => import('$lib/components/pro-relogin-slot.svelte')
    },
    {
      slot: 'app-shell-modal',
      variant: 'connected',
      lazyLoad: () => import('$lib/components/pro-relogin-slot.svelte')
    }
  ],
  viewerExtensions: [
    // Collaboration tab. Locked placeholder while sync is disabled for this
    // database (Pro daemon, sync_enabled: false — whether or not signed in); the
    // live view once enabled.
    {
      tab: COLLABORATION_TAB,
      variant: 'sign-in',
      lazyLoad: () => import('$lib/components/collaboration/collaboration-locked.svelte')
    },
    {
      tab: COLLABORATION_TAB,
      variant: 'consent',
      lazyLoad: () => import('$lib/components/collaboration/collaboration-locked.svelte')
    },
    {
      tab: COLLABORATION_TAB,
      variant: 'relogin',
      lazyLoad: () => import('$lib/components/collaboration/collaboration-tab.svelte')
    },
    {
      tab: COLLABORATION_TAB,
      variant: 'connected',
      lazyLoad: () => import('$lib/components/collaboration/collaboration-tab.svelte')
    }
  ]
};

uiExtensionRegistry.register(proSyncUiExtension);
