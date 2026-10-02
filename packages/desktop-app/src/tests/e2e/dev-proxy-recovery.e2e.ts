/**
 * E2E: dev-proxy gRPC channel recovers promptly when its peer appears LATE.
 *
 * This is the regression test for the dev-proxy readiness bug: the proxy
 * builds its gRPC-js client once, at startup, against the daemon socket. When
 * the proxy starts BEFORE the daemon binds that socket, stock gRPC-js puts the
 * channel into an ever-growing reconnect backoff (1s -> 120s) that is
 * independent of the socket actually becoming reachable, and a unary call
 * issued while the channel is not READY fails immediately with UNAVAILABLE.
 * Callers that catch-and-log (e.g. schemas.ts's loadSchemas) then silently get
 * nothing, which is why the harness previously had to poll a real RPC to
 * decide the proxy was "ready".
 *
 * The fix lives in `packages/dev-tools/src/grpc-client.ts`: it caps the
 * reconnect backoff to a fixed ~100ms and gates every RPC on the channel
 * reaching READY. This test drives that module DIRECTLY (no proxy process, no
 * daemon binary) against a stub gRPC server that binds LATE, and asserts:
 *
 *   - a SINGLE call issued before the peer exists still succeeds (no
 *     UNAVAILABLE, no caller-side real-RPC polling), and
 *   - it resolves PROMPTLY after the peer appears — within a window the
 *     capped backoff comfortably meets but stock gRPC-js's grown backoff
 *     would not (the peer binds well after the default 1s initial backoff,
 *     so an unfixed channel would be mid a multi-second backoff at that
 *     moment and resolve seconds late, if at all).
 */

import { describe, it, expect } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { createNodeSpaceClients } from '../../../../dev-tools/src/grpc-client';

const PROTO_PATH = path.resolve(__dirname, '../../../../proto/proto/node_service.proto');

interface StubHandle {
  server: grpc.Server;
  bind: () => Promise<void>;
}

/**
 * Build a NodeService gRPC server that answers GetAllSchemas, but do NOT bind
 * it yet — the caller decides when the socket appears, which is the whole
 * point of the test.
 */
function makeStubServer(socketPath: string): StubHandle {
  const def = protoLoader.loadSync(PROTO_PATH, {
    keepCase: false,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true
  });
  const pkg = grpc.loadPackageDefinition(def) as unknown as {
    nodespace: { NodeService: grpc.ServiceClientConstructor };
  };

  const handler = (
    _call: unknown,
    cb: (err: grpc.ServiceError | null, res: { schemasJson: string[] }) => void
  ) => cb(null, { schemasJson: [] });

  const server = new grpc.Server();
  server.addService((pkg.nodespace.NodeService as unknown as { service: grpc.ServiceDefinition }).service, {
    // Register under both name spellings so this doesn't depend on how
    // proto-loader keys the method with keepCase:false.
    getAllSchemas: handler,
    GetAllSchemas: handler
  } as unknown as grpc.UntypedServiceImplementation);

  const bind = () =>
    new Promise<void>((resolve, reject) => {
      server.bindAsync(
        `unix:${socketPath}`,
        grpc.ServerCredentials.createInsecure(),
        (err) => (err ? reject(err) : resolve())
      );
    });

  return { server, bind };
}

describe('dev-proxy gRPC channel: recovers promptly when the daemon appears late', () => {
  it(
    'a single RPC issued before the socket exists succeeds promptly once it does — no real-RPC polling',
    async () => {
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dev-proxy-recovery-'));
      const socketPath = path.join(tmpDir, 'daemon.sock');
      const stub = makeStubServer(socketPath);
      const clients = createNodeSpaceClients(`unix:${socketPath}`);

      // Peer binds LATE — well past gRPC-js's default 1s initial backoff, so
      // an unfixed channel would be sitting out a multi-second grown backoff
      // at this instant.
      const BIND_DELAY_MS = 3_000;
      let boundAt = 0;
      const bindPromise = new Promise<void>((resolve) => {
        setTimeout(() => {
          void stub.bind().then(() => {
            boundAt = Date.now();
            resolve();
          });
        }, BIND_DELAY_MS);
      });

      // Issue exactly ONE call, right now, against a socket that doesn't exist
      // yet. No polling, no retry loop in the caller — the proxy's own
      // channel must recover and let this call through.
      type UnaryMethod = (req: unknown, cb: (err: unknown, res: unknown) => void) => void;
      const getAllSchemas = (clients.nodeClient as unknown as Record<string, UnaryMethod>)
        .getAllSchemas;
      const res = await clients.call<Record<string, never>, { schemasJson: string[] }>(getAllSchemas, {});
      const resolvedAt = Date.now();

      await bindPromise;

      // The call came back with a real response, not a silent UNAVAILABLE.
      expect(Array.isArray(res.schemasJson)).toBe(true);

      // And it resolved promptly after the peer appeared. The capped backoff
      // recovers within ~100ms; stock gRPC-js's grown backoff would land this
      // well over a second late.
      const bindToResolveMs = resolvedAt - boundAt;
      expect(boundAt).toBeGreaterThan(0);
      expect(bindToResolveMs).toBeLessThan(800);

      clients.closeAll();
      await new Promise<void>((resolve) => stub.server.tryShutdown(() => resolve()));
      fs.rmSync(tmpDir, { recursive: true, force: true });
    },
    20_000
  );
});
