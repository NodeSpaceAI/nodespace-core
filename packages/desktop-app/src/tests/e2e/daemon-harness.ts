/**
 * DaemonTestHarness — spin up a real nodespaced + dev-proxy for e2e tests.
 *
 * Usage:
 *   const h = await DaemonTestHarness.start();
 *   const adapter = h.adapter;   // HttpAdapter pointing at the live stack
 *   await h.stop();
 *
 * Environment:
 *   NODESPACED_BINARY   — override binary path (defaults to prebuilt sidecar)
 *   E2E_DAEMON_TIMEOUT  — ms to wait for daemon readiness (default 10000)
 */

import * as child_process from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
// Import the real HttpAdapter directly (bypassing the `backendAdapter`
// singleton export, which detects the test environment and returns a
// MockAdapter) so this harness drives the same production request-shaping
// code as the browser dev path — not a fourth hand-synced copy of it.
import { HttpAdapter } from '$lib/services/backend-adapter';

// ============================================================================
// Helpers
// ============================================================================

function resolveDesktopAppDir(): string {
  // Walk up from __dirname until we find the src-tauri directory,
  // which marks the desktop-app package root.
  let dir = __dirname;
  for (let i = 0; i < 10; i++) {
    if (fs.existsSync(path.join(dir, 'src-tauri'))) return dir;
    dir = path.dirname(dir);
  }
  // Fallback: this file is at packages/desktop-app/src/tests/e2e/
  return path.resolve(__dirname, '../../..');
}

function resolveDaemonBinary(): string {
  if (process.env.NODESPACED_BINARY) {
    return process.env.NODESPACED_BINARY;
  }
  const arch = process.arch === 'arm64' ? 'aarch64' : 'x86_64';
  const platform = process.platform === 'darwin' ? 'apple-darwin' : 'unknown-linux-gnu';
  const binaryName = `nodespaced-${arch}-${platform}`;
  const desktopApp = resolveDesktopAppDir();
  return path.join(desktopApp, 'src-tauri', 'binaries', binaryName);
}

function resolveDevProxyScript(): string {
  const desktopApp = resolveDesktopAppDir();
  // packages/desktop-app/../dev-tools/src/dev-proxy.ts
  return path.join(desktopApp, '..', 'dev-tools', 'src', 'dev-proxy.ts');
}

async function findFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('Could not determine free port'));
        return;
      }
      const port = address.port;
      server.close(() => resolve(port));
    });
    server.on('error', reject);
  });
}

/** One-shot UDS reachability probe — mirrors the Rust `check_daemon_socket` semantics. */
async function probeSocket(socketPath: string): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    const sock = net.createConnection(socketPath);
    sock.once('connect', () => {
      sock.destroy();
      resolve(true);
    });
    sock.once('error', () => resolve(false));
  });
}

async function waitForSocket(socketPath: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await probeSocket(socketPath)) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`Daemon socket ${socketPath} not ready after ${timeoutMs}ms`);
}

async function waitForHttp(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`HTTP endpoint ${url} not ready after ${timeoutMs}ms`);
}

interface SpawnedProcesses {
  tmpDir: string;
  socketPath: string;
  proxyPort: number;
  daemonProc: child_process.ChildProcess;
  proxyProc: child_process.ChildProcess;
  daemonSpawnError: Error | undefined;
  proxySpawnError: Error | undefined;
}

/**
 * Spawn the daemon and dev-proxy processes without waiting on either's
 * readiness — shared by `start()` (which waits for both) and
 * `startDeferred()` (which only waits for the proxy).
 */
