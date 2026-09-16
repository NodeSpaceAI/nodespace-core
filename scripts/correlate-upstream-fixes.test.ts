// Covers the pre-push gate's failure/upstream correlation: when a stage
// fails and origin/main already carries a commit touching that code, the
// failure may be stale code rather than a live regression. These tests
// inject a fake `git log` so they exercise the parsing, scoping and
// formatting without touching the real repo or network.
import { describe, expect, test } from "bun:test";
import {
  candidatePathsFor,
  correlateUpstreamFixes,
  formatCorrelationWarning,
  packageScopeFor,
  packageScopesForPaths,
  parseCommitLog,
  parseFailingPaths,
  reportUpstreamFixes,
  scopesForPaths,
  vitestPackagePrefixes,
} from "./correlate-upstream-fixes";

// Captured from real vitest reporter output (see the issue's probe run),
// not hand-written from memory.
const VITEST_FAILURE = `
stdout | src/tests/unit/zzz.test.ts
 ❯ src/tests/e2e/watch-nodes.e2e.ts (4 tests | 1 failed) 5002ms
⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯
 FAIL  src/tests/e2e/watch-nodes.e2e.ts > WatchNodes SSE stream > delivers nodeCreated event when a node is written
 ❯ src/tests/e2e/watch-nodes.e2e.ts:5:15
 Test Files  1 failed | 242 passed (243)
`;

const CARGO_FAILURE = `
error[E0425]: cannot find value \`foo\` in this scope
  --> packages/core/src/db/mod.rs:12:9
   |
12 |         foo();
`;

// Captured from a real `cargo test` run (a deliberately failing assertion),
// including the thread id cargo prints. A test that BUILDS and then fails
// emits this and no \`-->\` line at all, so it is a distinct shape from the
// compiler output above rather than a variant of it.
const CARGO_PANIC = `
running 1 test
test zzz_panic_probe::probe ... FAILED

failures:

---- zzz_panic_probe::probe stdout ----

thread 'zzz_panic_probe::probe' (36545869) panicked at packages/core/src/lib.rs:57:18:
assertion \`left == right\` failed
  left: 1
 right: 2
`;

describe("parseFailingPaths", () => {
  test("extracts the failing test file from vitest output, de-duplicated across shapes", () => {
    // The same path appears three times (❯ summary, FAIL line, stack frame).
    expect(parseFailingPaths(VITEST_FAILURE)).toEqual(["src/tests/e2e/watch-nodes.e2e.ts"]);
  });

  test("extracts a Rust source path from a cargo diagnostic", () => {
    expect(parseFailingPaths(CARGO_FAILURE)).toEqual(["packages/core/src/db/mod.rs"]);
  });

  test("extracts the panicking file from a cargo test assertion failure", () => {
    // Two of the gate's stages run cargo tests. A test that compiles and
    // then fails an assertion emits `panicked at` and never `-->`, so
    // without this the check stays silent for the most common Rust failure.
    expect(parseFailingPaths(CARGO_PANIC)).toEqual(["packages/core/src/lib.rs"]);
  });

  test("stops the panic path at the trailing line:col rather than swallowing it", () => {
    const paths = parseFailingPaths(CARGO_PANIC);
    expect(paths[0]).not.toContain(":57");
    expect(paths[0]?.endsWith(".rs")).toBe(true);
  });

  test("returns nothing for output that names no failing file", () => {
    expect(parseFailingPaths("Finished dev profile in 1m 00s")).toEqual([]);
  });

  test("returns nothing for empty output rather than throwing", () => {
    expect(parseFailingPaths("")).toEqual([]);
  });

  test("returns the same paths when called repeatedly on the same output", () => {
    // The patterns are module-level and /g, so this guards against state
    // leaking between calls. matchAll clones the regex so it does not today,
    // but an edit to an exec()/test() loop would reintroduce the hazard and
    // this is what would catch it.
    const first = parseFailingPaths(VITEST_FAILURE);
    const second = parseFailingPaths(VITEST_FAILURE);
    expect(second).toEqual(first);
    expect(second.length).toBeGreaterThan(0);
  });
});

describe("packageScopeFor", () => {
  test("widens a path inside packages/ to its package directory", () => {
    expect(packageScopeFor("packages/dev-tools/src/dev-proxy.ts")).toBe("packages/dev-tools");
  });

  test("falls back to the top-level directory outside packages/", () => {
    expect(packageScopeFor("scripts/test-gate.ts")).toBe("scripts");
  });

  test("returns null for a bare filename with no directory to widen to", () => {
    expect(packageScopeFor("README.md")).toBeNull();
  });
});

