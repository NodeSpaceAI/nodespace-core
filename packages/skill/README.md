# @nodespaceai/skill

Installs the NodeSpace Agent Skill into PTY agents (Claude Code, Codex, Gemini CLI, OpenCode).

**This package is not published to npm.** `@nodespaceai/skill` never existed
on the npm registry, and it is not going to: the NodeSpace desktop app is the
one thing that installs this package's output, and it does so by running a
built installer directly (see
`packages/desktop-app/src-tauri/src/skill_setup.rs`) — never `npx`/`npm`, so
publishing to npm was never actually required for the app's own install path.

If you're using an external agent harness yourself (not launched via the
NodeSpace app) and want the skill without installing NodeSpace first, import
the generated public repo instead:
**[NodeSpaceAI/nodespace-skill](https://github.com/NodeSpaceAI/nodespace-skill)**.
It carries a spec-compliant `skills/nodespace/` folder, regenerated and
pushed by this repo's release pipeline (`scripts/publish-skill-repo.ts`) on
every release — never hand-edited.

## What's in this package

This package's build output (`dist/`, `shims/`, `SKILL.md`, `references/`) is
consumed two ways, both inside this monorepo's own tooling:

1. **Bundled into the desktop app.** `scripts/build-skill.ts` compiles
   `src/install.ts` into a standalone executable (`bun build --compile`,
   staged as the `nodespace-skill-installer` `externalBin` sidecar — the
   same mechanism as `nodespaced`/`nodespace`, on macOS and Windows, the two
   platforms with a Tauri desktop app) and also stages `dist/`, `shims/`,
   `SKILL.md`, and `references/` into
   `packages/desktop-app/src-tauri/resources/skill/` as a Tauri resource. On
   first launch, the app runs the compiled binary — genuinely zero
   dependency on any external runtime, so a packaged app's end user never
   needs `bun` or `node` installed — with `install --resource-root
   <path to the staged resources above>`, to detect which agents are
   present and copy `SKILL.md` (with the right frontmatter prepended) into
   each one's skills directory. Falls back to running `dist/install.js`
   directly via `bun` or `node` (never `npx`/`npm`) only when the compiled
   binary isn't available for some reason — an unwired platform, or a
   dev/source checkout that hasn't run the compile step.
2. **Published to `NodeSpaceAI/nodespace-skill`.** The release pipeline runs
   `scripts/publish-skill-repo.ts`, which renders the same frontmatter this
   package builds (via `buildSkillFrontmatter` in `src/agents.ts`) plus
   `SKILL.md`'s body and `references/cli.md`, and pushes them to the public
   repo — the channel for a harness the app didn't launch.

## Manual usage (from a source checkout)

```bash
bun run --cwd packages/skill build
bun packages/skill/dist/install.js install
```

`--resource-root <path>` is only needed when running a *compiled* copy of
`install.ts` (`bun build --compile`) from somewhere other than this package
directory — it has no source-relative sibling directory to find
`SKILL.md`/`shims`/`references` from the way `dist/install.js` does. A plain
`dist/install.js` run like the one above finds them automatically.

## Supported Agents

| Agent | Detection | Install path |
|-------|-----------|--------------|
| Claude Code | `~/.claude/` exists | `~/.claude/skills/nodespace/SKILL.md` |
| Codex | `~/.codex/` exists | `~/.codex/skills/nodespace/SKILL.md` |
| Antigravity CLI | `~/.gemini/antigravity-cli/` exists | `~/.gemini/antigravity-cli/skills/nodespace/SKILL.md` |
| OpenCode | `~/.opencode/` exists | `~/.opencode/skills/nodespace/SKILL.md` |
| Pi | `~/.pi/agent/` exists | `~/.pi/agent/skills/nodespace/SKILL.md` |

## Prerequisites

The `nodespace` CLI must be on `$PATH`. Install it via the [NodeSpace desktop app](https://nodespace.ai) or the shell installer.

## Programmatic API

```ts
import { install, uninstall } from './installer.js';

// Install for all detected agents
const results = install();

// Install for specific agents
const results = install(['claude-code', 'codex']);

// Uninstall
const results = uninstall();
```