async function spawnDaemonAndProxy(): Promise<SpawnedProcesses> {
  const binary = resolveDaemonBinary();
  const devProxyScript = resolveDevProxyScript();

  if (!fs.existsSync(binary)) {
    throw new Error(
      `nodespaced binary not found at ${binary}. ` +
      `Build with 'cargo build -p nodespaced' or set NODESPACED_BINARY.`
    );
  }

  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'nodespace-e2e-'));
  const socketPath = path.join(tmpDir, 'daemon.sock');
  const dbPath = path.join(tmpDir, 'test-db');
  const proxyPort = await findFreePort();

  const daemonEnv: NodeJS.ProcessEnv = {
    ...process.env,
    NODESPACED_SOCKET: socketPath,
    NODESPACED_DB_PATH: dbPath,
    // Isolate the whole `.nodespace` home under the temp dir so a spawned
    // daemon never seeds the real user's `~/.nodespace/databases.toml` with
    // this throwaway db path (ADR-053).
    NODESPACE_HOME: tmpDir,
    RUST_LOG: 'warn'
  };

  const daemonProc = child_process.spawn(binary, [], {
    env: daemonEnv,
    stdio: 'pipe',
    detached: false
  });

  daemonProc.stderr?.on('data', (chunk: Buffer) => {
    if (process.env.E2E_VERBOSE) {
      process.stderr.write(`[daemon] ${chunk.toString()}`);
    }
  });

  let daemonSpawnError: Error | undefined;
  daemonProc.on('error', (err: Error) => {
    daemonSpawnError = err;
  });

  const proxyEnv: NodeJS.ProcessEnv = {
    ...process.env,
    NODESPACED_SOCKET: socketPath,
    DEV_PROXY_PORT: String(proxyPort)
  };

  const proxyProc = child_process.spawn('bun', ['run', devProxyScript], {
    env: proxyEnv,
    stdio: 'pipe',
    detached: false
  });

  proxyProc.stderr?.on('data', (chunk: Buffer) => {
    if (process.env.E2E_VERBOSE) {
      process.stderr.write(`[proxy] ${chunk.toString()}`);
    }
  });

  let proxySpawnError: Error | undefined;
  proxyProc.on('error', (err: Error) => {
    proxySpawnError = err;
  });

  return {
    tmpDir,
    socketPath,
    proxyPort,
    daemonProc,
    proxyProc,
    daemonSpawnError,
    proxySpawnError
  };
}

// ============================================================================
// DaemonTestHarness
// ============================================================================

export class DaemonTestHarness {
  readonly adapter: HttpAdapter;
  private readonly tmpDir: string;
  private readonly socketPath: string;
  private readonly proxyPort: number;
  private readonly daemonProc: child_process.ChildProcess;
  private readonly proxyProc: child_process.ChildProcess;

  private constructor(opts: {
    adapter: HttpAdapter;
    tmpDir: string;
    socketPath: string;
    proxyPort: number;
    daemonProc: child_process.ChildProcess;
    proxyProc: child_process.ChildProcess;
  }) {
    this.adapter = opts.adapter;
    this.tmpDir = opts.tmpDir;
    this.socketPath = opts.socketPath;
    this.proxyPort = opts.proxyPort;
    this.daemonProc = opts.daemonProc;
    this.proxyProc = opts.proxyProc;
  }

  static async start(): Promise<DaemonTestHarness> {
    const timeoutMs = parseInt(process.env.E2E_DAEMON_TIMEOUT ?? '10000', 10);
    const spawned = await spawnDaemonAndProxy();

    // Wait for daemon socket
    try {
      await waitForSocket(spawned.socketPath, timeoutMs);
      if (spawned.daemonSpawnError !== undefined) {
        throw new Error(`nodespaced spawn failed: ${spawned.daemonSpawnError.message}`);
      }
    } catch (err) {
      spawned.daemonProc.kill();
      spawned.proxyProc.kill();
      fs.rmSync(spawned.tmpDir, { recursive: true, force: true });
      throw err;
    }

    // Wait for proxy HTTP health
    try {
      await waitForHttp(`http://localhost:${spawned.proxyPort}/health`, timeoutMs);
      if (spawned.proxySpawnError !== undefined) {
        throw new Error(`dev-proxy spawn failed: ${spawned.proxySpawnError.message}`);
      }
    } catch (err) {
      spawned.proxyProc.kill();
      spawned.daemonProc.kill();
      fs.rmSync(spawned.tmpDir, { recursive: true, force: true });
      throw err;
    }

    const adapter = new HttpAdapter(`http://localhost:${spawned.proxyPort}`);
    return new DaemonTestHarness({
      adapter,
      tmpDir: spawned.tmpDir,
      socketPath: spawned.socketPath,
      proxyPort: spawned.proxyPort,
      daemonProc: spawned.daemonProc,
      proxyProc: spawned.proxyProc
    });
  }

