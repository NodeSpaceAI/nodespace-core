/**
 * gRPC-js client factory for the Bun dev-proxy (browser mode).
 *
 * This module owns the single seam where the dev-proxy talks to nodespaced
 * over a Unix-domain socket. It exists as its own module (rather than inline
 * in dev-proxy.ts) so the not-ready -> recovered behavior below can be
 * regression-tested against a stub gRPC server without spawning the whole
 * proxy or the real daemon.
 *
 * ---------------------------------------------------------------------------
 * Why this module reshapes gRPC-js's reconnect behavior
 * ---------------------------------------------------------------------------
 * The dev-proxy constructs its gRPC-js client ONCE, at startup, against the
 * daemon socket. When the proxy starts BEFORE the daemon binds that socket
 * (which the e2e readiness harness does deliberately), the gRPC-js channel's
 * subchannel enters TRANSIENT_FAILURE and starts its OWN reconnect backoff —
 * a timer completely independent of the daemon socket actually becoming
 * reachable moments later. Two properties of stock gRPC-js make that a real
 * bug for the dev path:
 *
 *   1. The reconnect backoff GROWS unboundedly while the peer is down
 *      (grpc-js defaults: 1s initial, x1.6 each attempt, up to 120s). By the
 *      time the daemon binds, the channel may be sitting out a multi-second
 *      backoff before it even retries the transport — so a raw socket probe
 *      can report the daemon up while the channel is still mid-backoff.
 *      `getConnectivityState(true)` / `client.waitForReady()` / an LB
 *      `resetBackoff()` do NOT cut this short: in grpc-js the backoff lives on
 *      the subchannel, the pick-first balancer's `resetBackoff` is a no-op,
 *      and `startConnecting()` on a TRANSIENT_FAILURE subchannel only marks
 *      "reconnect once the current timer ends" — it does not restart the
 *      timer. The only robust lever the public API exposes is bounding the
 *      backoff via channel options, which we do here: a fixed ~100ms retry
 *      interval means the channel re-probes the transport ~10x/second and
 *      recovers within ~100ms of the socket appearing, regardless of how long
 *      the daemon was down.
 *
 *   2. A unary call issued while the channel is NOT READY fails IMMEDIATELY
 *      with UNAVAILABLE by default (gRPC "wait for ready" is off by default),
 *      so callers that catch-and-log (e.g. loadSchemas) silently get nothing.
 *      `call`/`agentCall`/`agentStream` gate every RPC on
 *      `client.waitForReady(deadline)` first,
 *      so the call is issued only once the channel has actually reached READY.
 *      Combined with (1), that resolves within ~100ms of the daemon binding.
 *
 * The production Tauri path (Rust/tonic `connect_with_connector_lazy`) has no
 * equivalent cached-backoff state — tonic retries the connector fresh per
 * call — so this shaping is dev-proxy-specific and does not exist there.
 */

import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const PROTO_PATH = path.resolve(__dirname, '../../proto/proto/node_service.proto');
const AGENT_PROTO_PATH = path.resolve(__dirname, '../../proto/proto/local_agent_service.proto');

/**
 * Maximum size, in bytes, of a single gRPC message on the dev-proxy's channel.
 * Mirrors `MAX_MESSAGE_SIZE_BYTES` in `packages/proto/src/lib.rs`, which the
 * production tonic clients and the daemon use — both stacks default to 4 MiB,
 * which several list-shaped RPCs (`QueryNodesSimple`, `GetChildrenTree`,
 * `GetCollectionMembers`, …) exceed on a real database. Keeping the two in
 * step means browser mode fails, or doesn't, exactly where the packaged app
 * does. Update both together.
 */
const MAX_MESSAGE_SIZE_BYTES = 64 * 1024 * 1024;