describe("vitestPackagePrefixes", () => {
  test("uses the packages discovered on disk", () => {
    expect(vitestPackagePrefixes(() => ["packages/a", "packages/b"])).toEqual([
      "packages/a",
      "packages/b",
    ]);
  });

  test("falls back to the known packages when the scan finds nothing", () => {
    // An empty result means the scan ran but matched nothing (a moved
    // directory, a sandbox). Returning [] would silence the check entirely.
    expect(vitestPackagePrefixes(() => [])).toContain("packages/desktop-app");
  });

  test("falls back, rather than throwing, when the directory cannot be read", () => {
    expect(
      vitestPackagePrefixes(() => {
        throw new Error("EACCES");
      })
    ).toContain("packages/desktop-app");
  });

  test("discovers the real repo's vitest packages", () => {
    // Guards the derivation itself: if this stops finding desktop-app, the
    // package-relative path resolution below silently stops working.
    expect(vitestPackagePrefixes()).toContain("packages/desktop-app");
  });
});

describe("candidatePathsFor", () => {
  test("leaves an already repo-relative path alone", () => {
    expect(candidatePathsFor("packages/core/src/db/mod.rs")).toEqual([
      "packages/core/src/db/mod.rs",
    ]);
  });

  test("leaves a scripts/ path alone", () => {
    expect(candidatePathsFor("scripts/test-gate.ts")).toEqual(["scripts/test-gate.ts"]);
  });

  test("prefixes a package-relative vitest path with each candidate package", () => {
    // vitest runs with --cwd packages/<pkg>, so it prints `src/tests/...`
    // while git needs `packages/<pkg>/src/tests/...`. Without this the
    // pathspec matches nothing and the warning silently never fires.
    const candidates = candidatePathsFor("src/tests/e2e/watch-nodes.e2e.ts", ["packages/desktop-app"]);
    expect(candidates).toContain("packages/desktop-app/src/tests/e2e/watch-nodes.e2e.ts");
    expect(candidates.every((c) => c.startsWith("packages/"))).toBe(true);
  });
});

describe("scopesForPaths", () => {
  test("yields the exact failing file, without widening to its package", () => {
    // Package widening is deliberately NOT part of the exact scope: on this
    // repo a package-level match hit 11 of 28 commits in one window, versus
    // 1 for the exact file. Mixing them would drown the strong signal.
    expect(scopesForPaths(["packages/desktop-app/src/tests/e2e/watch-nodes.e2e.ts"])).toEqual([
      "packages/desktop-app/src/tests/e2e/watch-nodes.e2e.ts",
    ]);
  });

  test("resolves a package-relative vitest path to a repo-relative scope", () => {
    expect(scopesForPaths(["src/tests/e2e/watch-nodes.e2e.ts"])).toContain(
      "packages/desktop-app/src/tests/e2e/watch-nodes.e2e.ts"
    );
  });
});

describe("packageScopesForPaths", () => {
  test("yields the containing package as the fallback scope", () => {
    expect(packageScopesForPaths(["packages/core/src/db/mod.rs"])).toEqual(["packages/core"]);
  });

  test("de-duplicates the package scope shared by two failing files", () => {
    const scopes = packageScopesForPaths(["packages/core/src/a.rs", "packages/core/src/b.rs"]);
    expect(scopes).toEqual(["packages/core"]);
  });
});

describe("parseCommitLog", () => {
  test("splits sha and subject, preserving spaces in the subject", () => {
    expect(parseCommitLog("96aee026 Gate SSE connected on the bridge")).toEqual([
      { sha: "96aee026", subject: "Gate SSE connected on the bridge" },
    ]);
  });

  test("ignores blank lines", () => {
    expect(parseCommitLog("\nabc123 One\n\ndef456 Two\n")).toHaveLength(2);
  });

  test("keeps a subject-less line rather than dropping it, so counts stay honest", () => {
    expect(parseCommitLog("abc123")).toEqual([{ sha: "abc123", subject: "" }]);
  });
});

