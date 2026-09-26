#!/usr/bin/env bun
// App-version single-source-of-truth guard.
//
// The desktop app's version is stamped into three files that must agree, or the
// running .app reports a version that doesn't match the build — a stale bundle
// silently under test. There is no build
// step that derives one field from another (Cargo, npm, and Tauri each read their
// own file), so the source of truth is enforced by CHECK, not by generation:
// `tauri.conf.json` is canonical (it stamps the bundle version the OS and the user
// see), and this guard fails if either sibling field drifts from it.
//
// A fourth field is covered the same way: the root Cargo.toml's
// [workspace.package] version. Every Rust workspace member crate (agent, cli,
// core, daemon, nlp-engine, nodespace-types, proto) inherits it via
// `version.workspace = true` rather than hardcoding its own, so checking this
// one field guards all seven at once. This matters at runtime, not just on
// disk: nodespace-daemon reads it via `env!("CARGO_PKG_VERSION")` for both the
// `nodespaced --version` flag (the installer's postinstall script queries it)
// and the `get_daemon_version` gRPC RPC, and nodespace-cli's `--version` flag
// reads it via clap's derived `version` field. Before this field existed, all
// seven crates hardcoded a version nothing kept in sync with the app version,
// so those surfaces silently reported a stale build.
//
// Run standalone (prints + exits non-zero on drift):  bun scripts/check-version-sync.ts
// Enforced automatically by the companion .test.ts under `bun test scripts/`
// (part of `test:all`, so the gate catches drift).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const APP = join(REPO, "packages", "desktop-app");

// Canonical first — everything else must equal it.
export const VERSION_SOURCES = {
  "tauri.conf.json": join(APP, "src-tauri", "tauri.conf.json"),
  "package.json": join(APP, "package.json"),
  "Cargo.toml": join(APP, "src-tauri", "Cargo.toml"),
  "workspace Cargo.toml": join(REPO, "Cargo.toml"),
} as const;

export const CANONICAL = "tauri.conf.json";

function readJsonVersion(path: string): string {
  const v = JSON.parse(readFileSync(path, "utf8"))?.version;
  if (typeof v !== "string") throw new Error(`no string "version" in ${path}`);
  return v;
}

// The [package] version, not a dependency's — match the first `version = "..."`
// that follows the `[package]` header so a `[dependencies]` pin can't be picked up.
function readCargoPackageVersion(path: string): string {
  const src = readFileSync(path, "utf8");
  const pkg = src.slice(src.indexOf("[package]"));
  const m = pkg.match(/^\s*version\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error(`no [package] version in ${path}`);
  return m[1];
}

// The [workspace.package] version, not a [workspace.dependencies] pin — same
// match-after-header approach as readCargoPackageVersion, against the root
// Cargo.toml's workspace section instead of a crate's [package] section.
function readCargoWorkspacePackageVersion(path: string): string {
  const src = readFileSync(path, "utf8");
  const section = src.slice(src.indexOf("[workspace.package]"));
  const m = section.match(/^\s*version\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error(`no [workspace.package] version in ${path}`);
  return m[1];
}

export interface VersionCheck {
  ok: boolean;
  canonical: string;
  versions: Record<string, string>;
  mismatches: string[];
}

export function checkAppVersionSync(): VersionCheck {
  const versions: Record<string, string> = {
    "tauri.conf.json": readJsonVersion(VERSION_SOURCES["tauri.conf.json"]),
    "package.json": readJsonVersion(VERSION_SOURCES["package.json"]),
    "Cargo.toml": readCargoPackageVersion(VERSION_SOURCES["Cargo.toml"]),
    "workspace Cargo.toml": readCargoWorkspacePackageVersion(VERSION_SOURCES["workspace Cargo.toml"]),
  };
  const canonical = versions[CANONICAL];
  const mismatches = Object.entries(versions)
    .filter(([name, v]) => name !== CANONICAL && v !== canonical)
    .map(([name, v]) => `${name}=${v}`);
  return { ok: mismatches.length === 0, canonical, versions, mismatches };
}

if (import.meta.main) {
  const r = checkAppVersionSync();
  if (r.ok) {
    console.log(`✅ app version in sync: ${r.canonical} (canonical: ${CANONICAL})`);
  } else {
    console.error(
      `❌ app version drift — canonical ${CANONICAL}=${r.canonical}, but: ${r.mismatches.join(", ")}\n` +
        `   Reconcile every field to the canonical value (bump the patch per build for validation).`,
    );
    process.exit(1);
  }
}