/**
 * Channel options for the dev-proxy's gRPC-js clients.
 *
 * The reconnect pair bounds gRPC-js's backoff to a short, FIXED interval. This
 * is the root-cause fix for the not-ready path: it caps grpc-js's default
 * 1s->120s exponential backoff so a channel whose peer was unreachable
 * re-probes the transport roughly every 100ms and recovers promptly once the
 * socket appears — instead of sitting out an ever-growing backoff the caller
 * cannot control. Applies only to the dev-proxy's gRPC-js client, never to the
 * production tonic path.
 *
 * The message-size pair lifts grpc-js's own 4 MiB default (see
 * [`MAX_MESSAGE_SIZE_BYTES`]).
 */
export const DEV_PROXY_CHANNEL_OPTIONS: grpc.ClientOptions = {
  'grpc.initial_reconnect_backoff_ms': 100,
  'grpc.max_reconnect_backoff_ms': 100,
  'grpc.max_receive_message_length': MAX_MESSAGE_SIZE_BYTES,
  'grpc.max_send_message_length': MAX_MESSAGE_SIZE_BYTES
};

/**
 * Channel options for the long-lived `WatchNodes` stream.
 *
 * ---------------------------------------------------------------------------
 * Why the watch stream needs a connection of its own
 * ---------------------------------------------------------------------------
 * Parking a long-lived server-streaming call on the HTTP/2 connection the
 * unary RPCs share wedges that connection under Bun: after roughly 64 KiB of
 * response data has arrived, every later RPC on it hangs forever. The channel
 * still reports READY and the daemon sits idle — the requests never reach it.
 *
 * This is a Bun defect, not HTTP/2 semantics. The same grpc-js, daemon, socket
 * and channel options, varying ONLY the runtime, with a `WatchNodes` stream
 * open on the shared channel:
 *
 *   Bun 1.2.16     3/40 calls, 62,856 bytes, then wedged
 *   Node v26.8.1   40/40 calls, 838,080 bytes, clean
 *
 * Node never wedges, so grpc-js's own flow-control handling is not at fault.
 * The 62,856-byte stopping point sits just under HTTP/2's default 65,535-byte
 * connection window, which is why byte volume rather than call count decides
 * when it hits: `GetAllSchemas` (~21 KB) wedged on the 4th call, while
 * `GetDaemonVersion` (~19 bytes) ran 400/400. Whether Bun fails to emit its own
 * WINDOW_UPDATEs or mishandles hyper's, we did not determine — an in-process
 * grpc-js stub server does NOT reproduce the wedge, only the real tonic/hyper
 * daemon does, so something the two servers do differently is also involved.
 *
 * Widening the window from the client does not help. gRPC-js exposes
 * `grpc-node.flow_control_window` and calls `session.setLocalWindowSize()` on
 * `remoteSettings` (`@grpc/grpc-js/build/src/transport.js`); Bun provides that
 * method and it does not throw, but raising the window to 16 MiB left the wedge
 * at the same 3 calls and the same byte count. So the stream has to stop
 * sharing the connection.
 *
 * `grpc.use_local_subchannel_pool` is gRPC-js's supported way to ask for that.
 * `internal-channel.js` passes it to `getSubchannelPool`, which returns a fresh
 * `SubchannelPool` instead of the process-global one, so this client can never
 * be pooled onto the connection the unary clients share — separation by
 * construction rather than by incidentally-unequal options. Verified: 200/200
 * unary calls and 4.19 MB through the proxy, versus 3 before.
 *
 * Two independent barriers actually keep the connections apart, which is worth
 * knowing before editing this object. Beyond the pool selection above, reuse
 * inside a shared pool also requires `channelOptionsEqual` (`subchannel-pool.js`
 * -> `channel-options.js`), an exact key-and-value comparison that this extra
 * key already defeats. Deleting the key collapses both at once and silently
 * restores a bug whose symptom is a 30s hang with no error — so keep it here
 * rather than folding it into DEV_PROXY_CHANNEL_OPTIONS, and let
 * `watch-stream-isolation.e2e.ts` be the thing that catches the mistake.
 *
 * Switching the proxy to Node would also avoid the defect, but that trades a
 * one-line channel option for a runtime split against this repo's Bun-only
 * standard, and would change how the dev-proxy runs for developers rather than
 * only in tests. The production Tauri path (Rust/tonic on both ends, no Bun) is
 * unaffected either way, so this split stays dev-proxy-specific.
 */
