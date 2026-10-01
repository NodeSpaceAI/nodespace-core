/**
 * Daemon Status Service
 *
 * Single shared source of daemon readiness. Fans the signal out to:
 *   - a `connecting` grace-period flag for `AppShell`'s banner (starts true,
 *     flips false once a status is known, or after a short local timer —
 *     whichever comes first)
 *   - an `unreachable` flag for the existing "background service is not
 *     running" banner/retry button
 *   - an `incompatibleDatabase` flag for the case the daemon is down on
 *     purpose: it refused a database another version of NodeSpace created,
 *     which only moving that database aside can fix
 *   - a set of "on reconnect" callbacks that daemon-dependent stores
 *     (schemas, collections, children-tree) register to retry their load
 *     once the daemon transitions to healthy
 *
 * The reconnect callbacks are the shared hook every daemon-dependent store
 * can use. They complement the one-off `daemon:data-plane-ready` event
 * (`DATA_PLANE_READY_EVENT`), which app-shell.svelte uses to reload once the
 * daemon first answers a real gRPC round trip at startup.
 *
 * Readiness is pulled AND pushed, not push-only. A push-only design has its
 * own startup race: the backend emits `daemon-status` from its own setup
 * task, on its own clock, and if that emit fires before the webview has
 * registered its listener, the signal is lost with no way to recover it.
 * Pulling the current status on subscribe (via the `check_daemon_status`
 * command) closes that window, and gives the manual "Retry" affordance a
 * real path through this shared contract instead of bypassing it.
 */

import { writable } from 'svelte/store';
import { isTauri } from '@tauri-apps/api/core';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('DaemonStatus');

/**
 * Emitted at most once per app start, with a `null` payload: after the
 * backend's post-startup socket check, and only when the daemon then answers a
 * real gRPC round trip. A reachable socket alone only proves something accepts
 * connections. Mirrors the Rust `DATA_PLANE_READY_EVENT` constant in the
 * desktop app library.
 */
export const DATA_PLANE_READY_EVENT = 'daemon:data-plane-ready';

/** How long to show a "connecting" banner before any status is known. */
const CONNECTING_GRACE_PERIOD_MS = 1500;

/**
 * How often to re-probe daemon health once the listener is running.
 *
 * The backend pushes `daemon-status` only from its one-shot startup task, so a
 * daemon that dies and is relaunched mid-session emits nothing — every
 * reconnect consumer would stay wedged until an app restart. A low-frequency
 * pull observes the `healthy → not_running → healthy` cycle so `applyStatus`
 * re-fires `onDaemonReconnect`. Kept slow because the only work on a steady
 * healthy poll is a cheap socket probe; the reconnect callbacks fire only on
 * the not-healthy → healthy edge, never on repeated healthy polls.
 */
const STEADY_STATE_POLL_MS = 15000;

export interface DaemonStatusState {
  /** True until the first status is known or the grace period elapses. */
  connecting: boolean;
  /** True once a `not_running` status has been observed. */
  unreachable: boolean;
  /**
   * True once an `incompatible_database` status has been observed: the daemon
   * refused its database because a different version of NodeSpace created it,
   * and stopped rather than retry. Retrying cannot help; see
   * {@link resetIncompatibleDatabase}.
   */
  incompatibleDatabase: boolean;
}

/** The daemon's record of the database it refused. */
export interface IncompatibleDatabase {
  /** Absolute path of the refused database file. */
  databasePath: string;
  /** Which tables differ and how — for support, not the headline. */
  detail: string;
  /** When the daemon refused it, RFC 3339. */
  detectedAt: string;
}

export interface ResetIncompatibleDatabaseResult {
  /** Where the refused database was moved to; `null` if it was already gone. */
  backupPath: string | null;
  /** Daemon status after restarting on a fresh database. */
  status: string;
}

/**
 * A source of daemon readiness, decoupled from any particular transport.
 * Production binds a Tauri source (pull via `check_daemon_status` + push via
 * the `daemon-status` event); tests can bind any other source — e.g. one
 * backed by a real headless daemon in an integration harness — without the
 * shared core caring how the status arrived.
 */
