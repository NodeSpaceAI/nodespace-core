#!/usr/bin/env bun

/**
 * MCP HTTP+SSE Proxy Logger
 *
 * Sits between Claude Code and the MCP server to log all traffic.
 * Claude Code connects to this proxy, which forwards to the real MCP server.
 */

const PROXY_PORT = 3200;  // Claude Code connects here
const TARGET_PORT = 3100; // Real MCP server

console.log(`🔍 MCP Proxy Logger starting...`);
console.log(`   Proxy: http://localhost:${PROXY_PORT}/mcp`);
console.log(`   Target: http://localhost:${TARGET_PORT}/mcp`);
console.log(`\n📝 Logging all traffic...\n`);

const server = Bun.serve({
  port: PROXY_PORT,
  idleTimeout: 120, // 2 minutes idle timeout for SSE streams
  async fetch(req) {
    const url = new URL(req.url);
    console.log(`\n${'='.repeat(80)}`);
    console.log(`📥 ${req.method} ${url.pathname}`);
    console.log(`   Headers:`, Object.fromEntries(req.headers.entries()));

    // Read body for POST requests
    let body = null;
    if (req.method === 'POST') {
      body = await req.text();
      console.log(`   Body: ${body}`);
    }

    // Forward to real MCP server
    const targetUrl = `http://localhost:${TARGET_PORT}${url.pathname}`;
    console.log(`\n📤 Forwarding to ${targetUrl}`);

    try {
      const response = await fetch(targetUrl, {
        method: req.method,
        headers: req.headers,
        body: body,
      });

      console.log(`\n📨 Response: ${response.status} ${response.statusText}`);
      console.log(`   Headers:`, Object.fromEntries(response.headers.entries()));

      // For SSE, stream the response and log events
      if (response.headers.get('content-type')?.includes('text/event-stream')) {
        console.log(`   🌊 SSE Stream starting...`);

        const { readable, writable } = new TransformStream({
          transform(chunk, controller) {
            try {
              const text = new TextDecoder().decode(chunk);
              if (text.trim()) {
                console.log(`   📡 SSE: ${text.trim()}`);
              }
              controller.enqueue(chunk);
            } catch (err) {
              console.error(`   ⚠️ SSE decode error:`, err);
              controller.enqueue(chunk);
            }
          }
        });

        response.body.pipeTo(writable).catch(err => {
          console.error(`   ⚠️ SSE stream error:`, err.message);
        });

        return new Response(readable, {
          status: response.status,
          headers: response.headers,
        });
      }

      // For regular responses, log the body
      const responseBody = await response.text();
      console.log(`   Response Body: ${responseBody}`);

      return new Response(responseBody, {
        status: response.status,
        headers: response.headers,
      });
    } catch (error) {
      console.error(`\n❌ Proxy error:`, error);
      return new Response(JSON.stringify({ error: error.message }), {
        status: 500,
        headers: { 'Content-Type': 'application/json' },
      });
    }
  },
});

console.log(`✅ Proxy listening on port ${PROXY_PORT}`);
console.log(`\nUpdate .mcp.json to use: http://localhost:${PROXY_PORT}/mcp\n`);
