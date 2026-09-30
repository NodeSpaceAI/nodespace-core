/**
 * Host API for build-time extensions (ADR-082 §2.6).
 *
 * An extension imports from `@nodespace/extension-api`, which the build aliases
 * to this directory. Core modules never import it: it is the surface core
 * offers extensions, not something core consumes.
 *
 * This entry carries only what registration needs. The host services, the `/ui`
 * and `/testing` entries and the compatibility policy are added before any
 * extension relies on them.
 */

export { EXTENSION_API_VERSION } from '$lib/plugins/ui-extensions';
export type {
  ChromeContribution,
  ChromeSlot,
  Contribution,
  NodespaceExtension,
  SettingsSectionContribution,
  SettingsSlot,
  SettingsSlotContribution,
  SettingsSlotContributionFor,
  ViewerTabContribution
} from '$lib/plugins/ui-extensions';
