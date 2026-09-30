/**
 * App-update status store.
 *
 * The Rust side (`update_check.rs`) checks this build's release source at startup and
 * emits `update://available` only when a newer NodeSpace version exists; it also
 * exposes the `check_for_update_command` for an on-demand / post-reload check. The
 * payload names where to download the update (`download_url`), or `null` when the
 * source names none. This store surfaces that as a dismissible, non-blocking banner
 * (see `update-banner.svelte`).
 *
 * The app bundle is not code-signed and ships no auto-updater, so "update" means
 * "open the release download" — the user installs it and their data is untouched
 * (the local store lives in ~/.nodespace, outside the bundle; the daemon also
 * snapshots the DB before any release's migrations run). This store therefore only
 * NOTIFIES; it never mutates anything.
 *
 * Dismissal is per-version (persisted): dismissing 0.3.0 hides the banner for
 * 0.3.0 but it re-appears when 0.4.0 ships.
 */
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '$lib/utils/external-links';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('UpdateStatus');

/** Mirrors the Rust `UPDATE_AVAILABLE_EVENT`. */
export const UPDATE_AVAILABLE_EVENT = 'update://available';
const DISMISSED_KEY = 'ns:update-dismissed-version';

/** Mirrors the Rust `UpdateStatus` payload. */
export interface UpdateStatus {
  current: string;
  latest: string | null;
  update_available: boolean;
  /** Where the source that found the update sends the user; `null` when it names none. */
  download_url: string | null;
}

function readDismissed(): string | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage.getItem(DISMISSED_KEY) : null;
  } catch {
    return null;
  }
}

class UpdateStore {
  current = $state('');
  latest = $state<string | null>(null);
  available = $state(false);
  /** Where to get the update, as named by the source that found it; `null` when none. */
  downloadUrl = $state<string | null>(null);
  /** The version the user last dismissed (persisted); banner stays hidden for it. */
  dismissedVersion = $state<string | null>(null);
  private unlisten: UnlistenFn | null = null;
  private started = false;

  /**
   * Show only when a newer version is available AND the user hasn't dismissed
   * THIS version. A newer `latest` than what was dismissed re-shows the banner.
   */
  get showBanner(): boolean {
    return this.available && this.latest !== null && this.dismissedVersion !== this.latest;
  }

  /** Whether the update source named a download location to open. */
  get canDownload(): boolean {
    return this.downloadUrl !== null;
  }

  private apply(status: UpdateStatus): void {
    this.current = status.current;
    this.latest = status.latest;
    this.available = status.update_available && status.latest !== null;
    this.downloadUrl = status.download_url ?? null;
  }

  /**
   * Subscribe to the startup event and run one on-demand check (so a webview
   * reload after the startup emit still learns of an available update).
   * Idempotent — only the first call wires up.
   */
  async init(): Promise<void> {
    if (this.started) return;
    this.started = true;
    this.dismissedVersion = readDismissed();
    try {
      this.unlisten = await listen<UpdateStatus>(UPDATE_AVAILABLE_EVENT, (e) => this.apply(e.payload));
    } catch (e) {
      log.warn('failed to subscribe to update event', { error: e });
    }
    try {
      this.apply(await invoke<UpdateStatus>('check_for_update_command'));
    } catch (e) {
      log.warn('on-demand update check failed', { error: e });
    }
  }

  /** Persist a per-version dismissal and hide the banner for the current `latest`. */
  dismiss(): void {
    if (!this.latest) return;
    try {
      localStorage?.setItem(DISMISSED_KEY, this.latest);
    } catch (e) {
      log.warn('failed to persist update dismissal', { error: e });
    }
    this.dismissedVersion = this.latest;
  }

  /**
   * Open the download location the update source named (no in-app install — see the
   * module doc). Does nothing when the source named none. The location is data from
   * the source, so one the browser opener rejects is logged rather than thrown.
   */
  async download(): Promise<void> {
    if (this.downloadUrl === null) {
      log.warn('download requested but the update source named no download location');
      return;
    }
    try {
      await openUrl(this.downloadUrl);
    } catch (e) {
      log.warn('failed to open the update download location', { error: e });
    }
  }

  stop(): void {
    this.unlisten?.();
    this.unlisten = null;
    this.started = false;
  }
}

export const updateStatus = new UpdateStore();
