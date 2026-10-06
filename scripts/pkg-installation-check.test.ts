// Regression guard for the Distribution installation check that
// scripts/build-pkg.sh writes into the .pkg's distribution.xml. It makes the
// same decision as scripts/pkg-resources/preinstall, but earlier and in the
// Installer window: it refuses to install over another NodeSpace product and
// shows why in a dialog instead of a generic "installation failed".
//
// The check is JavaScript that only the macOS Installer runs, so this test
// renders the actual heredoc block out of build-pkg.sh (between sentinel
// comments) under bash, pulls the generated script out of the resulting XML,
// and runs it in a vm context against a mocked `system` and `my`, shaped like
// Installer's: plistAtPath gives null for a missing file, a missing key reads
// as undefined, and system.env holds the installer process's environment,
// which the check does not consult.
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

// ADR-084 fixes this wording, character for character.
const REFUSAL =
  "The NodeSpace app on this Mac is a different NodeSpace product, or an older NodeSpace that does not say " +
  "which product it is. To replace it, move /Applications/NodeSpace.app to the Trash, then run this installer " +
  "again. Your databases stay on this Mac.";

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

/** The install-log tripwire from release.yml: the `if grep ... /var/log/install.log ...; then ... fi` block, dedented. */
function tripwireBlock(): string {
  const workflow = readFileSync(RELEASE_WORKFLOW, "utf8");
  const match = /^( *)if grep [^\n]*\/var\/log\/install\.log[^\n]*; then\n[\s\S]*?\n\1fi\n/m.exec(workflow);
  if (match === null) throw new Error(`could not find the install-log tripwire in ${RELEASE_WORKFLOW}`);
  return match[0]
    .split("\n")
    .map((line) => (line.startsWith(match[1]) ? line.slice(match[1].length) : line))
    .join("\n");
}

interface Scenario {
  /** Whether the app bundle exists. Defaults to true. */
  app?: boolean;
  /** What plistAtPath returns for the bundle's Info.plist. Defaults to an empty dictionary. */
  info?: Record<string, unknown> | null;
  /** system.env; undefined means the property does not exist. */
  env?: Record<string, string>;
  /** Makes plistAtPath throw. */
  plistThrows?: boolean;
}

interface Outcome {
  returned: unknown;
  result: { type?: string; title?: string; message?: string };
  logs: string[];
}

function runCheck(script: string, scenario: Scenario): Outcome {
  const appExists = scenario.app !== false;
  const info = scenario.info === undefined ? {} : scenario.info;
  const logs: string[] = [];
  const my = { result: {} as Outcome["result"] };
  const system = {
    env: scenario.env,
    log: (message: string) => logs.push(message),
    files: {
      fileExistsAtPath: (path: string) => appExists && path === APP,
      plistAtPath: (path: string) => {
        if (scenario.plistThrows) throw new Error("unreadable plist");
        return appExists && path === `${APP}/Contents/Info.plist` ? info : null;
      },
    },
  };
  const returned = runInNewContext(`${script}\nnodespaceInstallationCheck();`, { system, my });
  return { returned, result: my.result, logs };
}

function expectRefused(outcome: Outcome) {
  expect(outcome.returned).toBe(false);
  expect(outcome.result.type).toBe("Fatal");
  expect(outcome.result.message).toBe(REFUSAL);
}

function expectProceeds(outcome: Outcome) {
  expect(outcome.returned).toBe(true);
  expect(outcome.result).toEqual({});
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
    withRenderedXml((_xml, buildDir) => {
      const component = join(buildDir, "NodeSpace-component.pkg");
      const pkgbuild = Bun.spawnSync(
        ["/usr/bin/pkgbuild", "--nopayload", "--identifier", "com.nodespace.pkg", "--version", "1.2.3", component],
        { stdout: "pipe", stderr: "pipe" },
      );
      expect(pkgbuild.exitCode).toBe(0);
      const productbuild = Bun.spawnSync(
        [
          "/usr/bin/productbuild",
          "--distribution",
          join(buildDir, "distribution.xml"),
          "--package-path",
          buildDir,
          join(buildDir, "out.pkg"),
        ],
        { stdout: "pipe", stderr: "pipe" },
      );
      expect(productbuild.stderr.toString()).not.toMatch(/error/i);
      expect(productbuild.exitCode).toBe(0);
    });
  });
});

