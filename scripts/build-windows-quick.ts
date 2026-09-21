#!/usr/bin/env bun
/**
 * Tier 1 local Windows build: cross-compile from macOS (or Linux) via
 * cargo-xwin, producing an NSIS installer only.
 *
 * Mirrors the Windows leg of .github/workflows/release.yml's `build-tauri`
 * job (matrix id `windows-x86`) as closely as a non-Windows host allows:
 * builds the same two sidecars (nodespaced, nodespace) for the same target
 * (x86_64-pc-windows-msvc) and the same NSIS bundle. What it does NOT
 * produce, and cannot:
 *   - a .msi installer — WiX Toolset only runs on a real Windows host
 *     (Tier 2, scripts/build-windows.ts)
 *   - a code-signed binary — no Windows signing certificate locally
 *
 * ## Prerequisites (one-time)
 *
 *   brew install llvm            # clang-cl, the cross-compiler cargo-xwin uses
 *   brew install lld             # lld-link -- split out of the llvm formula
 *                                 # (see the llvm-prereq check below)
 *   brew install ninja           # llama-cpp-sys-2's CMake build forces the
 *                                 # Ninja generator when cross-compiling via
 *                                 # cargo-xwin; Unix Makefiles isn't offered
 *                                 # as a fallback there
 *   brew install makensis        # Tauri's NSIS bundler shells out to this to
 *                                 # actually produce the installer -- the
 *                                 # Homebrew formula is named `makensis`, not
 *                                 # `nsis`
 *   cargo install --locked cargo-xwin
 *   rustup target add x86_64-pc-windows-msvc
 *
 * Every one of the above is checked below with an actionable error instead
 * of a confusing failure three steps later -- this list was arrived at by
 * running the build for real on a clean machine that had none of them; the
 * issue this script implements documented only the first, third and fourth
 * (llvm, cargo-xwin, rustup target) -- `brew install lld`, `brew install
 * ninja` and `brew install makensis` were all missing from that list and are
 * included here precisely because a "clean Mac" run failed without them, one
 * at a time, each at a different stage of the build.
 *
 * ## What actually gets cross-compiled
 *
 * nodespaced/nodespace both pull in nodespace-nlp-engine with its default
 * features (embedding-service + chat-service), which means llama-cpp-2 --
 * i.e. this is not a pure-Rust cross-compile, it cross-compiles llama.cpp
 * (C/C++) too, via clang-cl emulating MSVC. First run additionally downloads
 * and caches the Windows SDK + MSVC CRT headers/libs via `xwin` (a few
 * hundred MB). Expect this to take substantially longer than a native
 * `cargo build` -- tens of minutes is normal for a cold cache, not a sign
 * something is wrong.
 *
 * ## The skill-installer sidecar
 *
 * Tauri's `externalBin` also expects a compiled `nodespace-skill-installer`
 * sidecar matching the TARGET triple. scripts/build-skill.ts's own default
 * always compiles for `hostTriple()` (correct for every other caller, which
 * never cross-compiles), so this script calls it with an explicit
 * `--target` override to cross-compile that binary too, via Bun's own
 * `--compile --target=` cross-compilation support.
 *
 * Usage:
 *   bun run build:windows:quick
 */

