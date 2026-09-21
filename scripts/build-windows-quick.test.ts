// Covers the pure prerequisite-problem-list logic in
// scripts/build-windows-quick.ts against synthetic facts. The script's own
// top-level build (cargo xwin, tauri build) is behind `import.meta.main`, so
// importing it here runs nothing real -- no actual `which`/`brew`/`rustup`
// shelling, no cross-compile.
import { describe, expect, test } from "bun:test";
import { missingPrerequisites, TARGET, type PrereqFacts } from "./build-windows-quick";

const ALL_PRESENT: PrereqFacts = {
  hasCargoXwin: true,
  hasLlvm: true,
  hasLldLink: true,
  hasNinja: true,
  hasMakensis: true,
  hasGh: true,
  installedRustTargets: [TARGET],
};

describe("missingPrerequisites", () => {
  test("reports nothing when every prerequisite is present", () => {
    expect(missingPrerequisites(ALL_PRESENT)).toEqual([]);
  });

  test("reports cargo-xwin missing with its install command", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasCargoXwin: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/cargo-xwin is not installed/);
    expect(problems[0]).toContain("cargo install --locked cargo-xwin");
  });

  test("reports llvm missing with its install command", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasLlvm: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/llvm \(clang-cl\) is not installed/);
    expect(problems[0]).toContain("brew install llvm");
  });

  // The regression this guards: Homebrew split lld-link out of the llvm
  // formula, so `brew install llvm` alone leaves cargo-xwin's linker step
  // broken -- this needs its own distinct check and install instruction, not
  // folded into the llvm one above.
  test("reports lld-link missing separately from llvm, with its own install command", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasLldLink: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/lld-link is not installed/);
    expect(problems[0]).toContain("brew install lld");
  });

  // The regression this guards: llama-cpp-sys-2's CMake build forces the
  // Ninja generator when cross-compiling, with no Unix Makefiles fallback --
  // discovered by running the real build without ninja installed.
  test("reports ninja missing with its install command", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasNinja: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/ninja is not installed/);
    expect(problems[0]).toContain("brew install ninja");
  });

  // The regression this guards: Tauri's NSIS bundler shells out to
  // `makensis` as its very last step -- discovered by running the real
  // build to completion (Rust compile succeeded fully) and having it fail
  // only there. Homebrew's formula is named `makensis`, not `nsis`.
  test("reports makensis missing with its install command", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasMakensis: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/makensis \(NSIS\) is not installed/);
    expect(problems[0]).toContain("brew install makensis");
  });

  // The regression this guards: the embedding-model download this script
  // added (mirroring release.yml's Windows leg) needs `gh` on PATH and
  // authenticated, which nothing else in this prerequisite list checked for.
  test("reports gh missing with its install and auth commands", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, hasGh: false });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/gh \(GitHub CLI\) is not installed/);
    expect(problems[0]).toContain("brew install gh");
    expect(problems[0]).toContain("gh auth login");
  });

  test("reports rustup itself missing (installedRustTargets: null) distinctly from a missing target", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, installedRustTargets: null });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/rustup is not installed/);
    expect(problems[0]).not.toContain("rustup target add");
  });

  test("reports the target missing when rustup is present but the triple isn't installed", () => {
    const problems = missingPrerequisites({ ...ALL_PRESENT, installedRustTargets: ["aarch64-apple-darwin"] });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`rustup target add ${TARGET}`);
  });

  test("does not report the target missing when it's present alongside others", () => {
    const problems = missingPrerequisites({
      ...ALL_PRESENT,
      installedRustTargets: ["aarch64-apple-darwin", TARGET],
    });
    expect(problems).toEqual([]);
  });

  test("accumulates one problem per missing prerequisite, independently", () => {
    const problems = missingPrerequisites({
      hasCargoXwin: false,
      hasLlvm: false,
      hasLldLink: false,
      hasNinja: false,
      hasMakensis: false,
      hasGh: false,
      installedRustTargets: null,
    });
    expect(problems).toHaveLength(7);
  });
});
