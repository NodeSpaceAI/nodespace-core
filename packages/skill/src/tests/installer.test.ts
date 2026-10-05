import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  mkdirSync,
  rmSync,
  rmdirSync,
  existsSync,
  readFileSync,
  readdirSync,
  writeFileSync,
  symlinkSync,
  lstatSync,
  statSync,
  utimesSync,
} from 'node:fs';
import { join, basename, dirname } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-skill-test-${process.pid}`);
const FAKE_PKG_ROOT = join(TMP, 'pkg');

vi.mock('node:os', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

// Isolate from the ambient env: claude-code detection honors $CLAUDE_CONFIG_DIR,
// so an inherited value (e.g. a `claude-ns` profile) would point the default
// AGENTS at a real dir and break the mocked-home assumptions below. The dedicated
// CLAUDE_CONFIG_DIR describe sets it explicitly where it needs to.
delete process.env.CLAUDE_CONFIG_DIR;

const {
  install,
  uninstall,
  checkInstalled,
  detectAgents,
  isNodespaceBinaryOnPath,
  claudeCodePluginManagedSkillExists,
  listReferenceFiles,
  INSTALL_RECORD,
  PRE_RECORD_REFERENCES,
} = await import('../installer.js');
const { AGENTS, SHARED_SKILL_FRONTMATTER } = await import('../agents.js');

const SKILL_MD_CONTENT = '# NodeSpace Skill\nTest content';
const PLUGIN_FILE_CONTENT = '// plugin file content';

// Reference files `seedPkgRoot` puts in the package root. The installer takes
// its reference list from the package root's `references/` directory, so this
// includes one file no agent config names (`extra-playbook.md`): it installs
// only if the directory, not a hand-kept list, decides.
const SEEDED_REFERENCES = [
  'references/cli.md',
  'references/extra-playbook.md',
  'references/graph-authored-guidance.md',
];

/**
 * What an agent installs besides the references, as `[path under the package
 * root, path inside the install directory]`: `SKILL.md`, and its harness
 * plugin's files where it has one.
 */
function agentFiles(agent: typeof AGENTS[number]): Array<[string, string]> {
  const plugin = agent.plugin;
  return [
    ['SKILL.md', 'SKILL.md'],
    ...(plugin ? plugin.files.map((file): [string, string] => [`${plugin.dir}/${file}`, file]) : []),
  ];
}

function seedPkgRoot(root: string, agent: typeof AGENTS[number]): void {
  for (const [src] of agentFiles(agent)) {
    mkdirSync(dirname(join(root, src)), { recursive: true });
    const content = src.endsWith('.md')
      ? `${SKILL_MD_CONTENT}\n<!-- ${src} -->`
      : `${PLUGIN_FILE_CONTENT} (${src})`;
    writeFileSync(join(root, src), content, 'utf8');
  }
  // Content is unique per file path, not just per file "kind" -- so a test
  // can tell multiple installed files apart, e.g. that installing several
  // reference files lands each with its own content rather than one silently
  // overwriting another.
  mkdirSync(join(root, 'references'), { recursive: true });
  for (const ref of SEEDED_REFERENCES) {
    writeFileSync(join(root, ref), `${SKILL_MD_CONTENT}\n<!-- ${ref} -->`, 'utf8');
  }
}

/** Everything `install()` puts in an agent's install directory from a `seedPkgRoot` package root. */
function seededInstallFiles(agent: typeof AGENTS[number]): string[] {
  return [...agentFiles(agent).map(([, rel]) => rel), ...SEEDED_REFERENCES].sort();
}

function readRecord(agent: typeof AGENTS[number]): { files: string[] } {
  return JSON.parse(readFileSync(join(agent.installDir, INSTALL_RECORD), 'utf8'));
}

/**
 * `../installer.js` loaded afresh against a `node:fs` whose functions `replace`
 * overrides, so a test decides the order a directory lists in, or that a delete
 * fails, instead of inheriting whatever the platform and the user running it
 * allow. The mock is removed again after each test.
 */
async function importInstallerWithFs(
  replace: (actual: typeof import('node:fs')) => Partial<typeof import('node:fs')>
): Promise<typeof import('../installer.js')> {
  vi.resetModules();
  vi.doMock('node:fs', async importOriginal => {
    const actual = await importOriginal<typeof import('node:fs')>();
    return { ...actual, ...replace(actual) };
  });
  return import('../installer.js');
}

/** A `node:fs` function that fails with `code` whenever `fails(path)` says so, and otherwise behaves as `real`. */
function failingOn<F extends (path: never, ...rest: never[]) => unknown>(
  real: F,
  fails: (path: string) => boolean,
  code = 'EACCES'
): F {
  return ((path: never, ...rest: never[]) => {
    if (fails(String(path))) throw Object.assign(new Error(`${code}: simulated`), { code });
    return real(path, ...rest);
  }) as F;
}

beforeEach(() => {
  mkdirSync(TMP, { recursive: true });
  mkdirSync(FAKE_PKG_ROOT, { recursive: true });
  writeFileSync(join(FAKE_PKG_ROOT, 'SKILL.md'), SKILL_MD_CONTENT, 'utf8');
});

afterEach(() => {
  vi.doUnmock('node:fs');
  vi.resetModules();
  rmSync(TMP, { recursive: true, force: true });
});

describe('AGENTS config', () => {
  it('defines five agents', () => {
    expect(AGENTS).toHaveLength(5);
    const names = AGENTS.map(a => a.name);
    expect(names).toContain('claude-code');
    expect(names).toContain('codex');
    expect(names).toContain('antigravity');
    expect(names).toContain('opencode');
    expect(names).toContain('pi');
  });

  // Reference files are not listed per agent: the installer copies every
  // `references/*.md` it finds in the package root. A `references/` entry in a
  // plugin's file list would be the hand-kept list coming back.
  it('each agent has detectionDir and installDir, and names no references', () => {
    for (const agent of AGENTS) {
      expect(agent.detectionDir).toBeTruthy();
      expect(agent.installDir).toBeTruthy();
      expect(
        (agent.plugin?.files ?? []).filter(file => file.startsWith('references/')),
        `${agent.name} lists references`
      ).toEqual([]);
    }
  });

  // Claude Code loads a plugin from its skill folder (ADR-093 §5). The other
  // harnesses get `SKILL.md` and the references alone.
  it('only claude-code installs a harness plugin, and every file it lists exists', () => {
    for (const agent of AGENTS) {
      if (agent.name !== 'claude-code') {
        expect(agent.plugin, `${agent.name} has a plugin`).toBeUndefined();
        continue;
      }
      expect(agent.plugin?.files).toContain('.claude-plugin/plugin.json');
      expect(agent.plugin?.files).toContain('hooks/hooks.json');
      for (const [src] of agentFiles(agent)) {
        expect(
          existsSync(join(import.meta.dirname, '../..', src)),
          `${src} is listed but not in the package`
        ).toBe(true);
      }
    }
  });

  // The hooks file names the module Claude Code loads. A module the plugin's
  // file list leaves out would install a plugin that cannot load.
  it("installs the module the plugin's hooks file names", () => {
    const plugin = AGENTS.find(a => a.name === 'claude-code')!.plugin!;
    const hooks = JSON.parse(
      readFileSync(join(import.meta.dirname, '../..', plugin.dir, 'hooks/hooks.json'), 'utf8')
    ) as { modules: string[] };
    const manifest = JSON.parse(
      readFileSync(join(import.meta.dirname, '../..', plugin.dir, '.claude-plugin/plugin.json'), 'utf8')
    ) as { types: string };
    for (const module of hooks.modules) {
      expect(plugin.files).toContain(join('hooks', module));
    }
    expect(plugin.files).toContain(join(manifest.types));
  });

  it('install paths are under the expected agent dir', () => {
    const expectedDirs: Record<string, string> = {
      'claude-code': '.claude',
      codex: '.codex',
      antigravity: '.gemini',
      opencode: '.opencode',
      pi: '.pi',
    };
    for (const agent of AGENTS) {
      expect(agent.installDir).toContain(expectedDirs[agent.name]);
    }
  });
});

describe('install', () => {
  it('returns empty array when no agents are detected', () => {
    const results = install(undefined, FAKE_PKG_ROOT);
    expect(results).toEqual([]);
  });

  it('installs SKILL.md when agent dir exists (only SKILL.md seeded)', () => {
    const agentName = 'claude-code';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.detectionDir, { recursive: true });

    const results = install([agentName], FAKE_PKG_ROOT);
    expect(results).toHaveLength(1);
    expect(results[0].agent).toBe(agentName);
    expect(results[0].installed).toHaveLength(1);
    expect(existsSync(join(config.installDir, 'SKILL.md'))).toBe(true);
    const expected = config.skillFrontmatter
      ? config.skillFrontmatter + '\n' + SKILL_MD_CONTENT
      : SKILL_MD_CONTENT;
    expect(readFileSync(join(config.installDir, 'SKILL.md'), 'utf8')).toBe(expected);
  });

  // A skill folder with no frontmatter is not a valid skill under the Agent
  // Skills standard — `name` + `description` are the entire discovery surface.
  // Three of the four targets used to install without it, so the skill could
  // never activate there.
  it('prepends frontmatter to SKILL.md for every target, not just Claude Code', () => {
    for (const config of AGENTS) {
      expect(config.skillFrontmatter, `${config.name} has no frontmatter`).toBeTruthy();
      mkdirSync(config.detectionDir, { recursive: true });
      install([config.name], FAKE_PKG_ROOT);
      const content = readFileSync(join(config.installDir, 'SKILL.md'), 'utf8');
      expect(content.startsWith('---\nname: nodespace')).toBe(true);
      expect(content).toContain('allowed-tools: Bash(nodespace:*)');
      expect(content).toContain(SKILL_MD_CONTENT);
    }
  });

  // Four separate places enumerate what the skill is made of: this package's
  // `files` array (npm), scripts/build-skill.ts (Tauri bundle), the per-agent
  // file lists (install), and context_assembly.rs (PTY). A path present in
  // one and missing from another ships a body linking to a file that isn't
  // there — silently, because nothing errors.
  it('publishes every directory the agents install from', () => {
    const pkg = JSON.parse(
      readFileSync(join(import.meta.dirname, '../../package.json'), 'utf8')
    ) as { files: string[] };

    // The first segment of each installed path: a directory (`plugins`) or a
    // bare file (`SKILL.md`). npm's `files` accepts either form verbatim, so an
    // exact match is the whole check. `references` is added by hand: no agent
    // names it, but the installer copies everything in it.
    const topLevel = new Set([
      'references',
      ...AGENTS.flatMap(a => agentFiles(a)).map(([src]) => src.split('/')[0]),
    ]);
    for (const entry of topLevel) {
      expect(
        pkg.files,
        `"${entry}" is installed but not in package.json "files"`
      ).toContain(entry);
    }
  });

  it('installs the same frontmatter block for every target', () => {
    const blocks = new Set(AGENTS.map(a => a.skillFrontmatter));
    expect(blocks.size).toBe(1);
    expect([...blocks][0]).toBe(SHARED_SKILL_FRONTMATTER);
  });

  // The six fields below are the entire set the standard defines. Claude Code
  // tolerates extra keys, but other distribution paths hard-error on any key
  // they don't recognize — so a harness-specific field added to the shared
  // block would silently break installs everywhere else.
  it('uses only spec-defined frontmatter fields', () => {
    const SPEC_FIELDS = [
      'name',
      'description',
      'license',
      'compatibility',
      'metadata',
      'allowed-tools',
    ];
    const body = SHARED_SKILL_FRONTMATTER.replace(/^---\n/, '').replace(/---\n?$/, '');
    const topLevelKeys = body
      .split('\n')
      .filter(line => /^[A-Za-z][A-Za-z0-9-]*:/.test(line))
      .map(line => line.slice(0, line.indexOf(':')));
    expect(topLevelKeys.length).toBeGreaterThan(0);
    for (const key of topLevelKeys) {
      expect(SPEC_FIELDS, `"${key}" is not a spec-defined frontmatter field`).toContain(key);
    }
  });

  // `name` must match the directory the skill installs into, and is capped at
  // 64 characters of lowercase alphanumerics and hyphens.
  it('uses a spec-valid name matching the install directory', () => {
    const name = /^name:\s*(\S+)/m.exec(SHARED_SKILL_FRONTMATTER)?.[1];
    expect(name).toBe('nodespace');
    expect(name!.length).toBeLessThanOrEqual(64);
    expect(name).toMatch(/^[a-z0-9][a-z0-9-]*$/);
    for (const config of AGENTS) {
      expect(basename(config.installDir)).toBe(name);
    }
  });

  // The description is capped at 1024 characters by the spec. It is also the
  // only text an agent sees before deciding whether to load the skill, so it
  // must carry the vocabulary of the work it should be reached for.
  it('has a description within the spec length limit that covers docs/specs vocabulary', () => {
    const description = /description:\s*>\n([\s\S]*?)\n(?=[a-z-]+:|---)/m.exec(
      SHARED_SKILL_FRONTMATTER
    )?.[1];
    expect(description).toBeTruthy();
    const flattened = description!.trim().replace(/\s+/g, ' ');
    expect(flattened.length).toBeLessThanOrEqual(1024);
    for (const term of ['spec', 'ADR', 'architecture', 'design', 'plan']) {
      expect(flattened.toLowerCase(), `description omits "${term}"`).toContain(term.toLowerCase());
    }
  });

  it('installs SKILL.md, the plugin at its own paths and every reference when all source files exist', () => {
    const agentName = 'claude-code';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);

    const results = install([agentName], FAKE_PKG_ROOT);
    expect(results[0].installed).toHaveLength(agentFiles(config).length + SEEDED_REFERENCES.length);
    for (const [src, rel] of agentFiles(config)) {
      expect(existsSync(join(config.installDir, rel)), `${rel} was not installed`).toBe(true);
      if (rel !== 'SKILL.md') {
        expect(readFileSync(join(config.installDir, rel), 'utf8')).toBe(`${PLUGIN_FILE_CONTENT} (${src})`);
      }
    }
    // Claude Code finds the plugin by its manifest, at this exact path.
    expect(existsSync(join(config.installDir, '.claude-plugin', 'plugin.json'))).toBe(true);
    expect(existsSync(join(config.installDir, 'hooks', 'register.ts'))).toBe(true);
    for (const ref of SEEDED_REFERENCES) {
      expect(existsSync(join(config.installDir, ref))).toBe(true);
    }
  });

  // SKILL.md links to `references/cli.md` by relative path. A reference
  // flattened to its basename on install would leave the body pointing at a
  // file that isn't where it says — the
  // agent follows the link, finds nothing, and silently loses the CLI
  // reference.
  it('installs references into a references/ subdirectory, not flattened', () => {
    for (const config of AGENTS) {
      const refs = SEEDED_REFERENCES;

      mkdirSync(config.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, config);
      install([config.name], FAKE_PKG_ROOT);

      for (const ref of refs) {
        expect(existsSync(join(config.installDir, ref)), `${config.name}: ${ref}`).toBe(true);
        expect(existsSync(join(config.installDir, basename(ref)))).toBe(false);
      }

      const body = readFileSync(join(config.installDir, 'SKILL.md'), 'utf8');
      for (const ref of refs) {
        if (body.includes(ref)) {
          expect(existsSync(join(config.installDir, ref))).toBe(true);
        }
      }
    }
  });

  // `references/` holds several files. Each must land as its own distinct file
  // rather than one clobbering the other -- a path-flattening bug would
  // collapse them all onto their basenames.
  it('installs multiple reference files side by side without collision', () => {
    for (const config of AGENTS) {
      const refs = SEEDED_REFERENCES;

      mkdirSync(config.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, config);
      install([config.name], FAKE_PKG_ROOT);

      const installedContents = refs.map(ref =>
        readFileSync(join(config.installDir, ref), 'utf8')
      );
      // Every reference file's installed content matches its own source, and
      // no two are identical -- collapsing to the same content is exactly
      // what a path-flattening bug would produce.
      for (const [i, ref] of refs.entries()) {
        const expected = readFileSync(join(FAKE_PKG_ROOT, ref), 'utf8');
        expect(installedContents[i], `${config.name}: ${ref}`).toBe(expected);
      }
      expect(new Set(installedContents).size).toBe(refs.length);
    }
  });

  it('creates install directory if it does not exist', () => {
    const agentName = 'codex';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.detectionDir, { recursive: true });

    install([agentName], FAKE_PKG_ROOT);
    expect(existsSync(config.installDir)).toBe(true);
  });

  it('does NOT create install directory when no source files exist', () => {
    const agentName = 'antigravity';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.detectionDir, { recursive: true });

    rmSync(join(FAKE_PKG_ROOT, 'SKILL.md'));

    const results = install([agentName], FAKE_PKG_ROOT);
    expect(results[0].installed).toHaveLength(0);
    expect(existsSync(config.installDir)).toBe(false);
  });

  it('reports a first install as a change, and a re-run over it as none, rewriting nothing', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);

    const first = install([config.name], FAKE_PKG_ROOT)[0];
    expect(first.changed).toBe(true);

    // A file left alone keeps its modification time; one rewritten with the
    // same bytes would not.
    const files = [...first.installed, join(config.installDir, INSTALL_RECORD)];
    const past = new Date('2020-01-01T00:00:00Z');
    for (const file of files) utimesSync(file, past, past);

    const second = install([config.name], FAKE_PKG_ROOT)[0];
    expect(second.changed).toBe(false);
    expect(second.installed).toEqual(first.installed);
    for (const file of files) {
      expect(statSync(file).mtime.getTime(), `${file} was rewritten`).toBe(past.getTime());
    }
  });

  it('reports a change when an installed file differs from what the skill ships', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);
    const reference = join(config.installDir, SEEDED_REFERENCES[0]);
    const shipped = readFileSync(reference, 'utf8');
    writeFileSync(reference, 'an older version', 'utf8');

    const result = install([config.name], FAKE_PKG_ROOT)[0];

    expect(result.changed).toBe(true);
    expect(readFileSync(reference, 'utf8')).toBe(shipped);
  });

  it('reports a change when all it did was remove a reference the skill dropped', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);
    rmSync(join(FAKE_PKG_ROOT, SEEDED_REFERENCES[1]));

    const result = install([config.name], FAKE_PKG_ROOT)[0];

    expect(result.changed).toBe(true);
    expect(existsSync(join(config.installDir, SEEDED_REFERENCES[1]))).toBe(false);
  });

  it('detects multiple agents when their dirs exist', () => {
    const agentNames = ['claude-code', 'antigravity'] as const;
    for (const name of agentNames) {
      const config = AGENTS.find(a => a.name === name)!;
      mkdirSync(config.detectionDir, { recursive: true });
    }

    const results = install(undefined, FAKE_PKG_ROOT);
    expect(results).toHaveLength(2);
    expect(results.map(r => r.agent).sort()).toEqual(['claude-code', 'antigravity'].sort());
  });
});

describe('uninstall', () => {
  it('returns empty array when no agents are installed', () => {
    const results = uninstall();
    expect(results).toEqual([]);
  });

  it('removes SKILL.md and cleans up empty install dir', () => {
    const agentName = 'claude-code';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.installDir, { recursive: true });
    writeFileSync(join(config.installDir, 'SKILL.md'), SKILL_MD_CONTENT, 'utf8');

    const results = uninstall([agentName]);
    expect(results).toHaveLength(1);
    expect(results[0].removed).toHaveLength(1);
    expect(existsSync(join(config.installDir, 'SKILL.md'))).toBe(false);
    expect(existsSync(config.installDir)).toBe(false);
  });

  it('does not remove install dir when other files remain', () => {
    const agentName = 'opencode';
    const config = AGENTS.find(a => a.name === agentName)!;
    mkdirSync(config.installDir, { recursive: true });
    writeFileSync(join(config.installDir, 'SKILL.md'), SKILL_MD_CONTENT, 'utf8');
    writeFileSync(join(config.installDir, 'other-file.md'), 'other content', 'utf8');

    uninstall([agentName]);
    expect(existsSync(config.installDir)).toBe(true);
    expect(existsSync(join(config.installDir, 'SKILL.md'))).toBe(false);
    expect(existsSync(join(config.installDir, 'other-file.md'))).toBe(true);
  });

  it('uninstalls from all detected agents when no target specified', () => {
    for (const agent of AGENTS) {
      mkdirSync(agent.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, agent);
      install([agent.name], FAKE_PKG_ROOT);
    }

    const results = uninstall();
    expect(results).toHaveLength(AGENTS.length);
    for (const result of results) {
      expect(result.removed.length).toBeGreaterThan(0);
    }
  });

  // Seeding the install dir by hand lets uninstall's own path assumptions go
  // unchallenged — which is exactly how uninstall kept using basename() after
  // install learned to preserve references/. Driving the real install() means
  // the two sides are tested against each other, not against a fixture that
  // agrees with whichever one is wrong.
  it('removes everything install() created, leaving no directory behind', () => {
    for (const config of AGENTS) {
      mkdirSync(config.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, config);
      const installed = install([config.name], FAKE_PKG_ROOT)[0].installed;
      expect(installed.length).toBe(agentFiles(config).length + SEEDED_REFERENCES.length);

      const removed = uninstall([config.name])[0].removed;
      expect(removed.length, `${config.name}: not everything was removed`).toBe(installed.length);

      for (const path of installed) {
        expect(existsSync(path), `${config.name}: ${path} survived uninstall`).toBe(false);
      }
      expect(
        existsSync(config.installDir),
        `${config.name}: install dir survived a full uninstall`
      ).toBe(false);
    }
  });

  // A leftover directory holding references/cli.md but no SKILL.md is worse
  // than either a clean removal or no removal at all: it is a malformed skill
  // folder sitting in the harness's scan path, produced by the command whose
  // job was to clean up.
  it('never leaves a reference file behind without its SKILL.md', () => {
    for (const config of AGENTS) {
      mkdirSync(config.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, config);
      install([config.name], FAKE_PKG_ROOT);
      uninstall([config.name]);

      for (const ref of SEEDED_REFERENCES) {
        expect(
          existsSync(join(config.installDir, ref)),
          `${config.name}: ${ref} survived uninstall`
        ).toBe(false);
      }
    }
  });

  // references/ holds multiple files today. If a user (or another tool)
  // deletes just one of them by hand before uninstall runs, the
  // directory-emptiness check must still see the surviving references, then
  // correctly prune them and the directory once uninstall removes them too
  // -- a state that can only arise once more than one file shares that
  // directory.
  it('removing one reference file by hand does not strand the other or the install dir', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    const refs = SEEDED_REFERENCES;

    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);

    // Simulate a user deleting just one reference file by hand.
    rmSync(join(config.installDir, refs[0]));
    expect(existsSync(join(config.installDir, refs[1]))).toBe(true);

    const results = uninstall([config.name]);
    expect(results[0].removed).not.toContain(join(config.installDir, refs[0]));
    expect(results[0].removed).toContain(join(config.installDir, refs[1]));
    expect(existsSync(config.installDir)).toBe(false);
  });

  // Uninstall must not reach outside what it installed. Pruning "any empty
  // directory" would delete a user's own folder, empty the parent, and take
  // the whole install directory with it — none of it reported in `removed`.
  // Install makes `skills/` on its way to `skills/nodespace/`, so an uninstall
  // that leaves it empty takes it too; the harness's own directory stays.
  it('removes the skills directory install created, and never the harness directory', () => {
    for (const config of AGENTS) {
      mkdirSync(config.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, config);
      install([config.name], FAKE_PKG_ROOT);

      uninstall([config.name]);

      expect(
        existsSync(join(config.installDir, '..')),
        `${config.name}: the empty skills directory survived uninstall`
      ).toBe(false);
      expect(existsSync(config.detectionDir), `${config.name}: harness directory removed`).toBe(true);
    }
  });

  it('keeps the skills directory when another skill is installed beside ours', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);
    const otherSkill = join(config.installDir, '..', 'someone-elses-skill');
    mkdirSync(otherSkill, { recursive: true });

    uninstall([config.name]);

    expect(existsSync(config.installDir)).toBe(false);
    expect(existsSync(otherSkill)).toBe(true);
  });

  // Removing more than we installed is a worse failure than leaving something
  // behind.
  it('never deletes directories it did not install', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);

    const userDir = join(config.installDir, 'user-scripts');
    mkdirSync(userDir, { recursive: true });

    uninstall([config.name]);

    expect(existsSync(userDir), 'uninstall deleted a user-created directory').toBe(true);
    expect(existsSync(config.installDir)).toBe(true);
    expect(existsSync(join(config.installDir, 'references'))).toBe(false);
    expect(existsSync(join(config.installDir, 'SKILL.md'))).toBe(false);
  });

  it('preserves user files inside a directory it does own', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);

    const userNote = join(config.installDir, 'references', 'my-notes.md');
    writeFileSync(userNote, 'user content', 'utf8');

    uninstall([config.name]);

    expect(existsSync(userNote), 'uninstall deleted a user file inside references/').toBe(true);
  });

  it('still removes the install dir when a file was already deleted by hand', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install([config.name], FAKE_PKG_ROOT);

    rmSync(join(config.installDir, 'SKILL.md'));
    uninstall([config.name]);

    expect(existsSync(config.installDir)).toBe(false);
  });
});

describe('listReferenceFiles', () => {
  it('returns the *.md files directly under references/ as package-root-relative paths', () => {
    const refs = join(FAKE_PKG_ROOT, 'references');
    mkdirSync(join(refs, 'nested'), { recursive: true });
    mkdirSync(join(refs, 'folder.md'));
    writeFileSync(join(refs, 'notes.txt'), 'not markdown', 'utf8');
    writeFileSync(join(refs, 'nested', 'deep.md'), 'not directly under references/', 'utf8');
    writeFileSync(join(refs, 'b.md'), 'b', 'utf8');
    writeFileSync(join(refs, 'a.md'), 'a', 'utf8');

    expect(listReferenceFiles(FAKE_PKG_ROOT)).toEqual(['references/a.md', 'references/b.md']);
  });

  it('sorts them, whatever order the filesystem lists them in', async () => {
    const refs = join(FAKE_PKG_ROOT, 'references');
    mkdirSync(refs, { recursive: true });
    for (const name of ['delta', 'alpha', 'echo', 'bravo']) {
      writeFileSync(join(refs, `${name}.md`), name, 'utf8');
    }
    // Reverse of whatever this platform lists: sorted or not, the result must be.
    const { listReferenceFiles: reversed } = await importInstallerWithFs(actual => ({
      readdirSync: ((...args: Parameters<typeof readdirSync>) =>
        [...(actual.readdirSync(...args) as unknown[])].reverse()) as unknown as typeof readdirSync,
    }));

    expect(reversed(FAKE_PKG_ROOT)).toEqual([
      'references/alpha.md',
      'references/bravo.md',
      'references/delta.md',
      'references/echo.md',
    ]);
  });

  it('returns an empty list when the package root has no references/ directory', () => {
    expect(listReferenceFiles(FAKE_PKG_ROOT)).toEqual([]);
  });

  it('returns an empty list when references is a file rather than a directory', () => {
    writeFileSync(join(FAKE_PKG_ROOT, 'references'), 'not a directory', 'utf8');
    expect(listReferenceFiles(FAKE_PKG_ROOT)).toEqual([]);
  });

  // An unreadable references/ is not "a skill with no references": reading it
  // as empty would make a reinstall delete every reference it installed before.
  it('rethrows a read error other than a missing directory, leaving installed references in place', async () => {
    const claude = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(claude.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, claude);
    install(['claude-code'], FAKE_PKG_ROOT);

    const { listReferenceFiles: failing, install: failingInstall } = await importInstallerWithFs(actual => ({
      readdirSync: failingOn(actual.readdirSync, () => true),
    }));

    expect(() => failing(FAKE_PKG_ROOT)).toThrow(/EACCES/);
    expect(() => failingInstall(['claude-code'], FAKE_PKG_ROOT)).toThrow(/EACCES/);
    for (const ref of SEEDED_REFERENCES) {
      expect(existsSync(join(claude.installDir, ref)), ref).toBe(true);
    }
  });
});

describe('references and the install record', () => {
  const claude = AGENTS.find(a => a.name === 'claude-code')!;
  const OUTSIDE_INSTALL_DIR = join(claude.installDir, '..', 'outside.md');

  /** Writes a file (and any parent directories) under `dir`. */
  function plant(dir: string, rel: string, content = `planted ${rel}`): string {
    const path = join(dir, rel);
    mkdirSync(join(path, '..'), { recursive: true });
    writeFileSync(path, content, 'utf8');
    return path;
  }

  function installClaude(): void {
    mkdirSync(claude.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, claude);
    install(['claude-code'], FAKE_PKG_ROOT);
  }

  /** Overwrites the record with exactly these entries, whatever they are. */
  function writeRecord(files: unknown[]): void {
    writeFileSync(join(claude.installDir, INSTALL_RECORD), JSON.stringify({ files }), 'utf8');
  }

  describe('install', () => {
    it('copies every references/*.md in the package root, including one no agent config names', () => {
      expect(AGENTS.flatMap(a => agentFiles(a)).some(([src]) => src.includes('extra-playbook'))).toBe(false);

      for (const config of AGENTS) {
        mkdirSync(config.detectionDir, { recursive: true });
        seedPkgRoot(FAKE_PKG_ROOT, config);
        install([config.name], FAKE_PKG_ROOT);

        for (const ref of SEEDED_REFERENCES) {
          expect(readFileSync(join(config.installDir, ref), 'utf8'), `${config.name}: ${ref}`).toBe(
            readFileSync(join(FAKE_PKG_ROOT, ref), 'utf8')
          );
        }
      }
    });

    it('writes a record listing exactly the files it wrote', () => {
      installClaude();

      const record = readRecord(claude);
      expect(Object.keys(record)).toEqual(['files']);
      expect(record.files).toEqual(seededInstallFiles(claude));
      // The record describes the skill's files; it is not one of them.
      expect(record.files).not.toContain(INSTALL_RECORD);
    });

    it('does not report the record among the files it installed', () => {
      mkdirSync(claude.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, claude);

      const [result] = install(['claude-code'], FAKE_PKG_ROOT);

      expect(result.installed.map(path => basename(path))).not.toContain(INSTALL_RECORD);
      expect(result.installed).toHaveLength(agentFiles(claude).length + SEEDED_REFERENCES.length);
    });

    it('installs none of the plugin when the package is missing one of its files', () => {
      installClaude();
      rmSync(join(FAKE_PKG_ROOT, claude.plugin!.dir, 'hooks', 'register.ts'));

      const [result] = install(['claude-code'], FAKE_PKG_ROOT);

      for (const file of claude.plugin!.files) {
        expect(existsSync(join(claude.installDir, file)), `${file} was left installed`).toBe(false);
      }
      expect(existsSync(join(claude.installDir, 'hooks'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(true);
      expect(result.changed).toBe(true);
      expect(readRecord(claude).files).toEqual(['SKILL.md', ...SEEDED_REFERENCES].sort());
    });

    // An earlier installer put a harness file at the root of each install
    // directory and recorded it. This skill ships none, so the record is what
    // gets it removed.
    it('removes a recorded harness file the skill no longer ships, for every agent', () => {
      for (const config of AGENTS) {
        mkdirSync(config.detectionDir, { recursive: true });
        seedPkgRoot(FAKE_PKG_ROOT, config);
        install([config.name], FAKE_PKG_ROOT);
        const retired = join(config.installDir, 'nodespace-plugin.ts');
        writeFileSync(retired, '// registered tools no harness loaded', 'utf8');
        writeFileSync(
          join(config.installDir, INSTALL_RECORD),
          JSON.stringify({ files: [...readRecord(config).files, 'nodespace-plugin.ts'] }),
          'utf8'
        );

        const [result] = install([config.name], FAKE_PKG_ROOT);

        expect(existsSync(retired), `${config.name}: the retired file survived a reinstall`).toBe(false);
        expect(result.changed).toBe(true);
        expect(readRecord(config).files).toEqual(seededInstallFiles(config));
      }
    });

    it('deletes an installed reference the new skill no longer ships and updates the record', () => {
      installClaude();
      rmSync(join(FAKE_PKG_ROOT, 'references', 'extra-playbook.md'));

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(join(claude.installDir, 'references', 'extra-playbook.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(true);
      expect(readRecord(claude).files).toEqual(
        seededInstallFiles(claude).filter(rel => rel !== 'references/extra-playbook.md')
      );
    });

    it('prunes references/ when the new skill ships no references at all', () => {
      installClaude();
      rmSync(join(FAKE_PKG_ROOT, 'references'), { recursive: true });

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(join(claude.installDir, 'references'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(true);
    });

    it('leaves a user\'s own file in references/ alone when it deletes a dropped reference', () => {
      installClaude();
      const mine = plant(join(claude.installDir, 'references'), 'mine.md');
      rmSync(join(FAKE_PKG_ROOT, 'references'), { recursive: true });

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(mine)).toBe(true);
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(false);
    });

    it('never deletes a stale record entry that points outside the install directory', () => {
      installClaude();
      plant(join(OUTSIDE_INSTALL_DIR, '..'), 'outside.md', 'not ours');
      writeRecord(['SKILL.md', '../outside.md', join(OUTSIDE_INSTALL_DIR)]);

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(readFileSync(OUTSIDE_INSTALL_DIR, 'utf8')).toBe('not ours');
    });

    // An entry spelled differently from what was just written must not turn
    // that file into a "stale" one: the reinstall would delete its own output.
    it('keeps a file it just wrote even when the previous record spells its path differently', () => {
      installClaude();
      writeRecord(['./SKILL.md', 'references//cli.md']);

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(true);
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(true);
    });

    // A package with no SKILL.md at all is broken, not a skill that shrank to
    // nothing. Treating it as the latter would wipe a working install.
    it('leaves an existing install and its record untouched when the package has nothing to install', () => {
      installClaude();
      const before = readFileSync(join(claude.installDir, INSTALL_RECORD), 'utf8');
      const emptyRoot = join(TMP, 'empty-pkg');
      mkdirSync(emptyRoot, { recursive: true });

      const [result] = install(['claude-code'], emptyRoot);

      expect(result.installed).toEqual([]);
      expect(readFileSync(join(claude.installDir, INSTALL_RECORD), 'utf8')).toBe(before);
      for (const rel of seededInstallFiles(claude)) {
        expect(existsSync(join(claude.installDir, rel)), rel).toBe(true);
      }
    });

    // A skill is discovered by its SKILL.md. A package that ships references and a
    // harness plugin but no SKILL.md must not replace a working install with a
    // folder the harness cannot use, nor drop files next to it.
    it('installs nothing, and leaves an existing install alone, when the package has no SKILL.md', () => {
      installClaude();
      const before = readFileSync(join(claude.installDir, INSTALL_RECORD), 'utf8');
      const skillBefore = readFileSync(join(claude.installDir, 'SKILL.md'), 'utf8');
      rmSync(join(FAKE_PKG_ROOT, 'SKILL.md'));
      plant(FAKE_PKG_ROOT, 'references/new-only.md');

      const [result] = install(['claude-code'], FAKE_PKG_ROOT);

      expect(result.installed).toEqual([]);
      expect(readFileSync(join(claude.installDir, 'SKILL.md'), 'utf8')).toBe(skillBefore);
      expect(readFileSync(join(claude.installDir, INSTALL_RECORD), 'utf8')).toBe(before);
      expect(existsSync(join(claude.installDir, 'references', 'new-only.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(true);
    });

    // A stale file that cannot be deleted (read-only directory, a file Windows has
    // locked) must stay in the record. Dropping it would orphan it: no later
    // install or uninstall would know it is ours, and `references/` and the
    // install directory would be stranded around it.
    it('keeps a stale file it could not delete in the record, warns, and still finishes the install', async () => {
      installClaude();
      rmSync(join(FAKE_PKG_ROOT, 'references', 'extra-playbook.md'));
      rmSync(join(FAKE_PKG_ROOT, 'references', 'cli.md'));
      const { install: failingInstall } = await importInstallerWithFs(actual => ({
        rmSync: failingOn(actual.rmSync, path => path.endsWith('extra-playbook.md')),
      }));
      const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);

      try {
        failingInstall(['claude-code'], FAKE_PKG_ROOT);
        expect(stderr).toHaveBeenCalledWith(expect.stringContaining('could not remove'));
      } finally {
        stderr.mockRestore();
      }

      // The one that could be deleted went; the other stays, and stays recorded.
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'references', 'extra-playbook.md'))).toBe(true);
      expect(readRecord(claude).files).toContain('references/extra-playbook.md');
      expect(readRecord(claude).files).not.toContain('references/cli.md');

      // ...so an uninstall still finds it, and leaves nothing stranded.
      uninstall(['claude-code'], FAKE_PKG_ROOT);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    // Most installs predate the record (the first-launch install runs once), so
    // the first reinstall over one has no record to diff against. It is read as
    // holding what the installer wrote before records existed.
    it('over an install with no record, removes pre-record references the new skill no longer ships', () => {
      for (const rel of ['SKILL.md', ...PRE_RECORD_REFERENCES]) {
        plant(claude.installDir, rel);
      }
      mkdirSync(claude.detectionDir, { recursive: true });
      plant(FAKE_PKG_ROOT, 'references/cli.md');

      install(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(true);
      expect(existsSync(join(claude.installDir, 'references', 'graph-authored-guidance.md'))).toBe(false);
      expect(readRecord(claude).files).toEqual(['SKILL.md', 'references/cli.md']);
    });

    it('installs every reference in the real skill package, and every link in the installed SKILL.md resolves', () => {
      const realRoot = join(import.meta.dirname, '../..');
      mkdirSync(claude.detectionDir, { recursive: true });

      install(['claude-code'], realRoot);

      const shipped = readdirSync(join(realRoot, 'references')).filter(name => name.endsWith('.md'));
      expect(shipped.length).toBeGreaterThan(0);
      for (const name of shipped) {
        expect(existsSync(join(claude.installDir, 'references', name)), name).toBe(true);
      }

      const body = readFileSync(join(claude.installDir, 'SKILL.md'), 'utf8');
      const links = [...new Set(body.match(/references\/[A-Za-z0-9._-]+\.md/g))];
      expect(links.length).toBeGreaterThan(0);
      for (const link of links) {
        expect(existsSync(join(claude.installDir, link)), `SKILL.md links ${link}`).toBe(true);
      }
    });
  });

  describe('uninstall', () => {
    it('with a record, removes exactly the recorded files and the record, leaving no install directory', () => {
      installClaude();
      // A recorded file that neither the agent's own files nor the package root
      // names: only the record can say it is ours.
      const recordedOnly = plant(claude.installDir, 'references/retired-playbook.md');
      writeRecord([...readRecord(claude).files, 'references/retired-playbook.md']);

      const [result] = uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(result.removed).toContain(recordedOnly);
      expect(result.removed).toHaveLength(seededInstallFiles(claude).length + 1);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    it('leaves no file behind from a package root shaped like an extension-staged skill', () => {
      mkdirSync(claude.detectionDir, { recursive: true });
      seedPkgRoot(FAKE_PKG_ROOT, claude);
      // What a build with a skill extension stages: core's references plus one
      // it adds, and a fragment at the end of SKILL.md pointing at it.
      plant(FAKE_PKG_ROOT, 'references/fixture-guide.md', '# Fixture guide');
      writeFileSync(
        join(FAKE_PKG_ROOT, 'SKILL.md'),
        `${SKILL_MD_CONTENT}\n\nRead references/fixture-guide.md.\n`,
        'utf8'
      );

      install(['claude-code'], FAKE_PKG_ROOT);

      const added = join(claude.installDir, 'references', 'fixture-guide.md');
      expect(readFileSync(added, 'utf8')).toBe('# Fixture guide');
      expect(readRecord(claude).files).toContain('references/fixture-guide.md');

      const [result] = uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(result.removed).toContain(added);
      expect(existsSync(added)).toBe(false);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    it('with a record, keeps a user\'s own file in references/, and with it references/ and the install directory', () => {
      installClaude();
      const mine = plant(join(claude.installDir, 'references'), 'mine.md', 'user content');

      uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(readFileSync(mine, 'utf8')).toBe('user content');
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, INSTALL_RECORD))).toBe(false);
    });

    it('with a record, ignores an entry that points outside the install directory', () => {
      installClaude();
      plant(join(OUTSIDE_INSTALL_DIR, '..'), 'outside.md', 'not ours');
      writeRecord(['SKILL.md', '../outside.md', OUTSIDE_INSTALL_DIR, '../../../not-there.md']);

      const [result] = uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(readFileSync(OUTSIDE_INSTALL_DIR, 'utf8')).toBe('not ours');
      expect(result.removed).toEqual([join(claude.installDir, 'SKILL.md')]);
    });

    it('with a record, ignores an entry that names a directory', () => {
      installClaude();
      const userFile = plant(join(claude.installDir, 'user-dir'), 'keep.md');
      writeRecord(['user-dir', 'SKILL.md']);

      uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(userFile)).toBe(true);
    });

    // The entry looks contained (`references/victim.md`), but `references` is a
    // symlink to a directory outside the install directory.
    it.skipIf(process.platform === 'win32')(
      'with a record, never deletes through a symlinked directory that leads outside',
      () => {
        installClaude();
        const elsewhere = join(TMP, 'elsewhere');
        const victim = plant(elsewhere, 'victim.md', 'not ours');
        rmSync(join(claude.installDir, 'references'), { recursive: true });
        symlinkSync(elsewhere, join(claude.installDir, 'references'));
        writeRecord(['references/victim.md']);

        uninstall(['claude-code'], FAKE_PKG_ROOT);

        expect(readFileSync(victim, 'utf8')).toBe('not ours');
      }
    );

    it('treats an unreadable or malformed record as no record, and ignores non-string entries', () => {
      const badRecords: Array<[string, string]> = [
        ['not json', '{ nope'],
        ['files is not an array', JSON.stringify({ files: 'SKILL.md' })],
        ['a bare string', JSON.stringify('SKILL.md')],
        ['null', 'null'],
      ];
      for (const [label, content] of badRecords) {
        plant(claude.installDir, 'SKILL.md');
        plant(claude.installDir, 'references/graph-authored-guidance.md');
        writeFileSync(join(claude.installDir, INSTALL_RECORD), content, 'utf8');

        uninstall(['claude-code'], FAKE_PKG_ROOT);

        expect(existsSync(claude.installDir), `${label}: install dir survived`).toBe(false);
      }

      plant(claude.installDir, 'SKILL.md');
      plant(claude.installDir, 'other.md', 'not in the record');
      writeRecord([1, null, { file: 'other.md' }, 'SKILL.md']);
      uninstall(['claude-code'], FAKE_PKG_ROOT);
      expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(false);
      expect(existsSync(join(claude.installDir, 'other.md'))).toBe(true);
    });

    // Without a record the install predates it: what the installer wrote then
    // was SKILL.md and a fixed set of references, including ones the skill has
    // since stopped shipping.
    it('without a record, removes the pre-record references, including one the package no longer ships, and prunes the directory', () => {
      for (const rel of ['SKILL.md', ...PRE_RECORD_REFERENCES]) {
        plant(claude.installDir, rel);
      }
      // The package root ships only SKILL.md: none of the references exist there.

      const [result] = uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(result.removed).toHaveLength(1 + PRE_RECORD_REFERENCES.length);
      expect(existsSync(join(claude.installDir, 'references'))).toBe(false);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    it('without a record, also removes a reference the package root ships that the pre-record list does not name', () => {
      plant(claude.installDir, 'SKILL.md');
      const extra = plant(claude.installDir, 'references/extra-playbook.md');
      plant(FAKE_PKG_ROOT, 'references/extra-playbook.md');

      const [result] = uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(result.removed).toContain(extra);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    it('without a record, keeps a user\'s own file in references/, and with it references/ and the install directory', () => {
      plant(claude.installDir, 'SKILL.md');
      plant(claude.installDir, 'references/cli.md');
      const mine = plant(claude.installDir, 'references/mine.md');

      uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(mine)).toBe(true);
      expect(existsSync(join(claude.installDir, 'references', 'cli.md'))).toBe(false);
    });
  });

  describe('uninstall, filesystem edge cases', () => {
    // A dotfile manager can link the skill's directory in from a repository. An
    // emptied symlink cannot be rmdir'd, and the uninstall must neither crash
    // nor skip the agents after it.
    it.skipIf(process.platform === 'win32')(
      'finishes, and moves on to the next agent, when the install directory is a symlink',
      () => {
        const codex = AGENTS.find(a => a.name === 'codex')!;
        for (const config of [claude, codex]) {
          mkdirSync(config.detectionDir, { recursive: true });
          seedPkgRoot(FAKE_PKG_ROOT, config);
        }
        const linked = join(TMP, 'dotfiles', 'nodespace');
        mkdirSync(linked, { recursive: true });
        mkdirSync(join(claude.installDir, '..'), { recursive: true });
        symlinkSync(linked, claude.installDir);
        install(['claude-code', 'codex'], FAKE_PKG_ROOT);
        expect(existsSync(join(linked, 'SKILL.md'))).toBe(true);

        const results = uninstall(['claude-code', 'codex'], FAKE_PKG_ROOT);

        expect(results.map(r => r.agent)).toEqual(['claude-code', 'codex']);
        expect(readdirSync(linked)).toEqual([]);
        expect(existsSync(codex.installDir)).toBe(false);
      }
    );

    // The record names files; a link the user put where one of them goes is
    // unlinked, never followed, so what it points at survives.
    it.skipIf(process.platform === 'win32')(
      'unlinks a recorded symlink without touching what it points at',
      () => {
        installClaude();
        const external = plant(join(TMP, 'external'), 'SKILL.md', 'lives elsewhere');
        const externalDir = join(TMP, 'external-dir');
        const keepMe = plant(externalDir, 'keep.md', 'lives elsewhere too');
        rmSync(join(claude.installDir, 'SKILL.md'));
        symlinkSync(external, join(claude.installDir, 'SKILL.md'));
        symlinkSync(externalDir, join(claude.installDir, 'linked-dir'));
        writeRecord(['SKILL.md', 'linked-dir']);

        uninstall(['claude-code'], FAKE_PKG_ROOT);

        expect(existsSync(join(claude.installDir, 'SKILL.md'))).toBe(false);
        expect(existsSync(join(claude.installDir, 'linked-dir'))).toBe(false);
        expect(readFileSync(external, 'utf8')).toBe('lives elsewhere');
        expect(readFileSync(keepMe, 'utf8')).toBe('lives elsewhere too');
      }
    );

    // The recorded files are already gone, so there is nothing to delete, but
    // the empty directories they leave behind are still this installer's.
    it('prunes the directories of recorded files the user already deleted by hand', () => {
      installClaude();
      for (const rel of readRecord(claude).files) rmSync(join(claude.installDir, rel));

      uninstall(['claude-code'], FAKE_PKG_ROOT);

      expect(existsSync(claude.installDir)).toBe(false);
    });

    // An unreadable references/ in the package being uninstalled must not stop
    // the removal of the files the pre-record list names.
    it('without a record, still removes the pre-record files when the package references cannot be read', async () => {
      for (const rel of ['SKILL.md', ...PRE_RECORD_REFERENCES]) plant(claude.installDir, rel);
      const { uninstall: failingUninstall } = await importInstallerWithFs(actual => ({
        readdirSync: failingOn(actual.readdirSync, path => path.endsWith('references') && path.startsWith(FAKE_PKG_ROOT)),
      }));

      const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
      let result;
      try {
        [result] = failingUninstall(['claude-code'], FAKE_PKG_ROOT);
        // The cleanup is partial by construction, so it says so.
        expect(stderr).toHaveBeenCalledWith(expect.stringContaining("could not read the skill's references"));
      } finally {
        stderr.mockRestore();
      }

      expect(result!.removed).toHaveLength(1 + PRE_RECORD_REFERENCES.length);
      expect(existsSync(claude.installDir)).toBe(false);
    });

    // On Windows, rmdir on a directory symlink removes the link even though the
    // directory behind it is not empty, unlike POSIX (ENOTDIR). This emulates
    // that, so the guard is exercised on every platform the tests run on.
    it.skipIf(process.platform === 'win32')(
      'never prunes a symlinked directory, even where rmdir would remove the link',
      async () => {
        installClaude();
        const elsewhere = join(TMP, 'elsewhere-references');
        mkdirSync(elsewhere, { recursive: true });
        for (const rel of readRecord(claude).files.filter(rel => rel.startsWith('references/'))) {
          plant(elsewhere, rel.slice('references/'.length));
        }
        rmSync(join(claude.installDir, 'references'), { recursive: true });
        symlinkSync(elsewhere, join(claude.installDir, 'references'));
        const { uninstall: windowsUninstall } = await importInstallerWithFs(actual => ({
          rmdirSync: ((path: string) =>
            actual.lstatSync(path).isSymbolicLink() ? actual.unlinkSync(path) : actual.rmdirSync(path)) as typeof rmdirSync,
        }));

        windowsUninstall(['claude-code'], FAKE_PKG_ROOT);

        expect(lstatSync(join(claude.installDir, 'references')).isSymbolicLink()).toBe(true);
        expect(readdirSync(elsewhere).length).toBeGreaterThan(0);
      }
    );
  });

  describe('Claude Code plugin-managed reconciliation', () => {
    // The cleanup is an uninstall of a possibly record-less copy, so it needs
    // to know which package root the reinstall is running from.
    it('cleans up a record-less copy using the package root install() was given', () => {
      plant(claude.installDir, 'SKILL.md');
      plant(claude.installDir, 'references/extra-playbook.md');
      plant(FAKE_PKG_ROOT, 'references/extra-playbook.md');
      mkdirSync(join(claude.detectionDir, 'plugins'), { recursive: true });
      writeFileSync(
        join(claude.detectionDir, 'plugins', 'installed_plugins.json'),
        JSON.stringify({ version: 2, plugins: { 'nodespace@nodespace-skill': [{ scope: 'user' }] } }),
        'utf8'
      );

      const [result] = install(['claude-code'], FAKE_PKG_ROOT);

      expect(result.skipReason).toBe('plugin-managed');
      expect(existsSync(claude.installDir)).toBe(false);
    });
  });

  describe('checkInstalled', () => {
    it('does not count an install directory holding only the record', () => {
      mkdirSync(claude.installDir, { recursive: true });
      writeRecord(['SKILL.md']);

      expect(checkInstalled(['claude-code'])).toEqual([]);
    });
  });
});

describe('detectAgents', () => {
  it('returns empty array when no agent is present', () => {
    expect(detectAgents()).toEqual([]);
  });

  it('reports an agent whose config dir exists, before anything is installed', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });

    // The whole point of this being separate from checkInstalled: detected,
    // but nothing installed into it yet. This is the state the onboarding
    // wizard asks its question in.
    expect(detectAgents()).toEqual(['claude-code']);
    expect(checkInstalled()).toEqual([]);
  });

  it('reports every present agent, in AGENTS order', () => {
    for (const name of ['claude-code', 'antigravity', 'codex']) {
      mkdirSync(AGENTS.find(a => a.name === name)!.detectionDir, { recursive: true });
    }

    // AGENTS order (claude-code, codex, antigravity, ...), not the order
    // the dirs were created in -- so the wizard's wording is stable rather
    // than dependent on filesystem incidentals.
    expect(detectAgents()).toEqual(['claude-code', 'codex', 'antigravity']);
  });

  it('does not report an agent that is absent', () => {
    mkdirSync(AGENTS.find(a => a.name === 'opencode')!.detectionDir, { recursive: true });

    expect(detectAgents()).toEqual(['opencode']);
  });
});

describe('checkInstalled', () => {
  it('returns empty array when nothing is installed', () => {
    expect(checkInstalled()).toEqual([]);
  });

  it('reports an agent as installed once install() has written SKILL.md', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    install(['claude-code'], FAKE_PKG_ROOT);

    expect(checkInstalled(['claude-code'])).toEqual(['claude-code']);
  });

  // The staleness scenario this exists to catch: agents_installed was
  // persisted by a real install, then the user deleted the harness's skill
  // directory by hand (or the harness itself) outside NodeSpace entirely.
  // checkInstalled must reflect the filesystem as it is now, not the stale
  // persisted claim.
  it('no longer reports an agent once its skill directory is deleted by hand', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.detectionDir, { recursive: true });
    install(['claude-code'], FAKE_PKG_ROOT);
    expect(checkInstalled(['claude-code'])).toEqual(['claude-code']);

    rmSync(config.installDir, { recursive: true, force: true });

    expect(checkInstalled(['claude-code'])).toEqual([]);
  });

  it('filters a mixed list down to only the agents actually present on disk', () => {
    const claudeCode = AGENTS.find(a => a.name === 'claude-code')!;
    const antigravity = AGENTS.find(a => a.name === 'antigravity')!;
    mkdirSync(claudeCode.detectionDir, { recursive: true });
    mkdirSync(antigravity.detectionDir, { recursive: true });
    install(['claude-code', 'antigravity'], FAKE_PKG_ROOT);

    rmSync(antigravity.installDir, { recursive: true, force: true });

    expect(checkInstalled(['claude-code', 'antigravity'])).toEqual(['claude-code']);
  });

  it('defaults to checking every configured agent when no target list is given', () => {
    const claudeCode = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(claudeCode.detectionDir, { recursive: true });
    install(['claude-code'], FAKE_PKG_ROOT);

    expect(checkInstalled()).toEqual(['claude-code']);
  });

  // An empty or partially-cleaned install directory (SKILL.md removed but the
  // directory itself left behind, e.g. an interrupted manual deletion) must
  // not read as "installed" — the check is specifically for SKILL.md, not
  // merely existsSync(installDir).
  it('does not count a directory that exists but has no SKILL.md as installed', () => {
    const config = AGENTS.find(a => a.name === 'claude-code')!;
    mkdirSync(config.installDir, { recursive: true });
    writeFileSync(join(config.installDir, 'other-file.md'), 'stray file', 'utf8');

    expect(checkInstalled(['claude-code'])).toEqual([]);
  });
});

describe('claudeCodePluginManagedSkillExists', () => {
  const claudeConfigDir = join(TMP, '.claude');
  const registryPath = join(claudeConfigDir, 'plugins', 'installed_plugins.json');

  function writeRegistry(plugins: Record<string, unknown>): void {
    mkdirSync(join(claudeConfigDir, 'plugins'), { recursive: true });
    writeFileSync(registryPath, JSON.stringify({ version: 2, plugins }), 'utf8');
  }

  it('returns false when the registry file does not exist at all', () => {
    expect(claudeCodePluginManagedSkillExists(claudeConfigDir)).toBe(false);
  });

  it('returns true when a nodespace plugin key is registered', () => {
    writeRegistry({ 'nodespace@nodespace-skill': [{ scope: 'user' }] });
    expect(claudeCodePluginManagedSkillExists(claudeConfigDir)).toBe(true);
  });

  // The marketplace half of the key is whatever label the user's own Claude
  // Code registered it under locally -- matching a fixed marketplace name
  // would miss a real install under a different one.
  it('matches regardless of which marketplace the plugin was registered under', () => {
    writeRegistry({ 'nodespace@some-other-marketplace': [{ scope: 'user' }] });
    expect(claudeCodePluginManagedSkillExists(claudeConfigDir)).toBe(true);
  });

  // Mirrors the shape of a real local installed_plugins.json containing only
  // unrelated plugins (verified against an actual file during design).
  it('returns false when only unrelated plugins are registered', () => {
    writeRegistry({ 'rust-analyzer-lsp@claude-plugins-official': [{ scope: 'user' }] });
    expect(claudeCodePluginManagedSkillExists(claudeConfigDir)).toBe(false);
  });

  it('fails open (returns false) on a malformed registry file rather than throwing', () => {
    mkdirSync(join(claudeConfigDir, 'plugins'), { recursive: true });
    writeFileSync(registryPath, '{ not valid json', 'utf8');
    expect(claudeCodePluginManagedSkillExists(claudeConfigDir)).toBe(false);
  });
});

describe('install — Claude Code plugin-managed reconciliation', () => {
  const config = AGENTS.find(a => a.name === 'claude-code')!;
  const registryPath = join(config.detectionDir, 'plugins', 'installed_plugins.json');

  function markPluginManaged(): void {
    mkdirSync(join(config.detectionDir, 'plugins'), { recursive: true });
    writeFileSync(
      registryPath,
      JSON.stringify({ version: 2, plugins: { 'nodespace@nodespace-skill': [{ scope: 'user' }] } }),
      'utf8'
    );
  }

  it('skips writing files and reports skipReason plugin-managed', () => {
    mkdirSync(config.detectionDir, { recursive: true });
    markPluginManaged();

    const results = install(['claude-code'], FAKE_PKG_ROOT);
    expect(results).toHaveLength(1);
    expect(results[0].installed).toEqual([]);
    expect(results[0].skipReason).toBe('plugin-managed');
    expect(existsSync(config.installDir)).toBe(false);
  });

  // An app-installed copy from before this reconciliation existed must not
  // be left behind once the marketplace copy is authoritative -- otherwise
  // "no NEW copy" would still leave the old one in place, and Claude Code
  // would still see the skill twice.
  it('cleans up a pre-existing app-installed copy once the marketplace copy is detected', () => {
    mkdirSync(config.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, config);
    install(['claude-code'], FAKE_PKG_ROOT);
    expect(existsSync(join(config.installDir, 'SKILL.md'))).toBe(true);

    markPluginManaged();
    const results = install(['claude-code'], FAKE_PKG_ROOT);

    expect(results[0].installed).toEqual([]);
    expect(results[0].skipReason).toBe('plugin-managed');
    expect(existsSync(config.installDir)).toBe(false);
  });

  it('does not block other agents from installing normally', () => {
    const antigravity = AGENTS.find(a => a.name === 'antigravity')!;
    mkdirSync(config.detectionDir, { recursive: true });
    markPluginManaged();
    mkdirSync(antigravity.detectionDir, { recursive: true });
    seedPkgRoot(FAKE_PKG_ROOT, antigravity);

    const results = install(['claude-code', 'antigravity'], FAKE_PKG_ROOT);
    const antigravityResult = results.find(r => r.agent === 'antigravity')!;
    expect(antigravityResult.installed.length).toBe(agentFiles(antigravity).length + SEEDED_REFERENCES.length);
    expect(antigravityResult.skipReason).toBeUndefined();
  });
});

describe('isNodespaceBinaryOnPath', () => {
  it('returns true when execFileSync exits 0', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => Buffer.from('nodespace 0.1.0\n'),
    }));
    const { isNodespaceBinaryOnPath: check } = await import('../installer.js');
    expect(check()).toBe(true);
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });

  it('returns false when execFileSync throws (binary not found)', async () => {
    // Patch child_process on the installer module's live reference by re-importing
    // after hoisting the mock.  vi.doMock + resetModules lets us control this.
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => { throw Object.assign(new Error('ENOENT'), { code: 'ENOENT' }); },
    }));
    const { isNodespaceBinaryOnPath: check } = await import('../installer.js');
    expect(check()).toBe(false);
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });
});