import { $ } from 'bun';
import { chmodSync, copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

export const TARGET = 'x86_64-pc-windows-msvc';
const WORKSPACE_ROOT = join(import.meta.dir, '..');
const DESKTOP_APP_DIR = join(WORKSPACE_ROOT, 'packages', 'desktop-app');
const BIN_DIR = join(DESKTOP_APP_DIR, 'src-tauri', 'binaries');
const TARGET_RELEASE_DIR = join(WORKSPACE_ROOT, 'target', TARGET, 'release');

// Binary names, not cargo package names -- nodespaced/nodespace are bin
// targets built via `--bin <name>`, not `-p <name>` (those are
// nodespace-daemon/nodespace-cli). A plain string list, not `{ crate, bin }`
// objects: an earlier version carried an unused `crate` field seeded with
// these same (wrong, if read as a package name) values, a footgun for
// anyone wiring `-p ${crate}` into the cargo-xwin invocation by analogy with
// build-sidecar-binaries.ts's actual `{ crate, bin }` pairs, which does use
// `crate` for exactly that.
const SIDECAR_BINARIES = ['nodespaced', 'nodespace'];

async function commandExists(cmd: string): Promise<boolean> {
  try {
    await $`which ${cmd}`.quiet();
    return true;
  } catch {
    return false;
  }
}

/**
 * Resolves Homebrew's llvm keg-only install location, or `null` if brew
 * itself or the llvm formula isn't present. Never assumes a fixed path --
 * differs between Apple Silicon (/opt/homebrew) and Intel (/usr/local), and
 * Homebrew isn't installed at all on some machines that might otherwise have
 * clang-cl on PATH some other way.
 */
async function llvmBinDir(): Promise<string | null> {
  try {
    const prefix = (await $`brew --prefix llvm`.quiet().text()).trim();
    return existsSync(join(prefix, 'bin', 'clang-cl')) ? join(prefix, 'bin') : null;
  } catch {
    return null;
  }
}

/** Raw facts `checkPrerequisites` probes for -- kept separate from the pure
 * problem-list logic below (`missingPrerequisites`) so that logic is
 * testable with synthetic input, without shelling out to `which`/`brew`/
 * `rustup` in a test run. */
export interface PrereqFacts {
  hasCargoXwin: boolean;
  hasLlvm: boolean;
  hasLldLink: boolean;
  hasNinja: boolean;
  hasMakensis: boolean;
  /** `null` means rustup itself isn't installed/runnable, which is a
   * different problem than "installed but missing the target". */
  installedRustTargets: string[] | null;
}

/**
 * Actionable problem descriptions for every missing prerequisite in
 * `facts`, empty when all are present. Pure -- see `checkPrerequisites` for
 * the actual `which`/`brew`/`rustup` probing this consumes.
 */
export function missingPrerequisites(facts: PrereqFacts): string[] {
  const problems: string[] = [];

  if (!facts.hasCargoXwin) {
    problems.push(
      'cargo-xwin is not installed.\n' + '    Install with: cargo install --locked cargo-xwin',
    );
  }

  if (!facts.hasLlvm) {
    problems.push(
      'llvm (clang-cl) is not installed, or not installed via Homebrew.\n' +
        '    Install with: brew install llvm',
    );
  }

  // lld-link: Homebrew's llvm formula no longer bundles LLD ("LLD and LLDB
  // are now provided in separate formulae" per `brew info llvm`) -- it must
  // be installed on its own, and cargo-xwin's linker step (-fuse-ld=lld-link)
  // needs it on PATH.
  if (!facts.hasLldLink) {
    problems.push('lld-link is not installed.\n' + '    Install with: brew install lld');
  }

  // ninja: llama-cpp-sys-2's build.rs sets CMAKE_GENERATOR=Ninja
  // unconditionally when cross-compiling (see this script's module doc) --
  // there is no fallback to Unix Makefiles, so a configure with no ninja on
  // PATH fails immediately with "CMake was unable to find a build program".
  if (!facts.hasNinja) {
    problems.push('ninja is not installed.\n' + '    Install with: brew install ninja');
  }

  // makensis: Tauri's NSIS bundler shells out to `makensis` to actually
  // produce the installer .exe from the compiled binary -- the Rust build
  // can succeed completely and this still fails at the very last step
  // ("failed to run command makensis.exe: No such file or directory") if
  // it's missing. Homebrew's formula for it is named `makensis`, not
  // `nsis` (the NSIS project's own name) -- easy to search for the wrong
  // thing.
  if (!facts.hasMakensis) {
    problems.push('makensis (NSIS) is not installed.\n' + '    Install with: brew install makensis');
  }

  if (facts.installedRustTargets === null) {
    problems.push('rustup is not installed or not on PATH.\n' + '    Install from: https://rustup.rs');
  } else if (!facts.installedRustTargets.includes(TARGET)) {
    problems.push(
      `Rust target ${TARGET} is not installed.\n` + `    Install with: rustup target add ${TARGET}`,
    );
  }

  return problems;
}

interface PrereqResult {
  ok: boolean;
  llvmBin: string | null;
}

async function checkPrerequisites(): Promise<PrereqResult> {
  const llvmBin = await llvmBinDir();

  let installedRustTargets: string[] | null = null;
  try {
    const output = await $`rustup target list --installed`.quiet().text();
    installedRustTargets = output.split('\n').map((line) => line.trim()).filter(Boolean);
  } catch {
    installedRustTargets = null;
  }

  const problems = missingPrerequisites({
    hasCargoXwin: await commandExists('cargo-xwin'),
    hasLlvm: llvmBin !== null,
    hasLldLink: await commandExists('lld-link'),
    hasNinja: await commandExists('ninja'),
    hasMakensis: await commandExists('makensis'),
    installedRustTargets,
  });

  if (problems.length > 0) {
    console.error('error: missing prerequisites for `bun run build:windows:quick`:\n');
    for (const problem of problems) console.error(`  - ${problem}\n`);
    console.error('See the local-builds development doc for the full prerequisite list.');
    return { ok: false, llvmBin };
  }

  return { ok: true, llvmBin };
}

/**
 * tauri.conf.json's `bundle.resources` includes `resources/models/**\/*`,
 * which is gitignored and empty on a fresh checkout, so without this the
 * produced NSIS installer silently ships without the embedding model -- the
 * exact bug this cross-compile path exists to reproduce "as closely as a
 * non-Windows host allows" (see module doc above). Unlike release.yml's CI
 * step (which fetches a pre-vetted copy from the `models-v2` GitHub Release
 * for build-pipeline reliability), this is a local dev machine, so it uses
 * the repo's own canonical, integrity-verified downloader
 * (scripts/download-models.ts, ADR-058) directly: it hashes an
 * already-downloaded file before reusing it -- so a truncated/corrupt file
 * left by an interrupted previous run gets re-fetched, not silently reused
 * -- and creates its own target directory, unlike `copySidecarBinaries`
 * below.
 */
async function downloadEmbeddingModel(): Promise<void> {
  console.log('\n==> Downloading embedding model for bundling');
  await $`bun run download:models:bundle`.cwd(WORKSPACE_ROOT);
}

async function buildSidecar(bin: string, env: Record<string, string | undefined>): Promise<void> {
  console.log(`\n==> cargo xwin build --release --bin ${bin} --target ${TARGET}`);
  await $`cargo xwin build --release --bin ${bin} --target ${TARGET}`.cwd(WORKSPACE_ROOT).env(env);
}

function copySidecarBinaries(): void {
  mkdirSync(BIN_DIR, { recursive: true });
  for (const bin of SIDECAR_BINARIES) {
    const src = join(TARGET_RELEASE_DIR, `${bin}.exe`);
    if (!existsSync(src)) {
      throw new Error(
        `Built binary not found: ${src}\n` +
          `  cargo xwin build reported success but the expected output is missing -- ` +
          `check that --bin ${bin} matches a [[bin]] target in the crate that builds it.`,
      );
    }
    const dest = join(BIN_DIR, `${bin}-${TARGET}.exe`);
    copyFileSync(src, dest);
    chmodSync(dest, 0o755);
    console.log(`  ${dest}`);
  }
}

async function main(): Promise<void> {
  if (process.platform === 'win32') {
    console.error(
      'error: build:windows:quick cross-compiles TO Windows FROM a non-Windows host.\n' +
        '  On a real Windows machine, build natively instead: bunx tauri build (see also `bun run build:windows`, Tier 2).',
    );
    process.exit(1);
  }

  const { ok, llvmBin } = await checkPrerequisites();
  if (!ok || !llvmBin) process.exit(1);

  await downloadEmbeddingModel();

  // cargo-xwin's default backend (clang-cl) resolves the compiler from
  // PATH. Homebrew's llvm is keg-only (never symlinked into
  // /opt/homebrew/bin) specifically so it doesn't shadow macOS's system
  // clang, so it must be prepended here rather than relying on the user
  // having added it to their shell rc.
  const env = { ...process.env, PATH: `${llvmBin}:${process.env.PATH ?? ''}` };

  for (const bin of SIDECAR_BINARIES) {
    await buildSidecar(bin, env);
  }

  console.log('\n==> Copying sidecar binaries to src-tauri/binaries/');
  copySidecarBinaries();

  console.log(`\n==> Cross-compiling the skill-installer sidecar for ${TARGET}`);
  await $`bun run skill:check`.cwd(WORKSPACE_ROOT);
  await $`bun run scripts/build-skill.ts --target ${TARGET}`.cwd(WORKSPACE_ROOT);

  // --runner cargo-xwin: `bunx tauri build` otherwise shells out to plain
  // `cargo build` for the src-tauri crate itself (nodespace-app and its own
  // dependency tree -- tao, wry, reqwest/rustls -> ring, etc.), which has
  // none of cargo-xwin's CC/CXX/linker environment and fails outright (ring
  // failing to find <assert.h> compiling with plain macOS `cc` targeting
  // MSVC is the actual failure this produces). `cargo-xwin` itself doubles
  // as a drop-in `cargo` replacement for exactly this purpose (`cargo-xwin
  // build`, `cargo-xwin metadata`, etc., alongside its own `cargo xwin ...`
  // subcommand form) -- confirmed by running this exact combination
  // end-to-end.
  console.log(`\n==> bunx tauri build --runner cargo-xwin --target ${TARGET} --bundles nsis`);
  await $`bunx tauri build --runner cargo-xwin --target ${TARGET} --bundles nsis`
    .cwd(DESKTOP_APP_DIR)
    .env(env);

  console.log(
    `\nDone. NSIS installer: target/${TARGET}/release/bundle/nsis/\n` +
      '(.msi is not produced by this path -- WiX requires a real Windows host; see `bun run build:windows`.)',
  );
}

if (import.meta.main) {
  main().catch((err) => {
    console.error('\nbuild-windows-quick failed:', err.message ?? err);
    process.exit(1);
  });
}
