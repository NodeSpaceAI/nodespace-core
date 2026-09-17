/**
 * Bun HTTP dev-proxy for browser mode
 *
 * Forwards REST calls from the Svelte frontend to nodespaced via gRPC,
 * and bridges the WatchNodes gRPC stream to browser SSE clients.
 *
 * Required flow:
 *   Playwright/Browser → dev-proxy (Bun/HTTP :3001) → gRPC → nodespaced → SQLite
 *
 * No external database required — nodespaced embeds SQLite/libsql.
 */

import * as grpc from '@grpc/grpc-js';
import {
  buildTaskNodeUpdatePatch,
  encodeInsertPosition,
  HTTP_ROUTE_PATTERNS,
  type InsertPosition,
} from '../../desktop-app/src/lib/services/adapter-core.ts';
import { createNodeSpaceClients } from './grpc-client.ts';
import { mapGrpcError } from './grpc-error-mapping.ts';

const PORT = parseInt(process.env.DEV_PROXY_PORT ?? '3001', 10);

// ============================================================================
// gRPC client setup
// ============================================================================
//
// The gRPC-js clients, the reconnect-backoff cap, and the READY-gate that make
// the dev-proxy recover promptly when the daemon socket appears AFTER the
// proxy starts all live in ./grpc-client.ts (see that file's header for the
// gRPC-js-specific rationale). `call`/`agentCall`/`agentStream` here are the
// gated wrappers — each waits for the channel to reach READY before issuing
// its RPC, so a call made moments after the daemon binds no longer fails
// silently with UNAVAILABLE.
const { address, nodeClient, agentClient, watchClient, ready, call, agentCall, agentStream } =
  createNodeSpaceClients();

// ============================================================================
// SSE broadcast
// ============================================================================

interface SseClient {
  id: string;
  controller: ReadableStreamDefaultController;
}

const sseClients = new Set<SseClient>();

function broadcast(event: Record<string, unknown>): void {
  const data = `data: ${JSON.stringify(event)}\n\n`;
  const encoded = new TextEncoder().encode(data);
  for (const client of sseClients) {
    try {
      client.controller.enqueue(encoded);
    } catch {
      sseClients.delete(client);
    }
  }
}

// ============================================================================
// WatchNodes → SSE bridge
// ============================================================================

// Proto NodeEvent oneof field names after camelCase conversion
interface ProtoNodeData {
  id: string;
  nodeType: string;
  content: string;
  parentId?: string;
  properties: string;
  version: string;
  lifecycleStatus: string;
  createdAt: string;
  modifiedAt: string;
  collectionId: string;
  // Server-computed display title (node_service.proto's NodeData.title) —
  // absent for a node type that never gets one (e.g. date/schema), so
  // optional like `parentId` above.
  title?: string;
}

interface ProtoNodeEvent {
  created?: ProtoNodeData;
  updated?: ProtoNodeData;
  deleted?: { nodeId: string; nodeType: string };
}

// Resolves once the daemon->dev-proxy gRPC watch stream is attached and
// broadcasting. `/api/events` awaits this before telling a browser client
// it is connected: the dev-proxy's own HTTP server (and thus `/health`)
// comes up before this bridge finishes connecting, so a client that arrived
// early enough could otherwise be told `: connected` while there is no
// upstream listener yet to relay the very write its own connection is
// racing against. `attachedResolve` is replaced on every reconnect so a
// client connecting during a bridge outage also waits for the new stream.
let attachedResolve: () => void;
let bridgeAttached: Promise<void> = new Promise((resolve) => {
  attachedResolve = resolve;
});

function resetBridgeAttached(): void {
  bridgeAttached = new Promise((resolve) => {
    attachedResolve = resolve;
  });
}

