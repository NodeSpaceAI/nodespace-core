// Enforces the app-version single source of truth. If any of
// the four app-version fields drifts from the canonical tauri.conf.json, this test
// fails under `bun test scripts/` (part of `test:all`, run by the merge gate; the
// push check runs check-version-sync.ts itself) so the gates catch
// it before a stale-versioned build can ship.
import { describe, expect, test } from "bun:test";
import { CANONICAL, checkAppVersionSync } from "./check-version-sync";

describe("app version single source of truth", () => {
  test("all app-version fields equal the canonical tauri.conf.json", () => {
    const r = checkAppVersionSync();
    // A readable failure lists exactly which field drifted.
    expect(r.mismatches).toEqual([]);
    expect(r.ok).toBe(true);
  });

  test("canonical version is a valid semver x.y.z", () => {
    const r = checkAppVersionSync();
    expect(r.canonical).toMatch(/^\d+\.\d+\.\d+$/);
  });

  test("the guard covers all four known version fields", () => {
    const r = checkAppVersionSync();
    expect(Object.keys(r.versions).sort()).toEqual(
      ["Cargo.toml", "package.json", "tauri.conf.json", "workspace Cargo.toml"].sort(),
    );
    expect(r.versions[CANONICAL]).toBeDefined();
  });

  // Regression test: every Rust workspace member crate (agent, cli, core, daemon,
  // nlp-engine, nodespace-types, proto) inherits its version from the root
  // Cargo.toml's [workspace.package] field via `version.workspace = true`. Before
  // this field existed, every one of those crates hardcoded its own stale version,
  // which `nodespaced --version` and the `get_daemon_version` gRPC RPC both read
  // at compile time via `env!("CARGO_PKG_VERSION")` -- so a caller trusting either
  // surface to reflect the real running build was being lied to. This test fails
  // the same way the drift check above does if the workspace field goes stale
  // again, which is the failure mode that shipped undetected for multiple releases.
  test("workspace Cargo.toml version (inherited by every member crate) matches canonical", () => {
    const r = checkAppVersionSync();
    expect(r.versions["workspace Cargo.toml"]).toBe(r.canonical);
  });
});
