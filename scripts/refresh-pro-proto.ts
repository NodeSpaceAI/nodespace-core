#!/usr/bin/env bun
/**
 * Dev-only: refresh the vendored Pro proto from the source-of-truth in the
 * private `nodespace-sync` repo. CI does not run this — the vendored
 * `proto/nodespace_pro.proto` checked into the public repo is what gets
 * compiled into the Tauri binary.
 *
 * Expects `nodespace-sync` to be a sibling directory of `nodespace-core`.
 * Sync access is required (CI will never have it); refusing to run with a
 * clear error is the right failure mode when the sibling tree is missing.
 *
 * Usage:
 *   bun run scripts/refresh-pro-proto.ts
 */

import { copyFileSync, existsSync } from "fs";
import { join } from "path";

const REPO_ROOT = join(import.meta.dir, "..");
const SRC = join(
  REPO_ROOT,
  "..",
  "nodespace-sync",
  "nodespaced-pro",
  "proto",
  "nodespace_pro.proto",
);
const DST = join(
  REPO_ROOT,
  "packages",
  "desktop-app",
  "app-lib",
  "proto",
  "nodespace_pro.proto",
);

function refreshProto() {
  if (!existsSync(SRC)) {
    console.error(`error: source proto not found at ${SRC}`);
    console.error(
      "       (expected ../nodespace-sync/nodespaced-pro/proto/nodespace_pro.proto relative to this repo)",
    );
    process.exit(1);
  }

  copyFileSync(SRC, DST);
  console.log(`✓ refreshed ${DST} from ${SRC}`);
}

if (import.meta.main) {
  if (process.argv.includes("--help") || process.argv.includes("-h")) {
    console.log("Usage: bun run scripts/refresh-pro-proto.ts");
    console.log(
      "Re-vendors nodespace_pro.proto from a sibling nodespace-sync checkout.",
    );
    process.exit(0);
  }
  refreshProto();
}