export interface DaemonStatusSource {
  /**
   * Pull the current status once. Resolves to `"healthy"`, `"starting"`,
   * `"not_running"`, or `"incompatible_database"`.
   */
  getCurrent(): Promise<string>;
  /** Subscribe to pushed status changes. Returns an unsubscribe function. */
  subscribe(callback: (status: string) => void): () => void;
  /**
   * Optional: probe the underlying transport for a wedged-but-healthy
   * connection and recover it in place. Resolves `true` if a wedge was found
   * and the connection was rebuilt (the caller should then re-fire reconnect
   * listeners), `false` if the connection was already live. Sources whose
   * transport cannot wedge (tests, browser dev) may omit this.
   */
  probeChannel?(): Promise<boolean>;
}

const _status = writable<DaemonStatusState>({
  connecting: true,
  unreachable: false,
  incompatibleDatabase: false
});

const reconnectListeners = new Set<() => void>();

let started = false;
let lastHealthy = false;
let activeSource: DaemonStatusSource | null = null;
let steadyStatePoll: ReturnType<typeof setInterval> | null = null;
/** Guards against overlapping polls if a `getCurrent` probe runs long. */
let pollInFlight = false;
/**
 * True while {@link resetIncompatibleDatabase} is moving the database aside and
 * restarting the daemon. Statuses observed meanwhile (a poll landing while the
 * daemon is still loading reports `not_running`) describe a restart in
 * progress, not a new failure, so they are held back and the reset's own final
 * status is applied instead. Without this the incompatible-database banner —
 * and its in-progress state — would be swapped for the generic not-running
 * banner mid-reset.
 */
let resetInFlight = false;

/**
 * Register a callback to run whenever the daemon transitions to healthy
 * (including the very first healthy status of the session). Returns an
 * unsubscribe function.
 */
export function onDaemonReconnect(callback: () => void): () => void {
  reconnectListeners.add(callback);
  return () => reconnectListeners.delete(callback);
}

export const daemonStatus = {
  subscribe: _status.subscribe
};

/** Run every registered reconnect listener once, isolating throws. */
function fireReconnectListeners(): void {
  for (const cb of reconnectListeners) {
    try {
      cb();
    } catch (err) {
      log.error('Reconnect listener threw', err);
    }
  }
}

/** Apply a status string to shared state and fan out reconnect callbacks. Transport-agnostic. */
function applyStatus(payload: string): void {
  if (resetInFlight) return;
  const healthy = payload === 'healthy';
  _status.set({
    connecting: false,
    unreachable: payload === 'not_running',
    incompatibleDatabase: payload === 'incompatible_database'
  });

  if (healthy && !lastHealthy) {
    lastHealthy = true;
    fireReconnectListeners();
  } else if (!healthy) {
    lastHealthy = false;
  }
}

/** Tauri-backed source: pulls via the `check_daemon_status` command, pushes via the `daemon-status` event. */
function tauriSource(): DaemonStatusSource {
  return {
    getCurrent: () => invoke<string>('check_daemon_status'),
    probeChannel: () => invoke<boolean>('probe_and_recover_channel'),
    subscribe(callback) {
      let unlistened = false;
      let unlisten: (() => void) | null = null;
      listen<string>('daemon-status', (event) => callback(event.payload))
        .then((fn) => {
          if (unlistened) {
            fn();
          } else {
            unlisten = fn;
          }
        })
        .catch((err) => {
          log.warn('Failed to register daemon-status listener', err);
        });
      return () => {
        unlistened = true;
        unlisten?.();
      };
    }
  };
}

/**
 * Start the shared daemon-status service against a given source (defaults to
 * the Tauri-backed source). Safe to call multiple times — only the first
 * call actually starts listening. Outside Tauri (browser dev mode) the
 * default source is a no-op, same as before.
 */