describe("the installation check's decision", () => {
  const script = withRenderedXml((xml) => extractScript(xml));

  test("no app installed proceeds", () => {
    expectProceeds(runCheck(script, { app: false, env: {} }));
  });

  test("a bundle declaring community proceeds", () => {
    expectProceeds(runCheck(script, { info: { NodeSpaceProduct: "community" }, env: {} }));
  });

  test("a bundle declaring another product is refused with the decided message", () => {
    const outcome = runCheck(script, { info: { NodeSpaceProduct: "other-product" }, env: {} });
    expectRefused(outcome);
    expect(outcome.result.title).toBe("This installer does not recognise the installed NodeSpace");
  });

  test("a bundle without the key is refused: it cannot say which product it is", () => {
    expectRefused(runCheck(script, { info: { CFBundleName: "NodeSpace" }, env: {} }));
  });

  test("an empty or non-string product is refused", () => {
    expectRefused(runCheck(script, { info: { NodeSpaceProduct: "" }, env: {} }));
    expectRefused(runCheck(script, { info: { NodeSpaceProduct: true }, env: {} }));
  });

  test("an Info.plist Installer cannot read (null) is refused", () => {
    expectRefused(runCheck(script, { info: null, env: {} }));
  });

  test("nothing in the installer's environment lets a refused install proceed", () => {
    const env = { NODESPACE_FORCE: "1", FORCE: "1", NODESPACE_PRODUCT: "community" };
    expectRefused(runCheck(script, { info: { NodeSpaceProduct: "other-product" }, env }));
    expectRefused(runCheck(script, { info: null, env }));
    expect(script).not.toContain("system.env");
  });

  test("an Installer without system.env still refuses instead of throwing the check away", () => {
    const outcome = runCheck(script, { info: { NodeSpaceProduct: "other-product" } });
    expectRefused(outcome);
    expect(outcome.logs).toEqual([]);
  });

  test("a check that throws logs the failure and leaves the decision to preinstall", () => {
    const outcome = runCheck(script, { plistThrows: true, env: {} });
    expectProceeds(outcome);
    expect(outcome.logs.length).toBe(1);
    expect(outcome.logs[0]).toContain("NodeSpace installation check failed");
  });

  test("the release workflow's install-log tripwire fails on the line the check logs, and passes otherwise", () => {
    const dir = mkdtempSync(join(tmpdir(), "pkg-installation-check-tripwire-"));
    try {
      const runTripwire = (logContents: string | null) => {
        const logPath = join(dir, "install.log");
        rmSync(logPath, { force: true });
        if (logContents !== null) writeFileSync(logPath, logContents);
        // Run it the way `shell: bash` does in the workflow.
        const result = Bun.spawnSync(
          [
            "/bin/bash",
            "--noprofile",
            "--norc",
            "-eo",
            "pipefail",
            "-c",
            tripwireBlock().replaceAll("/var/log/install.log", logPath),
          ],
          { env: { PATH: "/usr/bin:/bin" }, stdout: "pipe", stderr: "pipe" },
        );
        return { exitCode: result.exitCode, stdout: result.stdout.toString() };
      };
      const installerLine = (message: string) => `Oct  5 12:00:00 runner installer[123]: JS: ${message}\n`;

      expect(runTripwire(installerLine("unrelated line")).exitCode).toBe(0);
      expect(runTripwire(null).exitCode).toBe(0);

      const { logs } = runCheck(script, { plistThrows: true, env: {} });
      expect(logs.length).toBe(1);
      const tripped = runTripwire(installerLine(logs[0]));
      expect(tripped.exitCode).toBe(1);
      expect(tripped.stdout).toContain("::error::");
      expect(tripped.stdout).toContain(logs[0]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test.skipIf(!onMac)("refuses with exactly the text preinstall prints", () => {
    const volume = mkdtempSync(join(tmpdir(), "pkg-installation-check-parity-"));
    try {
      const contents = join(volume, "Applications", "NodeSpace.app", "Contents");
      mkdirSync(contents, { recursive: true });
      writeFileSync(
        join(contents, "Info.plist"),
        '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>NodeSpaceProduct</key><string>other-product</string></dict></plist>',
      );
      const preinstall = Bun.spawnSync(["/bin/bash", PREINSTALL, "pkg", "/", volume, "/"], {
        env: { PATH: "/usr/bin:/bin" },
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(preinstall.exitCode).toBe(1);
      const outcome = runCheck(script, { info: { NodeSpaceProduct: "other-product" }, env: {} });
      expect(outcome.result.message).toBe(preinstall.stderr.toString().trim());
    } finally {
      rmSync(volume, { recursive: true, force: true });
    }
  });
});
