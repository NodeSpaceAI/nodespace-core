#!/usr/bin/env bun
// A private address for this OS user's own gate sccache server. sccache runs
// exactly one server per address; this machine (and others like it) is
// shared by more than one macOS account, each with its own clone and
// `.tools/`. A single shared address would mean whichever account's gate
// started the server first silently owned it, and every other account's
// gate would then compile through a server process running as the wrong OS
// user — one that can't write that account's `target/` or temp files, so
// compiles fail with permission errors.
//
// `tmpdir()` is macOS-private per account (`/var/folders/.../T/`, mode
// 0700, owned by the calling user) on the only platform sccache is set up
// for today (setup-rust-tooling.ts pins a darwin-arm64 release only). A
// future non-macOS sccache release would need this rechecked: `tmpdir()`
// there is often the shared, world-writable `/tmp`, where a predictable
// per-uid filename is no longer private on its own.
//
// A gate that died mid-run (crash, kill -9, power loss) can leave a stale
// socket file with no listener behind. Verified against the pinned sccache
// build that `--start-server` cleanly rebinds over a stale path — no manual
// cleanup needed.
import { tmpdir } from "node:os";
import { join } from "node:path";

/**
 * A unix domain socket path private to `uid`, for `SCCACHE_SERVER_UDS`.
 * Keying it on the numeric uid keeps every account's server apart from
 * every other account's, and gives each account a stable path across runs.
 */
export function sccacheServerUds(uid: number): string {
  return join(tmpdir(), `nodespace-gate-sccache-${uid}.sock`);
}