describe('install PATH warning', () => {
  it('writes a warning to stderr when nodespace is not on PATH', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => { throw Object.assign(new Error('ENOENT'), { code: 'ENOENT' }); },
    }));
    const { install: freshInstall } = await import('../installer.js');

    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    freshInstall([], FAKE_PKG_ROOT);
    expect(stderrSpy).toHaveBeenCalledWith(expect.stringContaining('nodespace` is not on $PATH'));

    stderrSpy.mockRestore();
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });

  it('does not write a warning when nodespace is on PATH', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => Buffer.from('nodespace 0.1.0\n'),
    }));
    const { install: freshInstall } = await import('../installer.js');

    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    freshInstall([], FAKE_PKG_ROOT);
    expect(stderrSpy).not.toHaveBeenCalled();

    stderrSpy.mockRestore();
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });
});

describe('CLAUDE_CONFIG_DIR', () => {
  afterEach(() => {
    delete process.env.CLAUDE_CONFIG_DIR;
    vi.resetModules();
  });

  it('claude-code detects + installs into $CLAUDE_CONFIG_DIR when set', async () => {
    const custom = join(TMP, 'custom-claude-profile');
    process.env.CLAUDE_CONFIG_DIR = custom;
    vi.resetModules();
    const { AGENTS: A } = await import('../agents.js');
    const cc = A.find(a => a.name === 'claude-code')!;
    expect(cc.detectionDir).toBe(custom);
    expect(cc.installDir).toBe(join(custom, 'skills', 'nodespace'));
  });

  it('claude-code falls back to ~/.claude when $CLAUDE_CONFIG_DIR is unset', async () => {
    delete process.env.CLAUDE_CONFIG_DIR;
    vi.resetModules();
    const { AGENTS: A } = await import('../agents.js');
    const cc = A.find(a => a.name === 'claude-code')!;
    expect(cc.installDir).toBe(join(TMP, '.claude', 'skills', 'nodespace'));
  });
});
