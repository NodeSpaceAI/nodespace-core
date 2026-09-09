// Covers the freshness/idempotency helpers in scripts/build-skill.ts against
// synthetic trees. The script's own top-level build is behind
// `import.meta.main`, so importing it here runs nothing -- these exercise the
// pure filesystem logic only, no `tsc` and no 58MB `bun build --compile`.
//
// What the helpers exist to protect: everything build-skill.ts stages is a
// `cargo:rerun-if-changed` input of nodespace-app's build script, so a
// rewritten byte (or merely a fresher mtime on identical content) forces a
// full crate rebuild on every `dev:tauri`. See the script's module doc.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
  compileInputs,
  compileInstaller,
  isNotACompileInput,
  isOutputFresh,
  listFilesRecursive,
  newestMtimeMs,
  pruneEmptyDirs,
  STAGED_ENTRIES,
  syncTreeByContent,
} from "./build-skill";

let root: string;

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "build-skill-test-"));
});

afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});

/** Writes `content` to `root/rel`, creating parent directories as needed. */
function write(rel: string, content: string): string {
  const abs = join(root, rel);
  mkdirSync(dirname(abs), { recursive: true });
  writeFileSync(abs, content);
  return abs;
}

/** Stamps an absolute path's mtime to a fixed epoch-seconds value. */
function setMtime(abs: string, seconds: number): void {
  utimesSync(abs, seconds, seconds);
}

describe("listFilesRecursive", () => {
  test("returns every nested file relative to the root", () => {
    write("tree/a.txt", "a");
    write("tree/nested/b.txt", "b");
    write("tree/nested/deeper/c.txt", "c");

    expect(listFilesRecursive(join(root, "tree")).sort()).toEqual([
      "a.txt",
      join("nested", "b.txt"),
      join("nested", "deeper", "c.txt"),
    ].sort());
  });

  test("returns nothing for a path that does not exist", () => {
    expect(listFilesRecursive(join(root, "absent"))).toEqual([]);
  });

  test("returns the single empty relative path for a plain file", () => {
    // Lets a plain file (SKILL.md, package.json) feed the same sync loop as a
    // directory tree without a separate branch at the call site.
    write("SKILL.md", "# skill");
    expect(listFilesRecursive(join(root, "SKILL.md"))).toEqual([""]);
  });
});

describe("newestMtimeMs", () => {
  test("returns the newest mtime across mixed files and trees", () => {
    const older = write("tree/old.txt", "old");
    const newer = write("tree/nested/new.txt", "new");
    const loose = write("loose.txt", "loose");
    setMtime(older, 1_000);
    setMtime(newer, 3_000);
    setMtime(loose, 2_000);

    expect(newestMtimeMs([join(root, "tree"), join(root, "loose.txt")])).toBe(3_000_000);
  });

  test("returns null when none of the paths exist", () => {
    expect(newestMtimeMs([join(root, "absent"), join(root, "also-absent")])).toBeNull();
  });

  test("skips missing paths rather than erroring", () => {
    const present = write("present.txt", "here");
    setMtime(present, 1_500);

    expect(newestMtimeMs([join(root, "absent"), join(root, "present.txt")])).toBe(1_500_000);
  });
});

