// The daemon's team keychain needs a signed bundle that carries the team's
// provisioning profile and the keychain access group entitlement. This file
// holds what can be checked without the profile or a signing identity: the
// bundle block of build-pkg.sh (run against fixture trees with a stub
// `codesign`), and that the entitlement, the bundle's plist and the daemon's own
// access group name the same thing. Whether the operating system honours the
// signed result is checked on a signed, provisioned build (not here).
import { describe, expect, test } from "bun:test";
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const BUILD_PKG_SH = join(REPO, "scripts", "build-pkg.sh");
const RESOURCES = join(REPO, "scripts", "pkg-resources");
const TEAM_KEYCHAIN_RS = join(REPO, "packages", "daemon", "src", "team_keychain.rs");

const BEGIN = "# --- BEGIN daemon-bundle";
const END = "# --- END daemon-bundle ---";

function extractBlock(): string {
  const src = readFileSync(BUILD_PKG_SH, "utf8");
  const begin = src.indexOf(BEGIN);
  const end = src.indexOf(END);
  if (begin === -1 || end === -1 || end < begin) throw new Error(`no ${BEGIN} block in build-pkg.sh`);
  return src.slice(begin, end);
}

interface Fixture {
  root: string;
  payload: string;
  log: string;
  run: (env: Record<string, string>) => { exitCode: number | null; stderr: string };
}

function fixture(): Fixture {
  const root = mkdtempSync(join(tmpdir(), "daemon-bundle-"));
  const payload = join(root, "payload");
  const stubs = join(root, "stubs");
  const log = join(root, "codesign.log");
  mkdirSync(join(payload, "usr", "local", "bin"), { recursive: true });
  mkdirSync(stubs);
  writeFileSync(join(payload, "usr", "local", "bin", "nodespaced"), "#!/bin/sh\necho daemon\n");
  chmodSync(join(payload, "usr", "local", "bin", "nodespaced"), 0o755);
  // A stub codesign that records its arguments and checks nothing else.
  writeFileSync(join(stubs, "codesign"), `#!/bin/sh\necho "$@" >> "${log}"\n`);
  chmodSync(join(stubs, "codesign"), 0o755);
  const snippet = extractBlock();
  return {
    root,
    payload,
    log,
    run: (env) => {
      const result = Bun.spawnSync(["bash", "-c", ["set -euo pipefail", snippet].join("\n")], {
        env: {
          PATH: `${stubs}:${process.env.PATH ?? ""}`,
          PAYLOAD_ROOT: payload,
          PKG_RESOURCES: RESOURCES,
          PKG_VERSION: "1.2.3",
          APP_IDENTITY: "Test Identity",
          ...env,
        },
        stdout: "pipe",
        stderr: "pipe",
      });
      return { exitCode: result.exitCode, stderr: result.stderr.toString() };
    },
  };
}

describe("daemon bundle in build-pkg.sh", () => {
  test("without a provisioning profile the daemon stays a bare binary and is not signed with the entitlement", () => {
    const f = fixture();
    try {
      const r = f.run({});
      expect(r.exitCode).toBe(0);
      const bin = join(f.payload, "usr", "local", "bin", "nodespaced");
      expect(lstatSync(bin).isSymbolicLink()).toBe(false);
      expect(existsSync(join(f.payload, "usr", "local", "lib"))).toBe(false);
      expect(existsSync(f.log)).toBe(false);
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  });

  test("with a profile the daemon is in a bundle that embeds it, signed with the entitlement, and the old path leads to it", () => {
    const f = fixture();
    try {
      const profile = join(f.root, "p.provisionprofile");
      writeFileSync(profile, "profile-bytes");
      const r = f.run({ NODESPACED_PROVISION_PROFILE: profile });
      expect(r.exitCode).toBe(0);
      const bundle = join(f.payload, "usr", "local", "lib", "nodespace", "NodeSpaceDaemon.app");
      expect(readFileSync(join(bundle, "Contents", "embedded.provisionprofile"), "utf8")).toBe("profile-bytes");
      expect(readFileSync(join(bundle, "Contents", "Info.plist"), "utf8")).toContain("<string>1.2.3</string>");
      expect(readFileSync(join(bundle, "Contents", "Info.plist"), "utf8")).not.toContain("__PKG_VERSION__");
      expect(readFileSync(join(bundle, "Contents", "MacOS", "nodespaced"), "utf8")).toContain("echo daemon");
      const link = join(f.payload, "usr", "local", "bin", "nodespaced");
      expect(lstatSync(link).isSymbolicLink()).toBe(true);
      expect(readFileSync(link, "utf8")).toContain("echo daemon"); // resolves
      expect(readlinkSync(link).startsWith("/")).toBe(false); // relative: survives the install root
      const signed = readFileSync(f.log, "utf8").trim().split("\n");
      expect(signed).toHaveLength(1);
      expect(signed[0]).toContain("--options runtime");
      expect(signed[0]).toContain(`--entitlements ${join(RESOURCES, "nodespaced.entitlements")}`);
      expect(signed[0]).toContain("--sign Test Identity");
      expect(signed[0].endsWith(bundle)).toBe(true);
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  });

  test("a profile path that is not a file stops the build", () => {
    const f = fixture();
    try {
      const r = f.run({ NODESPACED_PROVISION_PROFILE: join(f.root, "missing") });
      expect(r.exitCode).not.toBe(0);
      expect(r.stderr).toContain("is not a file");
    } finally {
      rmSync(f.root, { recursive: true, force: true });
    }
  });

  test("the entitlement is used only inside the profile-gated block", () => {
    const src = readFileSync(BUILD_PKG_SH, "utf8");
    const code = src
      .replace(extractBlock(), "")
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("#"))
      .join("\n");
    expect(code.includes("nodespaced.entitlements")).toBe(false);
  });
});

describe("the access group", () => {
  const onMac = process.platform === "darwin";
  const macTest = onMac ? test : test.skip;

  function plistJson(path: string): Record<string, unknown> {
    const out = Bun.spawnSync(["plutil", "-convert", "json", "-o", "-", path], { stdout: "pipe", stderr: "pipe" });
    if (out.exitCode !== 0) throw new Error(`plutil failed: ${out.stderr.toString()}`);
    return JSON.parse(out.stdout.toString()) as Record<string, unknown>;
  }

  macTest("the entitlement names the group the daemon uses and the team id the release signs with", () => {
    const rs = readFileSync(TEAM_KEYCHAIN_RS, "utf8");
    const group = /pub const ACCESS_GROUP: &str = "([^"]+)";/.exec(rs)?.[1];
    expect(group).toMatch(/^[A-Z0-9]{10}\.app\.nodespace\.daemon$/);
    const ent = plistJson(join(RESOURCES, "nodespaced.entitlements"));
    expect(ent["keychain-access-groups"]).toEqual([group]);
    expect(ent["com.apple.application-identifier"]).toBe(group);
    expect(ent["com.apple.developer.team-identifier"]).toBe(group?.split(".")[0]);
    const workflow = readFileSync(join(REPO, ".github", "workflows", "release.yml"), "utf8");
    expect(workflow).toContain(`(${group?.split(".")[0]})`);
  });

  macTest("the bundle's plist is the App ID of the entitlement and names the daemon executable", () => {
    const info = plistJson(join(RESOURCES, "nodespaced-bundle-Info.plist"));
    expect(info.CFBundleIdentifier).toBe("app.nodespace.daemon");
    expect(info.CFBundleExecutable).toBe("nodespaced");
    const ent = plistJson(join(RESOURCES, "nodespaced.entitlements"));
    expect(String(ent["com.apple.application-identifier"]).endsWith(`.${String(info.CFBundleIdentifier)}`)).toBe(true);
  });
});