describe("correlateUpstreamFixes", () => {
  test("reports an exact-file match as high confidence, without consulting the package", async () => {
    let calls = 0;
    const result = await correlateUpstreamFixes(VITEST_FAILURE, {
      logUpstreamTouching: async () => {
        calls += 1;
        return "96aee026 Gate SSE connected on the daemon watch bridge";
      },
    });
    expect(result.commits).toHaveLength(1);
    expect(result.matchScope).toBe("exact");
    // The package query is a fallback — a hit on the exact file must not
    // pay for it, nor dilute the result with package-level noise.
    expect(calls).toBe(1);
  });

  test("falls back to a package match, labelled as the weaker signal (the cross-file shape)", async () => {
    // The motivating case: the failing test was watch-nodes.e2e.ts while the
    // fix was in dev-proxy.ts. The exact file matches nothing, so only the
    // package-level fallback connects them — at lower confidence.
    let call = 0;
    const result = await correlateUpstreamFixes(VITEST_FAILURE, {
      logUpstreamTouching: async () => {
        call += 1;
        return call === 1 ? "" : "96aee026 Gate SSE on the daemon watch bridge";
      },
    });
    expect(result.commits).toHaveLength(1);
    expect(result.matchScope).toBe("package");
  });

  test("reports no commits, and no scope, when neither the file nor its package matches", async () => {
    const result = await correlateUpstreamFixes(VITEST_FAILURE, {
      logUpstreamTouching: async () => "",
    });
    expect(result.commits).toEqual([]);
    expect(result.matchScope).toBeUndefined();
    expect(result.reason).toBeUndefined();
  });

  test("skips the git call entirely when no failing path was parsed", async () => {
    let called = false;
    const result = await correlateUpstreamFixes("no file names here", {
      logUpstreamTouching: async () => {
        called = true;
        return "";
      },
    });
    expect(called).toBe(false);
    expect(result.commits).toEqual([]);
  });

  test("degrades to a reason, never throws, when git log fails", async () => {
    const result = await correlateUpstreamFixes(VITEST_FAILURE, {
      logUpstreamTouching: async () => {
        throw new Error("unknown revision: origin/main");
      },
    });
    expect(result.commits).toEqual([]);
    expect(result.reason).toContain("unknown revision");
  });
});

describe("formatCorrelationWarning", () => {
  test("names the commits and points at a rebase", () => {
    const warning = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits: [{ sha: "96aee026abcdef", subject: "Gate SSE on the bridge" }],
      matchScope: "exact",
    });
    expect(warning).toContain("96aee026");
    expect(warning).toContain("Gate SSE on the bridge");
    expect(warning).toContain("git rebase origin/main");
    expect(warning).toContain("1 commit on origin/main touches");
  });

  test("agrees in number and pronoun across singular and plural", () => {
    const one = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits: [{ sha: "aaaaaaaa", subject: "One" }],
      matchScope: "exact",
    });
    expect(one).toContain("1 commit on origin/main touches");
    expect(one).toContain("does not contain it:");

    const two = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits: [
        { sha: "aaaaaaaa", subject: "One" },
        { sha: "bbbbbbbb", subject: "Two" },
      ],
      matchScope: "exact",
    });
    expect(two).toContain("2 commits on origin/main touch");
    expect(two).toContain("does not contain them:");
  });

  test("phrases an exact match as likely, and a package match as a weak hint", () => {
    const commits = [{ sha: "aaaaaaaa", subject: "One" }];
    const exact = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits,
      matchScope: "exact",
    });
    expect(exact).toContain("the failing file itself");
    expect(exact).toContain("likely already fixed upstream");

    const pkg = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits,
      matchScope: "package",
    });
    expect(pkg).toContain("not the file itself");
    expect(pkg).toContain("weak hint, not a diagnosis");
    // The weaker signal must never claim the stronger one's conclusion.
    expect(pkg).not.toContain("likely already fixed upstream");
  });

  test("is empty when there is nothing to report, so the common path stays silent", () => {
    expect(formatCorrelationWarning({ failingPaths: ["a.test.ts"], commits: [] })).toBe("");
  });

  test("caps a long list so a stale branch cannot flood the gate output", () => {
    const commits = Array.from({ length: 9 }, (_, i) => ({
      sha: `sha${i}0000000`,
      subject: `Commit ${i}`,
    }));
    const warning = formatCorrelationWarning({
      failingPaths: ["a.test.ts"],
      commits,
      matchScope: "exact",
    });
    expect(warning).toContain("9 commits on origin/main");
    expect(warning).toContain("... and 4 more");
    expect(warning).not.toContain("Commit 8");
  });
});

describe("reportUpstreamFixes", () => {
  test("returns the correlation result and never throws on a git failure", async () => {
    const result = await reportUpstreamFixes(VITEST_FAILURE, {
      logUpstreamTouching: async () => {
        throw new Error("no upstream configured");
      },
    });
    expect(result.commits).toEqual([]);
    expect(result.reason).toContain("no upstream configured");
  });
});
