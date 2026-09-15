// Covers the pure string-building in scripts/build-windows.ts (Tier 2:
// Parallels VM automation). The script's own top-level flow (prlctl, ssh,
// scp) is behind `import.meta.main`, so importing it here runs nothing real
// -- no actual VM, no process spawns. There is no VM on this machine to
// verify `runRemoteBuild`/`waitForSsh`/`copyArtifactsBack` against; this
// tests the one part of Tier 2 that doesn't need one.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { buildRemoteScript, buildScpSources, envOrDefault, setupInstructions, VM_NAME } from "./build-windows";

describe("setupInstructions", () => {
  test("names the VM and points at the one-time setup doc", () => {
    const message = setupInstructions();
    expect(message).toContain(VM_NAME);
    expect(message).toContain("local-builds development doc");
  });

  test("suggests Tier 1 as the fallback while no VM exists", () => {
    expect(setupInstructions()).toContain("bun run build:windows:quick");
  });
});

describe("envOrDefault", () => {
  const UNSET = Symbol("unset");
  let saved: string | typeof UNSET;

  beforeEach(() => {
    saved = Object.prototype.hasOwnProperty.call(process.env, "NODESPACE_TEST_ENV_VAR")
      ? (process.env.NODESPACE_TEST_ENV_VAR as string)
      : UNSET;
  });

  afterEach(() => {
    if (saved === UNSET) delete process.env.NODESPACE_TEST_ENV_VAR;
    else process.env.NODESPACE_TEST_ENV_VAR = saved;
  });

  test("returns the fallback when the var is unset", () => {
    delete process.env.NODESPACE_TEST_ENV_VAR;
    expect(envOrDefault("NODESPACE_TEST_ENV_VAR", "fallback")).toBe("fallback");
  });

  // Regression coverage for a real bug caught by post-merge review:
  // `process.env.X ?? fallback` only catches null/undefined, not an
  // explicitly-empty-string override. `NODESPACE_WIN_VM_REPO_PATH=` (the var
  // set but empty -- a real shell/CI misconfiguration shape) would silently
  // produce `''` instead of falling back to the documented default.
  test("returns the fallback when the var is set but empty, not the empty string", () => {
    process.env.NODESPACE_TEST_ENV_VAR = "";
    expect(envOrDefault("NODESPACE_TEST_ENV_VAR", "fallback")).toBe("fallback");
  });

  test("returns the real value when the var is set and non-empty", () => {
    process.env.NODESPACE_TEST_ENV_VAR = "actual-value";
    expect(envOrDefault("NODESPACE_TEST_ENV_VAR", "fallback")).toBe("actual-value");
  });
});

describe("buildRemoteScript", () => {
  test("cds into the given repo path before anything else", () => {
    const script = buildRemoteScript("~/nodespace-core");
    expect(script.startsWith("cd '~/nodespace-core' &&")).toBe(true);
  });

  // Regression coverage for a real bug caught by post-merge review: an
  // earlier version interpolated `cd ${repoPath}` unquoted. The whole joined
  // script is sent as one command line to the remote shell with no further
  // escaping, so a repoPath containing a space -- a real shape, since
  // NODESPACE_WIN_VM_REPO_PATH is operator-configurable and Git-Bash-style
  // Windows paths like `/c/Users/Build Machine/nodespace-core` are exactly
  // this -- would word-split into an unexpected extra `cd` argument and fail
  // the whole build at its very first step.
  test("quotes a repo path containing a space so it survives as one cd argument", () => {
    const script = buildRemoteScript("/c/Users/Build Machine/nodespace-core");
    expect(script.startsWith("cd '/c/Users/Build Machine/nodespace-core' &&")).toBe(true);
  });

  test("chains every step with && so a failure stops the remote build", () => {
    const script = buildRemoteScript("~/nodespace-core");
    // Every step below must actually be present and `&&`-joined, not just
    // some -- this is the difference between a build that stops on the
    // first real failure and one that silently limps past it.
    const steps = script.split(" && ");
    expect(steps).toEqual([
      "cd '~/nodespace-core'",
      "git pull",
      "bun install --frozen-lockfile",
      "bun run --cwd packages/desktop-app sync",
      "cargo build --release --bin nodespaced --target x86_64-pc-windows-msvc",
      "cargo build --release --bin nodespace --target x86_64-pc-windows-msvc",
      "mkdir -p packages/desktop-app/src-tauri/binaries",
      "cp target/x86_64-pc-windows-msvc/release/nodespaced.exe packages/desktop-app/src-tauri/binaries/nodespaced-x86_64-pc-windows-msvc.exe",
      "cp target/x86_64-pc-windows-msvc/release/nodespace.exe packages/desktop-app/src-tauri/binaries/nodespace-x86_64-pc-windows-msvc.exe",
      "bun run build:skill",
      "bunx tauri build --target x86_64-pc-windows-msvc",
    ]);
  });

  test("builds the Tauri bundle with no --bundles restriction, unlike Tier 1", () => {
    // Tier 2 runs on a real Windows host, so unlike build-windows-quick.ts
    // (--bundles nsis, since WiX can't run cross-compiled) it can and should
    // produce every bundle tauri.conf.json's bundle.targets asks for,
    // .msi included.
    const script = buildRemoteScript("~/nodespace-core");
    expect(script).toContain("bunx tauri build --target x86_64-pc-windows-msvc");
    expect(script).not.toContain("--bundles");
  });

  test("respects a different repo path", () => {
    const script = buildRemoteScript("/c/Users/build/nodespace-core");
    expect(script.startsWith("cd '/c/Users/build/nodespace-core' &&")).toBe(true);
  });
});

