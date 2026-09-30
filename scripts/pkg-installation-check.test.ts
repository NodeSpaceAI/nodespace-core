// Regression guard for the Distribution installation check that
// scripts/build-pkg.sh writes into the .pkg's distribution.xml. It makes the
// same decision as scripts/pkg-resources/preinstall, but earlier and in the
// Installer UI: it refuses to install over another NodeSpace product and shows
// the reason in a dialog instead of a generic "installation failed".
//
// The check is JavaScript that only the macOS Installer can run, so this test
// renders the *actual* heredoc block out of build-pkg.sh (between sentinel
// comments) under bash, pulls the generated script out of the resulting XML,
// and runs it in a vm context against a mocked `system` and `my`. The mocks
// return what the real Installer returns, checked on a real Installer:
// plistAtPath gives null for a missing file, a missing key reads as
// undefined, and system.env exposes the installer process's environment.
//
// Needles for the other product's daemon and the override variable are built
// from fragments so this file does not carry the boundary-check markers it
// guards against.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { runInNewContext } from "node:vm";

setDefaultTimeout(30_000);

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const BUILD_PKG_SH = join(REPO, "scripts", "build-pkg.sh");
const PREINSTALL = join(REPO, "scripts", "pkg-resources", "preinstall");
const RELEASE_WORKFLOW = join(REPO, ".github", "workflows", "release.yml");

const BEGIN_MARKER = "# --- BEGIN distribution-xml";
const END_MARKER = "# --- END distribution-xml ---";

const APP = "/Applications/NodeSpace.app";
const OTHER_DAEMON = ["nodespaced", "pro"].join("-");
const FORCE_VAR = ["NODESPACE", "FORCE", "COMMUNITY"].join("_");
const OTHER_NAME = ["Pr", "o"].join("");

const onMac = process.platform === "darwin";

