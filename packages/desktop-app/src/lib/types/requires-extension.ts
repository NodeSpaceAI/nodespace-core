/**
 * The refusal of a database that requires an extension this build does not
 * support (ADR-083 §2).
 *
 * A database's settings node lists, in `required_extensions`, the extensions a
 * reader needs in order to read it correctly. The daemon refuses to open a
 * database that lists one this build does not support, and every command
 * routed to that database rejects with a REQUIRES_EXTENSION CommandError.
 *
 * The error carries everything the app shows: the refusal message, and the
 * label and URL of its download link. All three are rendered by the app
 * binary's display-name module, so the CLI, the tray and the app say the same
 * thing, and the frontend renders them verbatim instead of building its own
 * text (ADR-081 §2b, ADR-084 §1).
 */

import type { CommandError } from './errors';

/**
 * The payload of a REQUIRES_EXTENSION CommandError, carried as
 * `requiresExtension`. Mirrors the app library's `RequiresExtensionPayload`
 * (camelCase).
 */
export interface RequiresExtensionPayload {
  /** The extension ids the database requires that this build does not support. */
  unsupportedExtensions: string[];

  /** The refusal message, shown verbatim. */
  message: string;

  /** The label of the download link. */
  downloadLabel: string;

  /** Where the download link points: an https URL. */
  downloadUrl: string;
}

/**
 * A CommandError produced when a command is routed to a database the daemon
 * refuses to open because it requires an extension this build does not
 * support.
 */
export interface RequiresExtensionCommandError extends CommandError {
  code: 'REQUIRES_EXTENSION';
  requiresExtension: RequiresExtensionPayload;
}

/**
 * Type guard: true when the thrown value is a REQUIRES_EXTENSION CommandError
 * carrying a well-formed refusal payload.
 *
 * Matches the Tauri shape: { message, code: "REQUIRES_EXTENSION",
 * requiresExtension: { unsupportedExtensions, message, downloadLabel,
 * downloadUrl } }. The download URL must be https: the refusal carries a
 * download link and nothing else, so a payload pointing anywhere else is
 * malformed.
 */
export function isRequiresExtension(error: unknown): error is RequiresExtensionCommandError {
  if (typeof error !== 'object' || error === null) return false;

  const err = error as Record<string, unknown>;
  if (err.code !== 'REQUIRES_EXTENSION' || typeof err.message !== 'string') return false;
  if (typeof err.requiresExtension !== 'object' || err.requiresExtension === null) return false;

  const payload = err.requiresExtension as Record<string, unknown>;
  return (
    Array.isArray(payload.unsupportedExtensions) &&
    payload.unsupportedExtensions.every((id) => typeof id === 'string') &&
    typeof payload.message === 'string' &&
    typeof payload.downloadLabel === 'string' &&
    typeof payload.downloadUrl === 'string' &&
    payload.downloadUrl.startsWith('https://')
  );
}