export function startDaemonStatusListener(source?: DaemonStatusSource): void {
  if (started) return;
  if (!source && !isTauri()) return;
  started = true;
  activeSource = source ?? tauriSource();

  const graceTimer = setTimeout(() => {
    _status.update((s) => ({ ...s, connecting: false }));
  }, CONNECTING_GRACE_PERIOD_MS);

  activeSource.subscribe((payload) => {
    clearTimeout(graceTimer);
    applyStatus(payload);
  });

  // Pull the current status immediately so a status emitted before this
  // subscribe call (or between app launch and listener registration) is
  // not lost — the push path above still applies later transitions.
  activeSource
    .getCurrent()
    .then((payload) => {
      clearTimeout(graceTimer);
      applyStatus(payload);
    })
    .catch((err) => {
      log.warn('Failed to pull initial daemon status', err);
    });

  // Steady-state poll: keep re-probing at a low frequency so a mid-session
  // daemon restart (healthy → not_running → healthy) is observed here. The
  // backend emits `daemon-status` only from its one-shot startup task, so a
  // relaunched daemon pushes nothing; without this poll every reconnect
  // consumer (schemas, collections, children-tree, pane hydration) stays
  // wedged until an app restart.
  steadyStatePoll = setInterval(() => {
    if (pollInFlight || !activeSource) return;
    pollInFlight = true;
    const source = activeSource;
    source
      .getCurrent()
      .then(async (payload) => {
        applyStatus(payload);
        // A healthy socket does NOT prove the long-lived gRPC channel is
        // usable: it can wedge (reads/writes and the WatchNodes stream hang
        // indefinitely) while the daemon stays up and the socket probe keeps
        // reporting "healthy". Probe the real channel and, if it had to be
        // rebuilt, re-fire the reconnect listeners so panes re-fetch on the
        // fresh channel — applyStatus won't, since the daemon never left
        // "healthy" for it to observe a transition.
        if (payload === 'healthy' && source.probeChannel) {
          const recovered = await source.probeChannel();
          if (recovered) {
            log.warn('gRPC channel was wedged — rebuilt it; re-firing reconnect listeners');
            fireReconnectListeners();
          }
        }
      })
      .catch((err) => log.warn('Steady-state daemon status poll failed', err))
      .finally(() => {
        pollInFlight = false;
      });
  }, STEADY_STATE_POLL_MS);
  // Never let the poll on its own keep a runtime (or a test worker) alive.
  (steadyStatePoll as unknown as { unref?: () => void })?.unref?.();
}

/**
 * Stop the steady-state poll and reset the listener so it can be started
 * again. Production never calls this (the service lives for the whole session);
 * it exists so tests can tear the singleton down between cases.
 */
export function stopDaemonStatusListener(): void {
  if (steadyStatePoll) {
    clearInterval(steadyStatePoll);
    steadyStatePoll = null;
  }
  pollInFlight = false;
  resetInFlight = false;
  started = false;
  lastHealthy = false;
  activeSource = null;
}

/**
 * Re-pull the current daemon status through the shared contract. Used by the
 * manual "Retry" affordance so a recovered daemon is detected the same way
 * an automatic recovery would be — including firing `onDaemonReconnect`
 * listeners — instead of only clearing the banner locally.
 */
export async function refreshDaemonStatus(): Promise<void> {
  if (!activeSource) return;
  const payload = await activeSource.getCurrent();
  applyStatus(payload);
}

/**
 * The database the daemon refused as incompatible, or `null` when there is no
 * standing refusal (or outside Tauri).
 */
export async function getIncompatibleDatabase(): Promise<IncompatibleDatabase | null> {
  if (!isTauri()) return null;
  return (await invoke<IncompatibleDatabase | null>('get_incompatible_database')) ?? null;
}

/**
 * Move the refused database aside (renamed beside itself with a timestamp —
 * nothing is deleted) and restart the daemon on a fresh one. Applies the
 * resulting status through the shared contract, so a healthy restart fires
 * `onDaemonReconnect` listeners exactly as any other recovery would.
 *
 * Rejects with the backend's message when nothing was moved.
 */
export async function resetIncompatibleDatabase(): Promise<ResetIncompatibleDatabaseResult> {
  resetInFlight = true;
  let result: ResetIncompatibleDatabaseResult;
  try {
    result = await invoke<ResetIncompatibleDatabaseResult>('reset_incompatible_database');
  } finally {
    resetInFlight = false;
  }
  applyStatus(result.status);
  return result;
}