export const WATCH_CHANNEL_OPTIONS: grpc.ClientOptions = {
  ...DEV_PROXY_CHANNEL_OPTIONS,
  'grpc.use_local_subchannel_pool': 1
};

/**
 * Ensures `run()`'s callback executes at most once for this instance; every
 * call after the first is a no-op. Unlike a flag shared across an entire
 * long-lived bridge, callers are expected to construct a FRESH instance per
 * logical attempt (e.g. per `ClientReadableStream`) — that scoping is what
 * makes the guard immune to cross-talk from an unrelated, already-superseded
 * attempt, rather than relying on event-ordering luck.
 *
 * `startWatchBridge()`'s `connect()` in dev-proxy.ts constructs one of these
 * per `WatchNodes` stream to fix a real bug found live-testing daemon-restart
 * recovery: a `ClientReadableStream` whose connection drops reliably fires
 * BOTH `'error'` and `'end'` for that one disconnect (confirmed with
 * `GRPC_TRACE` against a real daemon kill — every observed drop logged
 * "WatchNodes stream error" immediately followed by "WatchNodes stream
 * ended"). Scheduling a reconnect from both handlers independently — the
 * original code — creates two concurrent `connect()` chains per disconnect;
 * each one that itself fails again doubles the count. That compounds across
 * repeated daemon restarts into many overlapping `WatchNodes` stream attempts
 * that never quiesce: needless load on the daemon, and a proxy log that never
 * stops retrying even though any single chain would have recovered — which is
 * what made the bridge look like it "never recovers" after a restart.
 *
 * A single guard shared across every reconnect attempt (rather than one per
 * stream) would fix that same-stream double-event case but not structurally:
 * it would still allow a late event from an already-superseded stream to act
 * on a newer, already-healthy attempt's state, and it would arbitrarily let
 * whichever of 'error'/'end' happens to fire first dictate the reconnect
 * delay instead of the call site choosing one deliberately. Scoping to the
 * stream avoids both.
 */
export function createRunOnceGuard(): {
  run: (callback: () => void) => void;
} {
  let hasRun = false;
  return {
    run(callback) {
      if (hasRun) return;
      hasRun = true;
      callback();
    }
  };
}

export function resolveSocketAddress(): string {
  const sock =
    process.env.NODESPACED_SOCKET ?? `${process.env.HOME}/.nodespace/daemon.sock`;
  return `unix:${sock}`;
}

const loaderOptions: protoLoader.Options = {
  keepCase: false,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true
};

const proto = grpc.loadPackageDefinition(
  protoLoader.loadSync(PROTO_PATH, loaderOptions)
) as unknown as {
  nodespace: { NodeService: grpc.ServiceClientConstructor };
};

const agentProto = grpc.loadPackageDefinition(
  protoLoader.loadSync(AGENT_PROTO_PATH, loaderOptions)
) as unknown as {
  nodespace: { LocalAgentService: grpc.ServiceClientConstructor };
};