  /**
   * Spawn the daemon and dev-proxy WITHOUT waiting for the daemon's socket
   * to be reachable — only for the proxy's own HTTP liveness (its `/health`
   * reports the proxy process is up, independent of daemon reachability,
   * since gRPC-js channels dial lazily).
   *
   * For tests that need to observe a genuine not-ready → healthy transition
   * against a real daemon (readiness-contract tests), rather than always
   * starting from an already-healthy daemon like `start()` does.
   */
  static async startDeferred(): Promise<DaemonTestHarness> {
    const timeoutMs = parseInt(process.env.E2E_DAEMON_TIMEOUT ?? '10000', 10);
    const spawned = await spawnDaemonAndProxy();

    try {
      await waitForHttp(`http://localhost:${spawned.proxyPort}/health`, timeoutMs);
      if (spawned.proxySpawnError !== undefined) {
        throw new Error(`dev-proxy spawn failed: ${spawned.proxySpawnError.message}`);
      }
    } catch (err) {
      spawned.proxyProc.kill();
      spawned.daemonProc.kill();
      fs.rmSync(spawned.tmpDir, { recursive: true, force: true });
      throw err;
    }

    const adapter = new HttpAdapter(`http://localhost:${spawned.proxyPort}`);
    return new DaemonTestHarness({
      adapter,
      tmpDir: spawned.tmpDir,
      socketPath: spawned.socketPath,
      proxyPort: spawned.proxyPort,
      daemonProc: spawned.daemonProc,
      proxyProc: spawned.proxyProc
    });
  }

  /** One-shot UDS reachability probe for the daemon this harness spawned. */
  async isDaemonReachable(): Promise<boolean> {
    return probeSocket(this.socketPath);
  }

  /**
   * Wait until the daemon this harness spawned is reachable end-to-end
   * through the dev-proxy — which, since the dev-proxy's gRPC channel now
   * recovers promptly on its own, reduces to waiting for the daemon's UDS to
   * be reachable.
   *
   * This used to poll a real RPC (`getAllSchemas`) through the full HTTP ->
   * dev-proxy -> gRPC -> daemon stack, because the dev-proxy constructs its
   * gRPC-js client once at startup and — when started before the daemon binds
   * — the channel entered its own ever-growing reconnect backoff, independent
   * of the socket becoming reachable. A socket probe could report the daemon
   * up while the channel was still mid-backoff, so the only trustworthy
   * "ready" signal was the round-trip itself.
   *
   * `packages/dev-tools/src/grpc-client.ts` closes that gap at the source:
   * it caps the reconnect backoff to a fixed ~100ms and gates every RPC on
   * the channel reaching READY. Once the socket is reachable the channel is
   * usable within ~100ms, so the real-RPC poll is no longer needed — waiting
   * on the cheap socket probe is sufficient, and any RPC issued afterwards
   * self-heals inside the proxy rather than failing silently.
   */
  async waitUntilProxyReady(timeoutMs = 30_000): Promise<void> {
    await waitForSocket(this.socketPath, timeoutMs);
  }

  /** SSE endpoint URL for WatchNodes event tests. */
  get sseUrl(): string {
    return `http://localhost:${this.proxyPort}/api/events`;
  }

  async stop(): Promise<void> {
    this.proxyProc.kill('SIGTERM');
    this.daemonProc.kill('SIGTERM');

    // Give processes a moment to exit
    await new Promise((r) => setTimeout(r, 200));

    // Force-kill if still running
    try { this.proxyProc.kill('SIGKILL'); } catch { /* already dead */ }
    try { this.daemonProc.kill('SIGKILL'); } catch { /* already dead */ }

    // Remove temp directory
    try {
      fs.rmSync(this.tmpDir, { recursive: true, force: true });
    } catch {
      // Best-effort cleanup
    }
  }
}
