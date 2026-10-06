// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CaptureContent } from './capture-content';
import type { ProviderConfig } from './provider-config';

/**
 * Partial update for the settings node's core fields, received from the
 * frontend.
 *
 * Every field is tri-state: absent leaves it unchanged, `null` clears it (it
 * then reads as its default), and a value replaces it whole. A provider list
 * is replaced as a list, never merged.
 */
export type DatabaseSettingsNodeUpdate = {
  requiredExtensions?: Array<string> | null;
  captureEnabled?: boolean | null;
  captureContent?: CaptureContent | null;
  providers?: Array<ProviderConfig> | null;
};