function startWatchBridge(): void {
  async function connect(): Promise<void> {
    // Gate the long-lived watch stream on the channel reaching READY too, so a
    // proxy started before the daemon opens the stream promptly once the
    // socket appears (per the bounded reconnect backoff) instead of churning
    // through immediate stream errors while the channel is mid-backoff.
    //
    // This runs on `watchClient`, which dials a connection of its own: parking
    // a stream on the connection the unary RPCs share exhausts its HTTP/2
    // window and wedges them permanently. See `WATCH_CHANNEL_OPTIONS` in
    // ./grpc-client.ts for the measurements and why the window cannot simply
    // be widened from the client.
    try {
      await ready(watchClient);
    } catch (err) {
      console.error('[dev-proxy] WatchNodes channel not ready, retrying in 2s:', (err as Error).message);
      setTimeout(connect, 2000);
      return;
    }
    const stream = (watchClient as unknown as Record<string, Function>).watchNodes({
      nodeType: '',
      rootId: ''
    }) as grpc.ClientReadableStream<ProtoNodeEvent>;

    stream.on('data', (event: ProtoNodeEvent) => {
      if (event.created) {
        broadcast({
          type: 'nodeCreated',
          nodeId: event.created.id,
          nodeType: event.created.nodeType
        });
      } else if (event.updated) {
        broadcast({
          type: 'nodeUpdated',
          nodeId: event.updated.id
        });
      } else if (event.deleted) {
        broadcast({
          type: 'nodeDeleted',
          nodeId: event.deleted.nodeId,
          nodeType: event.deleted.nodeType
        });
      }
    });

    stream.on('error', (err: Error) => {
      console.error('[dev-proxy] WatchNodes stream error, reconnecting in 2s:', err.message);
      resetBridgeAttached();
      setTimeout(connect, 2000);
    });

    stream.on('end', () => {
      console.log('[dev-proxy] WatchNodes stream ended, reconnecting in 1s');
      resetBridgeAttached();
      setTimeout(connect, 1000);
    });

    // All three listeners above are now wired, so this bridge will relay
    // every event the daemon emits from here on. Signal that now, not after
    // the first event arrives, so a client connecting between now and the
    // first write is still told `: connected` only once delivery is real.
    attachedResolve();
  }

  void connect();
}

// ============================================================================
// Response helpers
// ============================================================================

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json', ...corsHeaders }
  });
}

function error(code: string, message: string, status = 500, conflictData?: unknown): Response {
  const body: Record<string, unknown> = { code, message, details: message };
  if (conflictData !== undefined) body.conflictData = conflictData;
  return json(body, status);
}

// Mirrors the Tauri command layer's status_to_command_error
// (packages/desktop-app/src-tauri/src/commands/nodes.rs): inspects the same
// gRPC trailer metadata to build SUBTREE_ACCESS_DENIED/VERSION_CONFLICT codes
// with their conflictData payload, so a live refusal reached through
// bun run dev:browser surfaces via isSubtreeAccessDenied/isVersionConflict
// the same way it does through the Tauri path. See grpc-error-mapping.ts.
function grpcError(err: grpc.ServiceError): Response {
  const status = grpcStatusToHttp(err.code);
  const { code, conflictData } = mapGrpcError(err);
  return error(code, err.details ?? err.message, status, conflictData);
}

function grpcStatusToHttp(code: grpc.status): number {
  switch (code) {
    case grpc.status.NOT_FOUND:
      return 404;
    case grpc.status.ALREADY_EXISTS:
      return 409;
    case grpc.status.INVALID_ARGUMENT:
      return 400;
    case grpc.status.PERMISSION_DENIED:
      return 403;
    case grpc.status.UNAUTHENTICATED:
      return 401;
    case grpc.status.RESOURCE_EXHAUSTED:
      return 429;
    case grpc.status.ABORTED:
      return 409;
    // A subtree-access refusal (ADR-041) — see mapGrpcError. Distinct from
    // the 500 default so it isn't misread as a server-side failure.
    case grpc.status.FAILED_PRECONDITION:
      return 412;
    default:
      return 500;
  }
}

const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, PATCH, DELETE, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type, Authorization'
};

// ============================================================================
// Node shape helpers
// ============================================================================

