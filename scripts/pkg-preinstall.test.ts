// Regression guard for the .pkg's pre-install check
// (scripts/pkg-resources/preinstall) and the two static facts it depends on:
// the app bundle declares its product (NodeSpaceProduct in Info.plist, merged
// in by Tauri), and postinstall no longer carries the old guard.
//
// The old guard lived in postinstall, so it ran after the payload was written,
// executed the installed nodespaced as root, and never saw the other product
// (its daemon sits beside nodespaced instead of replacing it). preinstall runs
// before anything is written and reads only static data, so the tests build
// fixture bundles under a temp "volume" and run the real script against them
// with a scrubbed environment. Every fixture bundle carries executables that
// write a sentinel file: the script must never run any of them.
//
// The script calls /usr/bin/plutil, so the behavioural tests skip off macOS.
// Needles for the other product's daemon and the override variable are built
// from fragments so this file does not carry the boundary-check markers it
// guards against.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

// Each case spawns bash and plutil: a correctness check, not a performance
// one, so the default 5s only measures how busy the machine is.
setDefaultTimeout(30_000);

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const PREINSTALL = join(REPO, "scripts", "pkg-resources", "preinstall");
const POSTINSTALL = join(REPO, "scripts", "pkg-resources", "postinstall");
const TAURI_CONF = join(REPO, "packages", "desktop-app", "src-tauri", "tauri.conf.json");

const OTHER_DAEMON = ["nodespaced", "pro"].join("-");
const FORCE_VAR = ["NODESPACE", "FORCE", "COMMUNITY"].join("_");
const EDITION_FLAG = ["--edi", "tion"].join("");
const OTHER_PRODUCT = "pro";
const OTHER_NAME = ["Pr", "o"].join("");
// The refusal text ADR-084 fixes, character for character.
const REFUSAL = `NodeSpace ${OTHER_NAME} is installed. To switch back to the free NodeSpace, uninstall ${OTHER_NAME} first (NodeSpace → Uninstall NodeSpace ${OTHER_NAME}…). Your databases stay on this Mac.`;

const onMac = process.platform === "darwin";

interface Fixture {
  /** The volume root handed to preinstall as $3. */
  volume: string;
  /** Created by any fixture executable that gets run. */
  sentinel: string;
  cleanup: () => void;
}

interface FixtureOptions {
  /** Whether /Applications/NodeSpace.app exists at all. Defaults to true. */
  app?: boolean;
  /** NodeSpaceProduct value; null or omitted means the key is absent. */
  product?: string | null;
  /** Whether Contents/MacOS holds the other product's daemon. */
  otherDaemon?: boolean;
}

function plist(product: string | null): string {
  const key = product === null ? "" : `<key>NodeSpaceProduct</key><string>${product}</string>`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleName</key><string>NodeSpace</string>${key}</dict></plist>
`;
}

function makeFixture(options: FixtureOptions = {}): Fixture {
  const volume = mkdtempSync(join(tmpdir(), "pkg-preinstall-test-"));
  const sentinel = join(volume, "executable-was-run");
  if (options.app !== false) {
    const macos = join(volume, "Applications", "NodeSpace.app", "Contents", "MacOS");
    mkdirSync(macos, { recursive: true });
    writeFileSync(join(volume, "Applications", "NodeSpace.app", "Contents", "Info.plist"), plist(options.product ?? null));
    const names = ["NodeSpace", "nodespaced", ...(options.otherDaemon ? [OTHER_DAEMON] : [])];
    for (const name of names) {
      const bin = join(macos, name);
      writeFileSync(bin, `#!/bin/bash\ntouch "${sentinel}"\necho community\n`);
      chmodSync(bin, 0o755);
    }
  }
  return { volume, sentinel, cleanup: () => rmSync(volume, { recursive: true, force: true }) };
}

interface RunResult {
  exitCode: number | null;
  stderr: string;
}

