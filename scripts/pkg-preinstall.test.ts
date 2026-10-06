// Regression guard for the .pkg's pre-install check
// (scripts/pkg-resources/preinstall) and the static fact it depends on: the
// app bundle declares its product (NodeSpaceProduct in Info.plist, merged in by
// Tauri from the plist tauri.conf.json names).
//
// preinstall runs as root before anything is written and reads only static
// data, so these tests build fixture bundles under a temp "volume" and run the
// real script against them with a scrubbed environment. Every fixture bundle
// carries executables that write a sentinel file: the script must never run
// any of them.
//
// The script calls /usr/bin/plutil, so the behavioural tests skip off macOS.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

// Each case spawns bash and plutil: a correctness check, not a performance
// one, so the default 5s would only measure how busy the machine is.
setDefaultTimeout(30_000);

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const PREINSTALL = join(REPO, "scripts", "pkg-resources", "preinstall");
const TAURI_CONF = join(REPO, "packages", "desktop-app", "src-tauri", "tauri.conf.json");

// ADR-084 fixes this wording, character for character.
const REFUSAL =
  "The NodeSpace app on this Mac is a different NodeSpace product, or an older NodeSpace that does not say " +
  "which product it is. To replace it, move /Applications/NodeSpace.app to the Trash, then run this installer " +
  "again. Your databases stay on this Mac.";

const onMac = process.platform === "darwin";

interface FixtureOptions {
  /** Whether /Applications/NodeSpace.app exists at all. Defaults to true. */
  app?: boolean;
  /** The NodeSpaceProduct entry as raw plist XML; omitted means the key is absent. */
  product?: string;
  /** Write Info.plist in the binary format. */
  binaryPlist?: boolean;
  /** Leave Info.plist out of the bundle. */
  noInfoPlist?: boolean;
}

const string = (value: string) => `<string>${value}</string>`;

interface Fixture {
  /** The volume root handed to preinstall as $3. */
  volume: string;
  /** Created by any fixture executable that gets run. */
  sentinel: string;
  cleanup: () => void;
}

function makeFixture(options: FixtureOptions = {}): Fixture {
  const volume = mkdtempSync(join(tmpdir(), "pkg-preinstall-test-"));
  const sentinel = join(volume, "executable-was-run");
  if (options.app !== false) {
    const contents = join(volume, "Applications", "NodeSpace.app", "Contents");
    const macos = join(contents, "MacOS");
    mkdirSync(macos, { recursive: true });
    if (!options.noInfoPlist) {
      const key = options.product === undefined ? "" : `<key>NodeSpaceProduct</key>${options.product}`;
      const plist = join(contents, "Info.plist");
      writeFileSync(
        plist,
        `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleName</key><string>NodeSpace</string>${key}</dict></plist>
`,
      );
      if (options.binaryPlist) {
        expect(Bun.spawnSync(["/usr/bin/plutil", "-convert", "binary1", plist]).exitCode).toBe(0);
      }
    }
    // Every name the script might be tempted to run, plutil included.
    for (const name of ["NodeSpace", "nodespaced", "nodespace", "plutil"]) {
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
function check(options: FixtureOptions, env: Record<string, string> = {}, volumeSuffix = ""): RunResult {
  const fixture = makeFixture(options);
  try {
    const result = runPreinstall(fixture.volume + volumeSuffix, env);
    expect(existsSync(fixture.sentinel)).toBe(false);
    return result;
  } finally {
    fixture.cleanup();
  }
}

function expectRefused(result: RunResult) {
  expect(result.exitCode).toBe(1);
  expect(result.stderr.trim()).toBe(REFUSAL);
}

function expectProceeds(result: RunResult) {
  expect(result.exitCode).toBe(0);
  expect(result.stderr).toBe("");
}

describe.skipIf(!onMac)("preinstall refusal matrix", () => {
  test("no app on the target volume proceeds", () => {
    expectProceeds(check({ app: false }));
  });

  test("a bundle declaring community proceeds", () => {
    expectProceeds(check({ product: string("community") }));
  });

  test("a binary Info.plist declaring community proceeds", () => {
    expectProceeds(check({ product: string("community"), binaryPlist: true }));
  });

  test("a bundle declaring another product is refused with the decided message", () => {
    expectRefused(check({ product: string("other-product") }));
  });

  test("a binary Info.plist declaring another product is refused", () => {
    expectRefused(check({ product: string("other-product"), binaryPlist: true }));
  });

  test("a bundle without the key is refused: it cannot say which product it is", () => {
    expectRefused(check({}));
  });

  test("an empty or non-string product is refused", () => {
    expectRefused(check({ product: string("") }));
    expectRefused(check({ product: "<true/>" }));
  });

  test("a bundle without an Info.plist is refused", () => {
    expectRefused(check({ noInfoPlist: true }));
  });

  test("the target volume decides which bundle is read, with or without a trailing slash", () => {
    expectRefused(check({ product: string("other-product") }, {}, "/"));
    expectProceeds(check({ product: string("community") }, {}, "/"));
  });

  test("the script reads nothing from its environment, so no variable lets a refused install proceed", () => {
    const env = { NODESPACE_FORCE: "1", FORCE: "1", NODESPACE_PRODUCT: "community" };
    expectRefused(check({ product: string("other-product") }, env));
    expectRefused(check({}, env));
    // Every expansion in the script's code is its own variable or argument.
    const code = readFileSync(PREINSTALL, "utf8")
      .split("\n")
      .filter((line) => !line.trim().startsWith("#"))
      .join("\n");
    const expansions = [...code.matchAll(/\$\{?([A-Za-z_0-9]+)/g)].map((m) => m[1]);
    expect([...new Set(expansions)].sort()).toEqual(["3", "APP", "VOLUME", "product"]);
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

  test("calls its one system tool by absolute path and never reaches into /usr/local/bin", () => {
    const code = readFileSync(PREINSTALL, "utf8")
      .split("\n")
      .filter((line) => !line.trim().startsWith("#"))
      .join("\n");
    expect(code).not.toContain("/usr/local/bin");
    expect(code).toContain("/usr/bin/plutil");
    expect(code).not.toMatch(/(?:^|[\s(`|;&])plutil\b/m);
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
    expect(existsSync(join(dirname(TAURI_CONF), "Info.plist"))).toBe(false);
  });

  test("that plist declares NodeSpaceProduct = community", () => {
    const path = join(dirname(TAURI_CONF), infoPlist as string);
    expect(readFileSync(path, "utf8")).toMatch(/<key>NodeSpaceProduct<\/key>\s*<string>community<\/string>/);
  });

  test.skipIf(!onMac)("plutil reads the key back as community, as preinstall does", () => {
    const path = join(dirname(TAURI_CONF), infoPlist as string);
    const result = Bun.spawnSync(["/usr/bin/plutil", "-extract", "NodeSpaceProduct", "raw", "-o", "-", path], {
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(result.exitCode).toBe(0);
    expect(result.stdout.toString().trim()).toBe("community");
  });
});