describe("isOutputFresh", () => {
  test("is false when the output does not exist", () => {
    write("src/install.ts", "export {};");
    expect(isOutputFresh(join(root, "out.bin"), [join(root, "src")])).toBe(false);
  });

  test("is true when the output is newer than every input", () => {
    const input = write("src/install.ts", "export {};");
    const output = write("out.bin", "compiled");
    setMtime(input, 1_000);
    setMtime(output, 2_000);

    expect(isOutputFresh(output, [join(root, "src")])).toBe(true);
  });

  test("is true when the output ties the newest input exactly", () => {
    // Coarse filesystem timestamps can make a legitimately-fresh output land
    // on the same tick as its input; treating a tie as stale would recompile
    // 58MB on every run on such a filesystem.
    const input = write("src/install.ts", "export {};");
    const output = write("out.bin", "compiled");
    setMtime(input, 1_000);
    setMtime(output, 1_000);

    expect(isOutputFresh(output, [join(root, "src")])).toBe(true);
  });

  test("is false when any single nested input is newer than the output", () => {
    const untouched = write("src/install.ts", "export {};");
    const edited = write("src/nested/agents.ts", "export const AGENTS = {};");
    const output = write("out.bin", "compiled");
    setMtime(untouched, 1_000);
    setMtime(output, 2_000);
    setMtime(edited, 3_000);

    expect(isOutputFresh(output, [join(root, "src")])).toBe(false);
  });

  test("is true when the output exists but no input does", () => {
    const output = write("out.bin", "compiled");
    setMtime(output, 1_000);

    expect(isOutputFresh(output, [join(root, "absent")])).toBe(true);
  });

  test("stays fresh when the only newer input is one the ignore predicate drops", () => {
    const source = write("src/install.ts", "export {};");
    const skillTest = write("src/tests/install.test.ts", "test();");
    const output = write("out.bin", "compiled");
    setMtime(source, 1_000);
    setMtime(output, 2_000);
    setMtime(skillTest, 3_000);

    expect(isOutputFresh(output, [join(root, "src")])).toBe(false);
    expect(isOutputFresh(output, [join(root, "src")], isNotACompileInput)).toBe(true);
  });
});

describe("isNotACompileInput", () => {
  test("drops src/tests, which install.ts never reaches", () => {
    expect(isNotACompileInput("tests")).toBe(true);
    expect(isNotACompileInput(join("tests", "install.test.ts"))).toBe(true);
    expect(isNotACompileInput(join("tests", "nested", "deep.test.ts"))).toBe(true);
  });

  test("keeps the sources that are bundled", () => {
    expect(isNotACompileInput("install.ts")).toBe(false);
    expect(isNotACompileInput("installer.ts")).toBe(false);
    expect(isNotACompileInput("agents.ts")).toBe(false);
  });

  test("does not drop a non-test path that merely starts with the word", () => {
    expect(isNotACompileInput("tests-helper.ts")).toBe(false);
  });
});