describe("buildScpSources", () => {
  // Regression coverage for a real bug caught by adversarial review: an
  // earlier version interpolated `${SSH_USER}@${SSH_HOST}:${remoteBundleDir}/nsis
  // ${remoteBundleDir}/msi` directly in the `$` template. That left the msi
  // source with no `user@host:` prefix at all (scp treated it as a LOCAL
  // path), and -- separately -- Bun's `$` tilde-expands an interpolated value
  // that itself starts with `~` against the local host, so the default
  // REPO_PATH (`~/nodespace-core`) silently spliced this Mac's own home
  // directory into what must stay a purely remote path. Both are only
  // avoidable by building the complete `user@host:path` string in plain JS
  // first, which is what these assertions pin down.
  const remoteBundleDir = "~/nodespace-core/target/x86_64-pc-windows-msvc/release/bundle";

  test("both sources carry the user@host: prefix", () => {
    const { nsis, msi } = buildScpSources("nodespace", "nodespace-build-win.shared", remoteBundleDir);
    expect(nsis.startsWith("nodespace@nodespace-build-win.shared:")).toBe(true);
    expect(msi.startsWith("nodespace@nodespace-build-win.shared:")).toBe(true);
  });

  test("the user@host: prefix precedes any leading ~ in the path, so $ never sees a bare ~ segment", () => {
    const { nsis, msi } = buildScpSources("nodespace", "nodespace-build-win.shared", remoteBundleDir);
    // The character immediately after the LAST ':' is what a shell would
    // tilde-expand if it were the start of the whole argument -- here it's
    // '~', but only after "user@host:" already precedes it as one
    // unbroken string, which is exactly what keeps Bun's `$` from treating
    // it as a standalone leading-tilde value.
    expect(nsis).toBe(`nodespace@nodespace-build-win.shared:${remoteBundleDir}/nsis`);
    expect(msi).toBe(`nodespace@nodespace-build-win.shared:${remoteBundleDir}/msi`);
  });

  test("nsis and msi sources point at distinct subdirectories", () => {
    const { nsis, msi } = buildScpSources("nodespace", "nodespace-build-win.shared", remoteBundleDir);
    expect(nsis).not.toBe(msi);
    expect(nsis.endsWith("/nsis")).toBe(true);
    expect(msi.endsWith("/msi")).toBe(true);
  });

  test("respects a different user, host, and bundle dir", () => {
    const { nsis, msi } = buildScpSources("build", "10.0.0.5", "/c/repo/target/x86_64-pc-windows-msvc/release/bundle");
    expect(nsis).toBe("build@10.0.0.5:/c/repo/target/x86_64-pc-windows-msvc/release/bundle/nsis");
    expect(msi).toBe("build@10.0.0.5:/c/repo/target/x86_64-pc-windows-msvc/release/bundle/msi");
  });
});