export interface NodeSpaceGrpcClients {
  address: string;
  nodeClient: grpc.Client;
  agentClient: grpc.Client;
  /**
   * `NodeService` client reserved for the long-lived `WatchNodes` stream. It
   * dials its own connection (see [`WATCH_CHANNEL_OPTIONS`]) so the parked
   * stream cannot exhaust the HTTP/2 connection window that `nodeClient`'s
   * unary RPCs depend on. Use it for `watchNodes` and nothing else.
   */
  watchClient: grpc.Client;
  /**
   * Wait for `client`'s channel to reach READY, actively driving a connection
   * attempt (and, thanks to DEV_PROXY_CHANNEL_OPTIONS, re-probing every
   * ~100ms). Resolves as soon as the transport is usable; rejects only if the
   * deadline passes with the channel still not READY.
   */
  ready: (client: grpc.Client, timeoutMs?: number) => Promise<void>;
  /** Promisified unary call on nodeClient, gated on the channel being READY. */
  call: <TReq, TRes>(method: Function, request: TReq) => Promise<TRes>;
  /** Promisified unary call on agentClient, gated on the channel being READY. */
  agentCall: <TReq, TRes>(method: Function, request: TReq) => Promise<TRes>;
  /** Server-streaming call on agentClient, gated on the channel being READY. */
  agentStream: <TReq, TEvent>(method: Function, request: TReq) => Promise<TEvent[]>;
  /**
   * Close every client this factory built. Callers that tear down should use
   * this rather than closing clients individually: enumerating them by hand
   * silently leaks whichever one a later change adds, and `watchClient`'s pool
   * is per-channel, so nothing else reclaims it.
   */
  closeAll: () => void;
}

/**
 * Build the dev-proxy's gRPC clients against `address` (a `unix:` target),
 * with the reconnect-backoff cap and the READY-gate wiring described above.
 */
export function createNodeSpaceClients(
  address: string = resolveSocketAddress()
): NodeSpaceGrpcClients {
  const nodeClient = new proto.nodespace.NodeService(
    address,
    grpc.credentials.createInsecure(),
    DEV_PROXY_CHANNEL_OPTIONS
  );
  const agentClient = new agentProto.nodespace.LocalAgentService(
    address,
    grpc.credentials.createInsecure(),
    DEV_PROXY_CHANNEL_OPTIONS
  );
  const watchClient = new proto.nodespace.NodeService(
    address,
    grpc.credentials.createInsecure(),
    WATCH_CHANNEL_OPTIONS
  );

  const ready = (client: grpc.Client, timeoutMs = 30_000): Promise<void> =>
    new Promise((resolve, reject) => {
      const deadline = new Date(Date.now() + timeoutMs);
      client.waitForReady(deadline, (err?: Error) => {
        if (err) reject(err);
        else resolve();
      });
    });

  const call = <TReq, TRes>(method: Function, request: TReq): Promise<TRes> =>
    ready(nodeClient).then(
      () =>
        new Promise<TRes>((resolve, reject) => {
          method.call(nodeClient, request, (err: grpc.ServiceError | null, res: TRes) => {
            if (err) reject(err);
            else resolve(res);
          });
        })
    );

  const agentCall = <TReq, TRes>(method: Function, request: TReq): Promise<TRes> =>
    ready(agentClient).then(
      () =>
        new Promise<TRes>((resolve, reject) => {
          method.call(agentClient, request, (err: grpc.ServiceError | null, res: TRes) => {
            if (err) reject(err);
            else resolve(res);
          });
        })
    );

  const agentStream = <TReq, TEvent>(method: Function, request: TReq): Promise<TEvent[]> =>
    ready(agentClient).then(
      () =>
        new Promise<TEvent[]>((resolve, reject) => {
          const stream = method.call(agentClient, request) as grpc.ClientReadableStream<TEvent>;
          const events: TEvent[] = [];
          stream.on('data', (evt: TEvent) => events.push(evt));
          stream.on('error', (err: grpc.ServiceError) => reject(err));
          stream.on('end', () => resolve(events));
        })
    );

  const closeAll = (): void => {
    nodeClient.close();
    agentClient.close();
    watchClient.close();
  };

  return {
    address,
    nodeClient,
    agentClient,
    watchClient,
    ready,
    call,
    agentCall,
    agentStream,
    closeAll
  };
}