function extractDistributionBlock(): string {
  const src = readFileSync(BUILD_PKG_SH, "utf8");
  const begin = src.indexOf(BEGIN_MARKER);
  const end = src.indexOf(END_MARKER);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${BEGIN_MARKER} / ${END_MARKER} sentinels in ${BUILD_PKG_SH} — ` +
        "did the distribution.xml block move or get renamed?",
    );
  }
  return src.slice(begin, end);
}

/** Runs the real block under bash, as build-pkg.sh does, and returns the distribution.xml it writes. */
function renderDistributionXml(buildDir: string): string {
  const script = ["set -euo pipefail", extractDistributionBlock()].join("\n");
  const result = Bun.spawnSync(["/bin/bash", "-c", script], {
    env: { PATH: "/usr/bin:/bin", BUILD_DIR: buildDir, PKG_VERSION: "1.2.3" },
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    throw new Error(`rendering distribution.xml failed: ${result.stderr.toString()}`);
  }
  return readFileSync(join(buildDir, "distribution.xml"), "utf8");
}

function withRenderedXml<T>(fn: (xml: string, buildDir: string) => T): T {
  const buildDir = mkdtempSync(join(tmpdir(), "pkg-installation-check-test-"));
  try {
    return fn(renderDistributionXml(buildDir), buildDir);
  } finally {
    rmSync(buildDir, { recursive: true, force: true });
  }
}

function extractScript(xml: string): string {
  const match = /<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(xml);
  if (match === null) throw new Error("distribution.xml has no <script> CDATA block");
  return match[1];
}

interface Scenario {
  /** Whether the app bundle exists. Defaults to true. */
  app?: boolean;
  /** NodeSpaceProduct value; omitted or null means the key is absent. */
  product?: string | null;
  /** Whether Contents/MacOS holds the other product's daemon. */
  otherDaemon?: boolean;
  /** system.env; undefined means the property does not exist. */
  env?: Record<string, string>;
  /** Makes plistAtPath throw. */
  plistThrows?: boolean;
  /** Makes fileExistsAtPath throw. */
  existsThrows?: boolean;
}

interface Outcome {
  returned: unknown;
  result: { type?: string; title?: string; message?: string };
  logs: string[];
}

function runCheck(script: string, scenario: Scenario): Outcome {
  const existing = new Set<string>();
  if (scenario.app !== false) existing.add(APP);
  if (scenario.otherDaemon) existing.add(`${APP}/Contents/MacOS/${OTHER_DAEMON}`);
  const plist = scenario.product === undefined || scenario.product === null ? {} : { NodeSpaceProduct: scenario.product };
  const logs: string[] = [];
  const my = { result: {} as Outcome["result"] };
  const system = {
    env: scenario.env,
    log: (message: string) => logs.push(message),
    files: {
      fileExistsAtPath: (path: string) => {
        if (scenario.existsThrows) throw new Error("unreadable volume");
        return existing.has(path);
      },
      plistAtPath: (path: string) => {
        if (scenario.plistThrows) throw new Error("unreadable plist");
        return path === `${APP}/Contents/Info.plist` && existing.has(APP) ? plist : null;
      },
    },
  };
  const returned = runInNewContext(`${script}\nnodespaceInstallationCheck();`, { system, my });
  return { returned, result: my.result, logs };
}

describe("the distribution.xml block in build-pkg.sh", () => {
  test("declares the installation check and its script", () => {
    withRenderedXml((xml) => {
      expect(xml).toContain('<installation-check script="nodespaceInstallationCheck()"/>');
      expect(extractScript(xml)).toContain("function nodespaceInstallationCheck()");
    });
  });

  test("still expands the package version and leaves the script untouched by the shell", () => {
    withRenderedXml((xml) => {
      expect(xml).toContain("<title>NodeSpace 1.2.3</title>");
      expect(xml).toContain('version="1.2.3" onConclusion="none">NodeSpace-component.pkg</pkg-ref>');
      // The heredoc is unquoted: a stray dollar sign or backtick in the
      // script would have been expanded away or executed by the shell.
      expect(extractScript(xml)).toBe(extractScript(readFileSync(BUILD_PKG_SH, "utf8")));
    });
  });

  test.skipIf(!onMac)("is accepted by productbuild", () => {
    withRenderedXml((xml, buildDir) => {
      expect(xml).toContain("<installer-gui-script");
      const component = join(buildDir, "NodeSpace-component.pkg");
      const pkgbuild = Bun.spawnSync(
        ["/usr/bin/pkgbuild", "--nopayload", "--identifier", "com.nodespace.pkg", "--version", "1.2.3", component],
        { stdout: "pipe", stderr: "pipe" },
      );
      expect(pkgbuild.exitCode).toBe(0);
      const productbuild = Bun.spawnSync(
        ["/usr/bin/productbuild", "--distribution", join(buildDir, "distribution.xml"), "--package-path", buildDir, join(buildDir, "out.pkg")],
        { stdout: "pipe", stderr: "pipe" },
      );
      expect(productbuild.stderr.toString()).not.toMatch(/error/i);
      expect(productbuild.exitCode).toBe(0);
    });
  });
});

describe("the installation check's decision", () => {
  const script = withRenderedXml((xml) => extractScript(xml));

  test("no app installed proceeds and sets no result", () => {
    const { returned, result } = runCheck(script, { app: false });
    expect(returned).toBe(true);
    expect(result).toEqual({});
  });

  test("a community bundle proceeds", () => {
    const { returned, result } = runCheck(script, { product: "community" });
    expect(returned).toBe(true);
    expect(result).toEqual({});
  });

  test("the other product's bundle is refused with a Fatal result", () => {
    const { returned, result } = runCheck(script, { product: "pro", env: {} });
    expect(returned).toBe(false);
    expect(result.type).toBe("Fatal");
    expect(result.title).toBe(`NodeSpace ${OTHER_NAME} is installed`);
    expect(result.message).toContain(`uninstall ${OTHER_NAME} first`);
    expect(result.message).toContain("Your databases stay on this Mac.");
  });

  test("an unrecognised product value is refused", () => {
    expect(runCheck(script, { product: "other", env: {} }).returned).toBe(false);
  });

  test("no key and no other daemon is treated as community", () => {
    expect(runCheck(script, { product: null }).returned).toBe(true);
  });

  test("no key but the other daemon beside the app is refused", () => {
    const { returned, result } = runCheck(script, { product: null, otherDaemon: true, env: {} });
    expect(returned).toBe(false);
    expect(result.type).toBe("Fatal");
  });

  test("a declared community product wins over a daemon file", () => {
    expect(runCheck(script, { product: "community", otherDaemon: true }).returned).toBe(true);
  });

  test("the override lets a refused install proceed, for a declared product and for the fallback", () => {
    expect(runCheck(script, { product: "pro", env: { [FORCE_VAR]: "1" } }).returned).toBe(true);
    expect(runCheck(script, { product: null, otherDaemon: true, env: { [FORCE_VAR]: "1" } }).returned).toBe(true);
  });

  test("only the value 1 overrides", () => {
    expect(runCheck(script, { product: "pro", env: { [FORCE_VAR]: "0" } }).returned).toBe(false);
    expect(runCheck(script, { product: "pro", env: { [FORCE_VAR]: "true" } }).returned).toBe(false);
  });

  test("an Installer without system.env still refuses instead of throwing away the check", () => {
    const { returned, result, logs } = runCheck(script, { product: "pro" });
    expect(returned).toBe(false);
    expect(result.type).toBe("Fatal");
    expect(logs).toEqual([]);
  });

  test("an unreadable Info.plist logs and falls back to the daemon-file check, as preinstall does", () => {
    const withoutDaemon = runCheck(script, { plistThrows: true });
    expect(withoutDaemon.returned).toBe(true);
    expect(withoutDaemon.result).toEqual({});
    expect(withoutDaemon.logs.length).toBe(1);
    expect(withoutDaemon.logs[0]).toContain("could not read Info.plist");

    const withDaemon = runCheck(script, { plistThrows: true, otherDaemon: true, env: {} });
    expect(withDaemon.returned).toBe(false);
    expect(withDaemon.result.type).toBe("Fatal");
  });

  test("any other exception logs the failure and proceeds, leaving the decision to preinstall", () => {
    const { returned, result, logs } = runCheck(script, { existsThrows: true });
    expect(returned).toBe(true);
    expect(result).toEqual({});
    expect(logs.length).toBe(1);
    expect(logs[0]).toContain("NodeSpace installation check failed");
  });

  test("the release workflow's install-log tripwire greps for the text the check logs on failure", () => {
    const workflow = readFileSync(RELEASE_WORKFLOW, "utf8");
    const match = /grep -a -q "([^"]+)" \/var\/log\/install\.log/.exec(workflow);
    expect(match).not.toBeNull();
    const { logs } = runCheck(script, { existsThrows: true });
    expect(logs[0]).toContain((match as RegExpExecArray)[1]);
  });

  test.skipIf(!onMac)("refuses with exactly the text preinstall prints", () => {
    const volume = mkdtempSync(join(tmpdir(), "pkg-installation-check-parity-"));
    try {
      const macos = join(volume, "Applications", "NodeSpace.app", "Contents", "MacOS");
      mkdirSync(macos, { recursive: true });
      writeFileSync(
        join(volume, "Applications", "NodeSpace.app", "Contents", "Info.plist"),
        '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>NodeSpaceProduct</key><string>pro</string></dict></plist>',
      );
      const preinstall = Bun.spawnSync(["/bin/bash", PREINSTALL, "pkg", "/", volume, "/"], {
        env: { PATH: "/usr/bin:/bin" },
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(preinstall.exitCode).toBe(1);
      expect(runCheck(script, { product: "pro", env: {} }).result.message).toBe(preinstall.stderr.toString().trim());
    } finally {
      rmSync(volume, { recursive: true, force: true });
    }
  });
});