describe("syncTreeByContent", () => {
  test("copies a tree that has never been staged", () => {
    write("src/a.txt", "a");
    write("src/nested/b.txt", "b");

    const changed = syncTreeByContent(join(root, "src"), join(root, "dest"));

    expect(changed).toBe(2);
    expect(readFileSync(join(root, "dest", "a.txt"), "utf8")).toBe("a");
    expect(readFileSync(join(root, "dest", "nested", "b.txt"), "utf8")).toBe("b");
  });

  test("rewrites nothing and preserves mtimes when content is identical", () => {
    // The central guarantee: `tsc` rewrites dist/*.js every run with
    // byte-identical output, so a mtime-based staging check would churn. A
    // preserved dest mtime is what keeps nodespace-app's build script valid.
    write("src/a.txt", "a");
    write("src/nested/b.txt", "b");
    syncTreeByContent(join(root, "src"), join(root, "dest"));

    const before = listFilesRecursive(join(root, "dest")).map((rel) =>
      statSync(join(root, "dest", rel)).mtimeMs,
    );
    // Re-stamp the sources newer, as a fresh `tsc` run would.
    setMtime(join(root, "src", "a.txt"), 9_000);
    setMtime(join(root, "src", "nested", "b.txt"), 9_000);

    const changed = syncTreeByContent(join(root, "src"), join(root, "dest"));

    expect(changed).toBe(0);
    const after = listFilesRecursive(join(root, "dest")).map((rel) =>
      statSync(join(root, "dest", rel)).mtimeMs,
    );
    expect(after).toEqual(before);
  });

  test("rewrites only the file whose content actually changed", () => {
    write("src/a.txt", "a");
    write("src/b.txt", "b");
    syncTreeByContent(join(root, "src"), join(root, "dest"));
    const untouchedBefore = statSync(join(root, "dest", "b.txt")).mtimeMs;

    write("src/a.txt", "a-edited");
    const changed = syncTreeByContent(join(root, "src"), join(root, "dest"));

    expect(changed).toBe(1);
    expect(readFileSync(join(root, "dest", "a.txt"), "utf8")).toBe("a-edited");
    expect(statSync(join(root, "dest", "b.txt")).mtimeMs).toBe(untouchedBefore);
  });

  test("deletes a staged file that no longer exists in the source", () => {
    write("src/keep.txt", "keep");
    write("src/gone.txt", "gone");
    syncTreeByContent(join(root, "src"), join(root, "dest"));

    rmSync(join(root, "src", "gone.txt"));
    const changed = syncTreeByContent(join(root, "src"), join(root, "dest"));

    expect(changed).toBe(1);
    expect(existsSync(join(root, "dest", "gone.txt"))).toBe(false);
    expect(existsSync(join(root, "dest", "keep.txt"))).toBe(true);
  });

  test("self-heals when a staged directory is now a plain file in the source", () => {
    // Byte-comparing a directory throws EISDIR. Clearing the flipped path and
    // rebuilding it keeps the staging self-correcting rather than wedging on
    // an error whose remedy isn't obvious from the message.
    mkdirSync(join(root, "dest", "SKILL.md"), { recursive: true });
    writeFileSync(join(root, "dest", "SKILL.md", "stray.txt"), "stale");
    write("SKILL.md", "# skill");

    const changed = syncTreeByContent(join(root, "SKILL.md"), join(root, "dest", "SKILL.md"));

    expect(changed).toBe(1);
    expect(statSync(join(root, "dest", "SKILL.md")).isFile()).toBe(true);
    expect(readFileSync(join(root, "dest", "SKILL.md"), "utf8")).toBe("# skill");
  });

  test("self-heals when a staged file is now a directory in the source", () => {
    write("dest/refs", "was a file");
    write("refs/cli.md", "# cli");

    const changed = syncTreeByContent(join(root, "refs"), join(root, "dest", "refs"));

    expect(changed).toBe(1);
    expect(readFileSync(join(root, "dest", "refs", "cli.md"), "utf8")).toBe("# cli");
  });

  test("self-heals a nested path that flipped from directory to file", () => {
    write("src/nested/a.txt", "a");
    syncTreeByContent(join(root, "src"), join(root, "dest"));
    rmSync(join(root, "src", "nested"), { recursive: true });
    write("src/nested", "now a file");

    const changed = syncTreeByContent(join(root, "src"), join(root, "dest"));

    expect(changed).toBeGreaterThan(0);
    expect(readFileSync(join(root, "dest", "nested"), "utf8")).toBe("now a file");
  });

  test("stages a plain file as a plain file", () => {
    write("SKILL.md", "# skill");

    const changed = syncTreeByContent(join(root, "SKILL.md"), join(root, "dest", "SKILL.md"));

    expect(changed).toBe(1);
    expect(readFileSync(join(root, "dest", "SKILL.md"), "utf8")).toBe("# skill");
    expect(syncTreeByContent(join(root, "SKILL.md"), join(root, "dest", "SKILL.md"))).toBe(0);
  });
});

describe("pruneEmptyDirs", () => {
  test("removes emptied nested directories but keeps the root", () => {
    mkdirSync(join(root, "stage", "empty", "deeper"), { recursive: true });
    write("stage/kept/file.txt", "x");

    pruneEmptyDirs(join(root, "stage"));

    expect(existsSync(join(root, "stage"))).toBe(true);
    expect(existsSync(join(root, "stage", "empty"))).toBe(false);
    expect(existsSync(join(root, "stage", "kept", "file.txt"))).toBe(true);
  });

  test("keeps a root that is itself empty", () => {
    mkdirSync(join(root, "stage"), { recursive: true });

    pruneEmptyDirs(join(root, "stage"));

    expect(existsSync(join(root, "stage"))).toBe(true);
  });
});