describe("preinstall removes an old daemon bundle and link", () => {
  const PREINSTALL = join(RESOURCES, "preinstall");

  function block(): string {
    const src = readFileSync(PREINSTALL, "utf8");
    const begin = src.indexOf("# --- BEGIN daemon-link");
    const end = src.indexOf("# --- END daemon-link ---");
    if (begin === -1 || end === -1) throw new Error("no daemon-link block in preinstall");
    return src.slice(begin, end);
  }

  function run(volume: string): number | null {
    const script = ["set -euo pipefail", `VOLUME="${volume}"`, block(), "remove_daemon_bundle"].join("\n");
    return Bun.spawnSync(["bash", "-c", script], { stdout: "pipe", stderr: "pipe" }).exitCode;
  }

  test("a link and the bundle it leads to are removed, so the payload's regular file is not written through the link", () => {
    const root = mkdtempSync(join(tmpdir(), "preinstall-link-"));
    try {
      const bundleExe = join(root, "usr/local/lib/nodespace/NodeSpaceDaemon.app/Contents/MacOS");
      mkdirSync(bundleExe, { recursive: true });
      writeFileSync(join(bundleExe, "nodespaced"), "old");
      mkdirSync(join(root, "usr/local/bin"), { recursive: true });
      const link = join(root, "usr/local/bin/nodespaced");
      symlinkSync("../lib/nodespace/NodeSpaceDaemon.app/Contents/MacOS/nodespaced", link);
      expect(run(root)).toBe(0);
      expect(() => lstatSync(link)).toThrow();
      expect(existsSync(join(root, "usr/local/lib/nodespace/NodeSpaceDaemon.app"))).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("a link that is not into the daemon's bundle (a Homebrew link) survives", () => {
    const root = mkdtempSync(join(tmpdir(), "preinstall-brew-"));
    try {
      const cellar = join(root, "opt/homebrew/Cellar/nodespace/1.0/bin");
      mkdirSync(cellar, { recursive: true });
      writeFileSync(join(cellar, "nodespaced"), "brew");
      mkdirSync(join(root, "usr/local/bin"), { recursive: true });
      const link = join(root, "usr/local/bin/nodespaced");
      symlinkSync(join(cellar, "nodespaced"), link);
      expect(run(root)).toBe(0);
      expect(lstatSync(link).isSymbolicLink()).toBe(true);
      expect(readFileSync(link, "utf8")).toBe("brew");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("a regular daemon file is left alone", () => {
    const root = mkdtempSync(join(tmpdir(), "preinstall-file-"));
    try {
      mkdirSync(join(root, "usr/local/bin"), { recursive: true });
      const file = join(root, "usr/local/bin/nodespaced");
      writeFileSync(file, "bare");
      expect(run(root)).toBe(0);
      expect(readFileSync(file, "utf8")).toBe("bare");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("the removal runs on both paths of preinstall: no app installed, and an installed community app", () => {
    const src = readFileSync(PREINSTALL, "utf8");
    expect(src).toContain("    remove_daemon_bundle\n    exit 0");
    expect(src.trimEnd().endsWith("remove_daemon_bundle")).toBe(true);
  });
});