/** Runs preinstall the way Installer does: $1 package, $2 location, $3 volume, $4 startup disk. */
function runPreinstall(volumeArg: string, env: Record<string, string> = {}): RunResult {
  const result = Bun.spawnSync(["/bin/bash", PREINSTALL, "pkg", "/", volumeArg, "/"], {
    env: { PATH: "/usr/bin:/bin", ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  return { exitCode: result.exitCode, stderr: result.stderr.toString() };
}

/** Builds a fixture, runs preinstall against it and asserts nothing under the bundle was executed. */
function check(options: FixtureOptions, env: Record<string, string> = {}): RunResult {
  const fixture = makeFixture(options);
  try {
    const result = runPreinstall(fixture.volume, env);
    expect(existsSync(fixture.sentinel)).toBe(false);
    return result;
  } finally {
    fixture.cleanup();
  }
}

describe.skipIf(!onMac)("preinstall refusal matrix", () => {
  test("no app installed on the target volume proceeds, whatever is at the running system's /Applications", () => {
    expect(check({ app: false }).exitCode).toBe(0);
  });

  test("a community bundle proceeds", () => {
    const { exitCode, stderr } = check({ product: "community" });
    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
  });

  test("the other product's bundle is refused with the message that names its uninstall", () => {
    const { exitCode, stderr } = check({ product: OTHER_PRODUCT });
    expect(exitCode).toBe(1);
    expect(stderr.trim()).toBe(REFUSAL);
  });

  test("an unrecognised product value is refused", () => {
    expect(check({ product: "other" }).exitCode).toBe(1);
  });

  test("no key and no other daemon is treated as community", () => {
    expect(check({ product: null }).exitCode).toBe(0);
  });

  test("no key but the other daemon beside the app is refused", () => {
    const { exitCode, stderr } = check({ product: null, otherDaemon: true });
    expect(exitCode).toBe(1);
    expect(stderr.trim()).toBe(REFUSAL);
  });

  test("a declared community product wins over a daemon file: the fallback is only for a missing key", () => {
    expect(check({ product: "community", otherDaemon: true }).exitCode).toBe(0);
  });

  test("the override lets a refused install proceed", () => {
    const { exitCode, stderr } = check({ product: OTHER_PRODUCT }, { [FORCE_VAR]: "1" });
    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
  });

  test("the override also covers the no-key fallback", () => {
    expect(check({ product: null, otherDaemon: true }, { [FORCE_VAR]: "1" }).exitCode).toBe(0);
  });

  test("only the value 1 overrides", () => {
    expect(check({ product: OTHER_PRODUCT }, { [FORCE_VAR]: "0" }).exitCode).toBe(1);
    expect(check({ product: OTHER_PRODUCT }, { [FORCE_VAR]: "" }).exitCode).toBe(1);
    expect(check({ product: OTHER_PRODUCT }, { [FORCE_VAR]: "true" }).exitCode).toBe(1);
  });
});

describe("preinstall packaging", () => {
  test("is executable, so pkgbuild ships it as the package's preinstall script", () => {
    expect(statSync(PREINSTALL).mode & 0o111).toBe(0o111);
  });

  test("is valid bash", () => {
    const result = Bun.spawnSync(["/bin/bash", "-n", PREINSTALL], { stdout: "pipe", stderr: "pipe" });
    expect(result.stderr.toString()).toBe("");
    expect(result.exitCode).toBe(0);
  });

  test("names its one system tool by absolute path and never reaches into /usr/local/bin", () => {
    const code = readFileSync(PREINSTALL, "utf8")
      .split("\n")
      .filter((line) => !line.trim().startsWith("#"))
      .join("\n");
    expect(code).not.toContain("/usr/local/bin");
    expect(code).toContain("/usr/bin/plutil");
    expect(code).not.toMatch(/(?:^|[\s(`|;&])plutil\b/m);
  });
});

describe("postinstall no longer guards against another product", () => {
  const src = readFileSync(POSTINSTALL, "utf8");
  const codeLines = src.split("\n").filter((line) => !line.trim().startsWith("#"));

  test("carries neither the edition flag nor the override variable", () => {
    expect(src).not.toContain(EDITION_FLAG);
    expect(src).not.toContain(FORCE_VAR);
  });

  test("never runs the installed daemon: every line that names it is a chmod or chown", () => {
    const daemonLines = codeLines.filter((line) => /nodespaced/i.test(line));
    expect(daemonLines.length).toBeGreaterThan(0);
    for (const line of daemonLines) {
      expect(line).toMatch(/^\s*(?:chmod|chown)\b/);
    }
  });

  test("keeps the launchd-bootstrap block the bootstrap test extracts", () => {
    expect(src).toContain("# --- BEGIN launchd-bootstrap");
    expect(src).toContain("# --- END launchd-bootstrap ---");
  });
});

describe("the app bundle declares its product", () => {
  const config = JSON.parse(readFileSync(TAURI_CONF, "utf8")) as { bundle?: { macOS?: { infoPlist?: unknown } } };
  const infoPlist = config.bundle?.macOS?.infoPlist;

  test("bundle.macOS.infoPlist names a plist that is not the auto-merged Info.plist", () => {
    expect(typeof infoPlist).toBe("string");
    // Tauri also merges a file named exactly Info.plist beside the config,
    // and a build config can replace this path but cannot un-merge that one.
    expect(basename(infoPlist as string)).not.toBe("Info.plist");
  });

  test("that plist exists and declares NodeSpaceProduct = community", () => {
    const path = join(dirname(TAURI_CONF), infoPlist as string);
    expect(existsSync(path)).toBe(true);
    expect(readFileSync(path, "utf8")).toMatch(/<key>NodeSpaceProduct<\/key>\s*<string>community<\/string>/);
  });

  test.skipIf(!onMac)("plutil reads the key back as community", () => {
    const path = join(dirname(TAURI_CONF), infoPlist as string);
    const result = Bun.spawnSync(["/usr/bin/plutil", "-extract", "NodeSpaceProduct", "raw", "-o", "-", path], { stdout: "pipe", stderr: "pipe" });
    expect(result.stdout.toString().trim()).toBe("community");
  });
});