describe("compileInputs", () => {
  test("covers the sources bun build --compile actually bundles", () => {
    expect(compileInputs("/skill")).toEqual([
      join("/skill", "src"),
      join("/skill", "package.json"),
      join("/skill", "tsconfig.json"),
    ]);
  });

  test("the ignored subtree is the one packages/skill's tsconfig excludes", () => {
    // Guards against drift: if that tsconfig ever stops excluding src/tests,
    // the ignore predicate here would be silently skipping a real input.
    const tsconfig = readFileSync(
      join(import.meta.dir, "..", "packages", "skill", "tsconfig.json"),
      "utf8",
    );
    expect(tsconfig).toContain("src/tests/**/*");
    expect(isNotACompileInput(join("tests", "install.test.ts"))).toBe(true);
  });

  test("excludes the runtime-only resources read via --resource-root", () => {
    // SKILL.md/references/shims are read at runtime from the staged resource
    // root, never bundled. Counting them would recompile 58MB whenever a doc
    // line moved -- exactly the tax the guard exists to remove.
    const inputs = compileInputs("/skill");
    for (const runtimeOnly of ["SKILL.md", "references", "shims", "dist"]) {
      expect(inputs).not.toContain(join("/skill", runtimeOnly));
    }
  });
});

describe("compileInstaller", () => {
  test("produces a runnable binary at outfile and leaves no temp file behind", async () => {
    const entrypoint = write("entry.ts", "console.log('hello from installer');");
    const outfile = join(root, "installer");

    await compileInstaller(entrypoint, outfile);

    expect(existsSync(outfile)).toBe(true);
    expect(statSync(outfile).size).toBeGreaterThan(0);
    expect(listFilesRecursive(root).filter((rel) => rel.includes(".tmp-"))).toEqual([]);
  }, 60_000);

  test("never names outfile as the compile target", async () => {
    // The load-bearing property. A real `bun build --compile` resolves and
    // bundles before opening its output, so no malformed entrypoint can
    // reproduce a partial write from outside -- which is exactly why this
    // asserts the seam instead: outfile must only ever be written by the
    // rename, never handed to the compiler.
    const outfile = join(root, "installer");
    const targets: string[] = [];

    await compileInstaller("entry.ts", outfile, async (_entry, target) => {
      targets.push(target);
      writeFileSync(target, "compiled bytes");
    });

    expect(targets).toHaveLength(1);
    expect(targets[0]).not.toBe(outfile);
    expect(readFileSync(outfile, "utf8")).toBe("compiled bytes");
  });

  test("a compile that dies mid-write leaves the previous binary untouched", async () => {
    // The latch this guards against: a compile killed partway through writing
    // 58MB must not leave a freshly-stamped truncated outfile, or
    // isOutputFresh reports it fresh forever and every later run silently
    // ships the corrupt binary. Reverting compileInstaller to write outfile
    // directly fails this test.
    const outfile = write("installer", "previous good binary");
    setMtime(outfile, 1_000);

    await expect(
      compileInstaller("entry.ts", outfile, async (_entry, target) => {
        writeFileSync(target, "truncated par");
        throw new Error("killed mid-write");
      }),
    ).rejects.toThrow("killed mid-write");

    expect(readFileSync(outfile, "utf8")).toBe("previous good binary");
    expect(statSync(outfile).mtimeMs).toBe(1_000_000);
    expect(listFilesRecursive(root).filter((rel) => rel.includes(".tmp-"))).toEqual([]);
  });

  test("a compile that dies mid-write leaves the output still stale, so the next run retries", async () => {
    // The end-to-end consequence: after a failure the guard must still say
    // "not fresh" rather than latching on the partial artifact.
    const source = write("src/entry.ts", "console.log('x');");
    const outfile = write("installer", "previous good binary");
    setMtime(outfile, 1_000);
    setMtime(source, 5_000);

    await expect(
      compileInstaller("entry.ts", outfile, async (_entry, target) => {
        writeFileSync(target, "truncated par");
        throw new Error("killed mid-write");
      }),
    ).rejects.toThrow();

    expect(isOutputFresh(outfile, [join(root, "src")])).toBe(false);
  });

  test("sweeps a temp file orphaned by an earlier hard kill", async () => {
    // A SIGKILL skips the catch, so nothing would otherwise reclaim a ~58MB
    // partial. Sweeping on the next call bounds it at one per interrupted run.
    const outfile = join(root, "installer");
    writeFileSync(`${outfile}.tmp-999999`, "orphaned 58MB partial");

    await compileInstaller("entry.ts", outfile, async (_entry, target) => {
      writeFileSync(target, "compiled bytes");
    });

    expect(listFilesRecursive(root).filter((rel) => rel.includes(".tmp-"))).toEqual([]);
    expect(readFileSync(outfile, "utf8")).toBe("compiled bytes");
  });

  test("does not sweep unrelated files in the output directory", async () => {
    const outfile = join(root, "installer");
    writeFileSync(join(root, "nodespaced"), "a different sidecar");
    writeFileSync(join(root, "installer-other.tmp-1"), "another binary's temp");

    await compileInstaller("entry.ts", outfile, async (_entry, target) => {
      writeFileSync(target, "compiled bytes");
    });

    expect(readFileSync(join(root, "nodespaced"), "utf8")).toBe("a different sidecar");
    expect(readFileSync(join(root, "installer-other.tmp-1"), "utf8")).toBe(
      "another binary's temp",
    );
  });

  test("keeps outfile's extension on the compile target", async () => {
    // The regression this guards: `bun build --compile` on Windows silently
    // appends `.exe` to a target that doesn't already end in it, so a naive
    // `${outfile}.tmp-${pid}` target (extension-less) gets written to a
    // different path than the one compileInstaller then tries to rename,
    // and the rename 404s. The tmp marker must land before the extension,
    // not after, on every platform outfile might carry one on.
    const outfile = join(root, "installer.exe");
    const targets: string[] = [];

    await compileInstaller("entry.ts", outfile, async (_entry, target) => {
      targets.push(target);
      writeFileSync(target, "compiled bytes");
    });

    expect(targets[0]).toMatch(/\.exe$/);
    expect(readFileSync(outfile, "utf8")).toBe("compiled bytes");
  });

  test("sweeps an orphaned temp file for an outfile that carries an extension", async () => {
    const outfile = join(root, "installer.exe");
    writeFileSync(join(root, "installer.tmp-999999.exe"), "orphaned 58MB partial");

    await compileInstaller("entry.ts", outfile, async (_entry, target) => {
      writeFileSync(target, "compiled bytes");
    });

    expect(listFilesRecursive(root).filter((rel) => rel.includes(".tmp-"))).toEqual([]);
    expect(readFileSync(outfile, "utf8")).toBe("compiled bytes");
  });

  test("rejects with the compiler's own error, unwrapped", async () => {
    const outfile = join(root, "installer");
    const failure = new Error("bun build exited with code 1");

    await expect(
      compileInstaller("entry.ts", outfile, () => Promise.reject(failure)),
    ).rejects.toBe(failure);

    expect(existsSync(outfile)).toBe(false);
  });
});

describe("STAGED_ENTRIES", () => {
  test("matches the files packages/skill declares as its shipped artifact", () => {
    const skillPkg = JSON.parse(
      readFileSync(join(import.meta.dir, "..", "packages", "skill", "package.json"), "utf8"),
    ) as { files: string[] };

    // package.json itself is staged beyond the published `files` list, purely
    // for its `"type": "module"` marker (see the script's module doc).
    expect(STAGED_ENTRIES.filter((entry) => entry !== "package.json").sort()).toEqual(
      [...skillPkg.files].sort(),
    );
    expect(STAGED_ENTRIES).toContain("package.json");
  });
});