function nodeDataToApiNode(n: ProtoNodeData): Record<string, unknown> {
  let properties: Record<string, unknown> = {};
  try {
    properties = n.properties ? (JSON.parse(n.properties) as Record<string, unknown>) : {};
  } catch {
    properties = {};
  }
  return {
    id: n.id,
    nodeType: n.nodeType,
    content: n.content,
    parentId: n.parentId && n.parentId !== '' ? n.parentId : null,
    properties,
    version: parseInt(n.version, 10),
    lifecycleStatus: n.lifecycleStatus,
    createdAt: n.createdAt,
    modifiedAt: n.modifiedAt,
    collectionId: n.collectionId && n.collectionId !== '' ? n.collectionId : null,
    // Unlike parentId/collectionId above (relationship references, where ""
    // and absent are the same "no relationship"), an empty title is a real,
    // distinct value from an absent one — a title_template whose fields are
    // all still blank legitimately computes to "" (see NodeData.title's own
    // doc comment in node_service.proto). Only true absence collapses to
    // null; an empty string passes through unchanged.
    title: n.title === undefined ? null : n.title
  };
}

// ============================================================================
// Request router
// ============================================================================

async function handleRequest(req: Request): Promise<Response> {
  const url = new URL(req.url);
  const pathname = url.pathname;
  const method = req.method;

  if (method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders });
  }

  // GET /health
  if (method === 'GET' && pathname === '/health') {
    return new Response('ok', { status: 200, headers: corsHeaders });
  }

  // GET /api/events  (SSE)
  if (method === 'GET' && pathname === '/api/events') {
    const clientId = url.searchParams.get('clientId') ?? crypto.randomUUID();
    let clientRef: SseClient;

    // See `bridgeAttached`'s doc comment above for why this must be
    // awaited before registering the client or flushing `: connected`.
    await bridgeAttached;

    let heartbeatTimer: ReturnType<typeof setInterval>;
    const stream = new ReadableStream({
      start(controller) {
        clientRef = { id: clientId, controller };
        sseClients.add(clientRef);
        controller.enqueue(new TextEncoder().encode(': connected\n\n'));
        // Keepalive every 15s — prevents Bun's idle timeout from closing the stream
        heartbeatTimer = setInterval(() => {
          try {
            controller.enqueue(new TextEncoder().encode(': ping\n\n'));
          } catch {
            sseClients.delete(clientRef);
            clearInterval(heartbeatTimer);
          }
        }, 15_000);
      },
      cancel() {
        sseClients.delete(clientRef);
        clearInterval(heartbeatTimer);
      }
    });

    return new Response(stream, {
      status: 200,
      headers: {
        ...corsHeaders,
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache',
        Connection: 'keep-alive'
      }
    });
  }

  // POST /api/nodes
  if (method === 'POST' && pathname === '/api/nodes') {
    try {
      const body = await req.json() as Record<string, unknown>;
      // Optional proto string fields (parentId, collection, lifecycleStatus) must be
      // omitted entirely for the "unset" case — sending '' is treated as Some("") by
      // the Rust side and triggers validation errors (empty parent lookup, empty path).
      const request: Record<string, unknown> = {
        nodeType: body.nodeType ?? '',
        content: body.content ?? '',
        properties: body.properties ? JSON.stringify(body.properties) : '',
        id: body.id ?? '',
        // encodeInsertPosition matches the `oneof position` in
        // CreateNodeRequest — the same builder TauriAdapter/HttpAdapter use,
        // so a browser-mode create-with-position can't silently drift into
        // always-append like the legacy moveNode route did.
        ...encodeInsertPosition(body.insertPosition as InsertPosition | null | undefined)
      };
      if (body.parentId && body.parentId !== '') request.parentId = body.parentId;
      if (body.collection && body.collection !== '') request.collection = body.collection;
      if (body.lifecycleStatus && body.lifecycleStatus !== '') request.lifecycleStatus = body.lifecycleStatus;
      const res = await call<typeof request, { nodeId: string }>(
        (nodeClient as unknown as Record<string, Function>).createNode,
        request
      );
      return json(res.nodeId);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id
  const getNodeMatch = pathname.match(HTTP_ROUTE_PATTERNS.getNode);
  if (method === 'GET' && getNodeMatch) {
    const nodeId = decodeURIComponent(getNodeMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).getNode,
        { nodeId }
      );
      if (!res.nodeData) return json(null);
      return json(nodeDataToApiNode(res.nodeData));
    } catch (err) {
      const grpcErr = err as grpc.ServiceError;
      if (grpcErr.code === grpc.status.NOT_FOUND) return json(null);
      return grpcError(grpcErr);
    }
  }

  // PATCH /api/nodes/:id
  if (method === 'PATCH' && getNodeMatch) {
    const nodeId = decodeURIComponent(getNodeMatch[1]);
    try {
      const body = await req.json() as Record<string, unknown>;
      // Optional proto fields must be omitted (not '') for the "no change" case.
      const request: Record<string, unknown> = {
        nodeId,
        version: body.version ?? null
      };
      if (body.nodeType && body.nodeType !== '') request.nodeType = body.nodeType;
      if (body.content !== undefined) request.content = String(body.content);
      if (body.properties !== undefined) request.properties = JSON.stringify(body.properties);
      if (body.addToCollection && body.addToCollection !== '') request.addToCollection = body.addToCollection;
      if (body.removeFromCollection && body.removeFromCollection !== '') request.removeFromCollection = body.removeFromCollection;
      if (body.lifecycleStatus && body.lifecycleStatus !== '') request.lifecycleStatus = body.lifecycleStatus;
      const res = await call<typeof request, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).updateNode,
        request
      );
      if (!res.nodeData) return error('NO_DATA', 'UpdateNode returned no data');
      return json(nodeDataToApiNode(res.nodeData));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // DELETE /api/nodes/:id
  if (method === 'DELETE' && getNodeMatch) {
    const nodeId = decodeURIComponent(getNodeMatch[1]);
    try {
      let body: Record<string, unknown> = {};
      try { body = await req.json() as Record<string, unknown>; } catch { /* no body */ }
      const request = {
        nodeId,
        version: body.version ?? null
      };
      await call<typeof request, unknown>(
        (nodeClient as unknown as Record<string, Function>).deleteNode,
        request
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      const grpcErr = err as grpc.ServiceError;
      if (grpcErr.code === grpc.status.NOT_FOUND) {
        return new Response(null, { status: 204, headers: corsHeaders });
      }
      return grpcError(grpcErr);
    }
  }

  // PATCH /api/tasks/:id
  const taskMatch = pathname.match(HTTP_ROUTE_PATTERNS.updateTaskNode);
  if (method === 'PATCH' && taskMatch) {
    const nodeId = decodeURIComponent(taskMatch[1]);
    try {
      const body = await req.json() as Record<string, unknown>;
      // buildTaskNodeUpdatePatch is the single authoritative tri-state
      // encoder (packages/desktop-app/src/lib/services/adapter-core.ts) —
      // absent field = no change, null = clear, value = set. Reused here so
      // this gRPC request can't drift from what the Tauri command layer
      // encodes for the same logical update.
      const patch = buildTaskNodeUpdatePatch(body as {
        status?: string;
        priority?: string | null;
        dueDate?: string | null;
        startedAt?: string | null;
        completedAt?: string | null;
        content?: string;
      });
      const request = {
        nodeId,
        version: body.version ?? 0,
        status: patch.status ?? null,
        priority: patch.priority ?? null,
        dueDate: patch.dueDate ?? null,
        startedAt: patch.startedAt ?? null,
        completedAt: patch.completedAt ?? null,
        content: patch.content ?? null,
        properties: body.properties !== undefined ? JSON.stringify(body.properties) : null
      };
      const res = await call<typeof request, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).updateTaskNode,
        request
      );
      if (!res.nodeData) return error('NO_DATA', 'UpdateTaskNode returned no data');
      return json(nodeDataToApiNode(res.nodeData));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/nodes/:id/parent  (move node)
  const parentMatch = pathname.match(HTTP_ROUTE_PATTERNS.moveNode);
  if (method === 'POST' && parentMatch) {
    const nodeId = decodeURIComponent(parentMatch[1]);
    try {
      const body = await req.json() as Record<string, unknown>;
      // `insertAfterNodeId` predates the `oneof position` shape in
      // MoveNodeRequest (node_service.proto) and no longer exists on the
      // wire — the field was silently dropped by proto-loader, so a browser
      // move-with-position always fell back to default "End" placement.
      // encodeInsertPosition is the same builder TauriAdapter/HttpAdapter
      // use for CreateNode, so this can't drift from what they send again.
      const request = {
        nodeId,
        version: body.version ?? 0,
        newParentId: body.parentId ?? '',
        ...encodeInsertPosition(body.insertPosition as InsertPosition | null | undefined)
      };
      const res = await call<typeof request, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).moveNode,
        request
      );
      if (!res.nodeData) return error('NO_DATA', 'MoveNode returned no data');
      return json(nodeDataToApiNode(res.nodeData));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/children
  const childrenMatch = pathname.match(HTTP_ROUTE_PATTERNS.getChildren);
  if (method === 'GET' && childrenMatch) {
    const nodeId = decodeURIComponent(childrenMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).getChildren,
        { nodeId }
      );
      return json((res.nodes ?? []).map(nodeDataToApiNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/children-tree
  const childrenTreeMatch = pathname.match(HTTP_ROUTE_PATTERNS.getChildrenTree);
  if (method === 'GET' && childrenTreeMatch) {
    const nodeId = decodeURIComponent(childrenTreeMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { treeJson: string }>(
        (nodeClient as unknown as Record<string, Function>).getChildrenTree,
        { nodeId }
      );
      const tree = res.treeJson ? JSON.parse(res.treeJson) : {};
      return json(tree);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/relationships
  const relationshipsMatch = pathname.match(HTTP_ROUTE_PATTERNS.getNodeRelationships);
  if (method === 'GET' && relationshipsMatch) {
    const nodeId = decodeURIComponent(relationshipsMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { relationshipsJson: string }>(
        (nodeClient as unknown as Record<string, Function>).getNodeRelationships,
        { nodeId }
      );
      // relationships_json is already camelCase (the daemon serializes its typed
      // NodeRelationshipsOutput to JSON); parse and forward the aggregate as-is,
      // matching what the Tauri get_node_relationships command returns.
      const aggregate = res.relationshipsJson ? JSON.parse(res.relationshipsJson) : {};
      return json(aggregate);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/query
  if (method === 'POST' && pathname === '/api/query') {
    try {
      const body = await req.json() as Record<string, unknown>;
      const request = {
        id: body.id ?? null,
        mentionedBy: body.mentionedBy ?? null,
        contentContains: body.contentContains ?? null,
        titleContains: body.titleContains ?? null,
        nodeType: body.nodeType ?? null,
        limit: body.limit ?? 0,
        offset: body.offset ?? 0
      };
      const res = await call<typeof request, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).queryNodesSimple,
        request
      );
      return json((res.nodes ?? []).map(nodeDataToApiNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/query/execute
  if (method === 'POST' && pathname === '/api/query/execute') {
    try {
      const body = await req.json() as Record<string, unknown>;
      const request = {
        // Required by the RPC, unlike its optional siblings — a missing one is
        // a malformed request the daemon should reject, not a defaulted empty.
        targetType: body.targetType,
        filtersJson: body.filtersJson ?? null,
        sortingJson: body.sortingJson ?? null,
        limit: body.limit ?? 0
      };
      const res = await call<typeof request, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).executeQuery,
        request
      );
      return json((res.nodes ?? []).map(nodeDataToApiNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/query/count
  if (method === 'POST' && pathname === '/api/query/count') {
    try {
      const body = await req.json() as Record<string, unknown>;
      // Same request message as /api/query/execute — the daemon ignores
      // sortingJson/limit for a count, so they are forwarded as given rather
      // than special-cased here.
      const request = {
        targetType: body.targetType,
        filtersJson: body.filtersJson ?? null,
        sortingJson: body.sortingJson ?? null,
        limit: body.limit ?? 0
      };
      const res = await call<typeof request, { count: string }>(
        (nodeClient as unknown as Record<string, Function>).countQuery,
        request
      );
      // `count` is a proto int64, and grpc-client.ts loads with `longs: String`
      // — so it arrives as a decimal string and must be converted before it
      // reaches a caller that expects a number.
      return json(Number(res.count ?? 0));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/mentions
  if (method === 'POST' && pathname === '/api/mentions') {
    try {
      const body = await req.json() as Record<string, unknown>;
      await call(
        (nodeClient as unknown as Record<string, Function>).createMention,
        { mentioningNodeId: body.sourceId, mentionedNodeId: body.targetId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // DELETE /api/mentions
  if (method === 'DELETE' && pathname === '/api/mentions') {
    try {
      const body = await req.json() as Record<string, unknown>;
      await call(
        (nodeClient as unknown as Record<string, Function>).deleteMention,
        { mentioningNodeId: body.sourceId, mentionedNodeId: body.targetId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/relationships  (create typed relationship edge)
  if (method === 'POST' && pathname === '/api/relationships') {
    try {
      const body = await req.json() as Record<string, unknown>;
      const request: Record<string, unknown> = {
        sourceId: body.sourceId,
        relationshipName: body.relationshipName,
        targetId: body.targetId
      };
      // edge_data_json is proto `optional string`; omit it entirely for a bare
      // edge so the daemon defaults to "{}" rather than seeing an empty string.
      if (body.edgeData !== undefined && body.edgeData !== null) {
        request.edgeDataJson = JSON.stringify(body.edgeData);
      }
      await call(
        (nodeClient as unknown as Record<string, Function>).createRelationship,
        request
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // DELETE /api/relationships  (remove typed relationship edge)
  if (method === 'DELETE' && pathname === '/api/relationships') {
    try {
      const body = await req.json() as Record<string, unknown>;
      await call(
        (nodeClient as unknown as Record<string, Function>).deleteRelationship,
        { sourceId: body.sourceId, relationshipName: body.relationshipName, targetId: body.targetId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // PATCH /api/relationships  (replace edge attributes wholesale)
  if (method === 'PATCH' && pathname === '/api/relationships') {
    try {
      const body = await req.json() as Record<string, unknown>;
      await call(
        (nodeClient as unknown as Record<string, Function>).updateRelationshipProperties,
        {
          sourceId: body.sourceId,
          relationshipName: body.relationshipName,
          targetId: body.targetId,
          propertiesJson: JSON.stringify(body.properties ?? {})
        }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/mentions/autocomplete
  if (method === 'POST' && pathname === '/api/mentions/autocomplete') {
    try {
      const body = await req.json() as Record<string, unknown>;
      const res = await call<{ query: string; limit: number }, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).mentionAutocomplete,
        { query: String(body.query ?? ''), limit: Number(body.limit ?? 0) }
      );
      return json((res.nodes ?? []).map(nodeDataToApiNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/nodes/find-duplicate
  // Suggest-don't-block uniqueness lookup (ADR-065): "no conflict" is an
  // ordinary empty NodeResponse from the daemon, never a NotFound status, so
  // this always resolves 200 with either a node or `null` — never an error.
  if (method === 'POST' && pathname === '/api/nodes/find-duplicate') {
    try {
      const body = await req.json() as Record<string, unknown>;
      // exclude_id is an `optional string` proto field — omit the key entirely
      // for "no exclusion" rather than sending null/'', matching the optional
      // proto-field convention used above (see PATCH /api/nodes/:id).
      const request: Record<string, unknown> = {
        nodeType: String(body.nodeType ?? ''),
        field: String(body.field ?? ''),
        value: String(body.value ?? '')
      };
      if (body.excludeId && body.excludeId !== '') request.excludeId = body.excludeId;
      const res = await call<typeof request, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).findDuplicate,
        request
      );
      if (!res.nodeData) return json(null);
      return json(nodeDataToApiNode(res.nodeData));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/mentions/outgoing
  const outgoingMatch = pathname.match(HTTP_ROUTE_PATTERNS.getOutgoingMentions);
  if (method === 'GET' && outgoingMatch) {
    const nodeId = decodeURIComponent(outgoingMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { nodeIds: string[] }>(
        (nodeClient as unknown as Record<string, Function>).getOutgoingMentions,
        { nodeId }
      );
      return json(res.nodeIds ?? []);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/mentions/incoming
  const incomingMatch = pathname.match(HTTP_ROUTE_PATTERNS.getIncomingMentions);
  if (method === 'GET' && incomingMatch) {
    const nodeId = decodeURIComponent(incomingMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { nodeIds: string[] }>(
        (nodeClient as unknown as Record<string, Function>).getIncomingMentions,
        { nodeId }
      );
      return json(res.nodeIds ?? []);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/nodes/:id/mentions/roots
  const mentionRootsMatch = pathname.match(HTTP_ROUTE_PATTERNS.getMentioningContainers);
  if (method === 'GET' && mentionRootsMatch) {
    const nodeId = decodeURIComponent(mentionRootsMatch[1]);
    try {
      const res = await call<{ nodeId: string }, { references: Array<{ id: string; title?: string; nodeType: string }> }>(
        (nodeClient as unknown as Record<string, Function>).getMentioningRoots,
        { nodeId }
      );
      // Backend adapter expects string[] (node IDs) for getMentioningContainers
      return json((res.references ?? []).map((r) => r.id));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/schemas
  if (method === 'GET' && pathname === '/api/schemas') {
    try {
      const res = await call<Record<string, never>, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).getAllSchemas,
        {}
      );
      return json((res.nodes ?? []).map(nodeDataToSchemaNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/schemas/:id
  const schemaMatch = pathname.match(HTTP_ROUTE_PATTERNS.getSchema);
  if (method === 'GET' && schemaMatch) {
    const schemaId = decodeURIComponent(schemaMatch[1]);
    try {
      const res = await call<{ schemaId: string }, { nodeData?: ProtoNodeData }>(
        (nodeClient as unknown as Record<string, Function>).getSchemaDefinition,
        { schemaId }
      );
      if (!res.nodeData) return error('SCHEMA_NOT_FOUND', `Schema '${schemaId}' not found`, 404);
      return json(nodeDataToSchemaNode(res.nodeData));
    } catch (err) {
      const grpcErr = err as grpc.ServiceError;
      if (grpcErr.code === grpc.status.NOT_FOUND) {
        return error('SCHEMA_NOT_FOUND', `Schema '${schemaId}' not found`, 404);
      }
      return grpcError(grpcErr);
    }
  }

  // GET /api/daemon/version
  if (method === 'GET' && pathname === '/api/daemon/version') {
    try {
      const res = await call<Record<string, never>, { version: string }>(
        (nodeClient as unknown as Record<string, Function>).getDaemonVersion,
        {}
      );
      return json(res.version);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/collections
  if (method === 'GET' && pathname === '/api/collections') {
    try {
      const res = await call<Record<string, never>, { collections: Array<{ node: ProtoNodeData; memberCount: number; parentCollectionIds: string[] }> }>(
        (nodeClient as unknown as Record<string, Function>).getAllCollections,
        {}
      );
      const result = (res.collections ?? []).map((c) => ({
        ...nodeDataToApiNode(c.node),
        memberCount: c.memberCount,
        parentCollectionIds: c.parentCollectionIds ?? []
      }));
      return json(result);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/collections/:id/members
  const collectionMembersMatch = pathname.match(/^\/api\/collections\/([^/]+)\/members$/);
  if (method === 'GET' && collectionMembersMatch) {
    const collectionId = decodeURIComponent(collectionMembersMatch[1]);
    try {
      const res = await call<{ collectionId: string }, { nodes: ProtoNodeData[] }>(
        (nodeClient as unknown as Record<string, Function>).getCollectionMembers,
        { collectionId }
      );
      return json((res.nodes ?? []).map(nodeDataToApiNode));
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // -------------------------------------------------------------------------
  // LocalAgentService — model management routes
  // -------------------------------------------------------------------------

  // GET /api/agent/models
  if (method === 'GET' && pathname === '/api/agent/models') {
    try {
      const forceRefresh = url.searchParams.get('force_refresh') === 'true';
      const res = await agentCall<{ forceRefresh: boolean }, { models: Array<Record<string, unknown>> }>(
        (agentClient as unknown as Record<string, Function>).listModels,
        { forceRefresh }
      );
      const models = (res.models ?? []).map((m) => ({
        id: m.id,
        name: m.name,
        backend: m.backend,
        status: m.statusJson ? JSON.parse(m.statusJson as string) : { status: 'not_downloaded' },
        sizeBytes: parseInt(String(m.sizeBytes ?? '0'), 10),
        quantization: m.quantization ?? '',
        minMemoryGb: Number(m.minMemoryGb ?? 0),
      }));
      return json(models);
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/agent/models/:id/download
  const modelDownloadMatch = pathname.match(/^\/api\/agent\/models\/([^/]+)\/download$/);
  if (method === 'POST' && modelDownloadMatch) {
    const modelId = decodeURIComponent(modelDownloadMatch[1]);
    try {
      await agentStream(
        (agentClient as unknown as Record<string, Function>).downloadModel,
        { modelId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // DELETE /api/agent/models/:id
  const modelDeleteMatch = pathname.match(/^\/api\/agent\/models\/([^/]+)$/);
  if (method === 'DELETE' && modelDeleteMatch) {
    const modelId = decodeURIComponent(modelDeleteMatch[1]);
    try {
      await agentCall(
        (agentClient as unknown as Record<string, Function>).deleteModel,
        { modelId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/agent/models/:id/load
  const modelLoadMatch = pathname.match(/^\/api\/agent\/models\/([^/]+)\/load$/);
  if (method === 'POST' && modelLoadMatch) {
    const modelId = decodeURIComponent(modelLoadMatch[1]);
    try {
      await agentCall(
        (agentClient as unknown as Record<string, Function>).loadModel,
        { modelId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/agent/models/unload
  if (method === 'POST' && pathname === '/api/agent/models/unload') {
    try {
      await agentCall(
        (agentClient as unknown as Record<string, Function>).unloadModel,
        {}
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // DELETE /api/agent/models/:id/download  (cancel)
  const modelCancelMatch = pathname.match(/^\/api\/agent\/models\/([^/]+)\/download$/);
  if (method === 'DELETE' && modelCancelMatch) {
    const modelId = decodeURIComponent(modelCancelMatch[1]);
    try {
      await agentCall(
        (agentClient as unknown as Record<string, Function>).cancelModelDownload,
        { modelId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/agent/recommended-model
  if (method === 'GET' && pathname === '/api/agent/recommended-model') {
    try {
      const res = await agentCall<Record<string, never>, { modelId: string }>(
        (agentClient as unknown as Record<string, Function>).recommendedModel,
        {}
      );
      return json({ modelId: res.modelId });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // GET /api/agent/system-ram
  if (method === 'GET' && pathname === '/api/agent/system-ram') {
    try {
      const res = await agentCall<Record<string, never>, { ramBytes: string }>(
        (agentClient as unknown as Record<string, Function>).getSystemRam,
        {}
      );
      const ramBytes = parseInt(res.ramBytes ?? '0', 10);
      return json({ ramGb: Math.floor(ramBytes / (1024 ** 3)) });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/agent/ensure-model-ready
  if (method === 'POST' && pathname === '/api/agent/ensure-model-ready') {
    try {
      const body = await req.json() as { modelId: string };
      await agentStream(
        (agentClient as unknown as Record<string, Function>).ensureModelReady,
        { modelId: body.modelId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  // POST /api/agent/cancel-turn
  if (method === 'POST' && pathname === '/api/agent/cancel-turn') {
    try {
      const body = await req.json() as { nodeId: string };
      await agentCall(
        (agentClient as unknown as Record<string, Function>).cancelTurn,
        { nodeId: body.nodeId }
      );
      return new Response(null, { status: 204, headers: corsHeaders });
    } catch (err) {
      return grpcError(err as grpc.ServiceError);
    }
  }

  return new Response('Not found', { status: 404, headers: corsHeaders });
}

// ============================================================================
// Schema node helpers
// ============================================================================

function nodeDataToSchemaNode(n: ProtoNodeData): Record<string, unknown> {
  const base = nodeDataToApiNode(n);
  const props = base.properties as Record<string, unknown>;
  return {
    ...base,
    isCore: props.isCore ?? false,
    schemaVersion: props.schemaVersion ?? 1,
    description: props.description ?? '',
    fields: props.fields ?? []
  };
}

// ============================================================================
// Server startup
// ============================================================================

startWatchBridge();

const server = Bun.serve({
  port: PORT,
  idleTimeout: 0,
  fetch: handleRequest,
  error(err: Error) {
    console.error('[dev-proxy] Unhandled error:', err);
    return new Response('Internal Server Error', { status: 500 });
  }
});

console.log(`[dev-proxy] Listening on http://localhost:${PORT}`);
console.log(`[dev-proxy] Connecting to nodespaced at ${address}`);
console.log(`[dev-proxy] SSE endpoint: http://localhost:${PORT}/api/events`);
