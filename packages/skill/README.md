# @nodespaceai/skill

Installs the NodeSpace Agent Skill into PTY agents (Claude Code, Codex, Antigravity CLI, OpenCode, Pi).

**This package is not published to npm.** `@nodespaceai/skill` never existed
on the npm registry, and it is not going to: the NodeSpace desktop app is the
one thing that installs this package's output, and it does so by running a
built installer directly (see
`packages/desktop-app/app-lib/src/skill_setup.rs`) — never `npx`/`npm`, so
publishing to npm was never actually required for the app's own install path.

If you're using an external agent harness yourself (not launched via the
NodeSpace app) and want the skill without installing NodeSpace first, import
the generated public repo instead:
**[NodeSpaceAI/nodespace-skill](https://github.com/NodeSpaceAI/nodespace-skill)**.
It carries a spec-compliant `skills/nodespace/` folder, regenerated and
pushed by this repo's release pipeline (`scripts/publish-skill-repo.ts`) on
every release — never hand-edited.

## What's in this package

This package's build output (`dist/`, `plugins/`, `SKILL.md`, `references/`) is
consumed two ways, both inside this monorepo's own tooling:

1. **Bundled into the desktop app.** `scripts/build-skill.ts` compiles
   `src/install.ts` into a standalone executable (`bun build --compile`,
   staged as the `nodespace-skill-installer` `externalBin` sidecar — the
   same mechanism as `nodespaced`/`nodespace`, on macOS and Windows, the two
   platforms with a Tauri desktop app) and also stages `dist/`, `plugins/`,
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
   `SKILL.md`'s body and every `references/*.md` file, and pushes them to the
   public repo — the channel for a harness the app didn't launch. The Claude
   Code plugin is published at that repo's root, so a marketplace install
   loads the same plugin the app installs.

## Manual usage (from a source checkout)

```bash
bun run --cwd packages/skill build
bun packages/skill/dist/install.js install
```

`--resource-root <path>` is only needed when running a *compiled* copy of
`install.ts` (`bun build --compile`) from somewhere other than this package
directory — it has no source-relative sibling directory to find
`SKILL.md`/`plugins`/`references` from the way `dist/install.js` does. A plain
`dist/install.js` run like the one above finds them automatically.

## Supported Agents

| Agent | Detection | Skill | Beyond the skill |
|-------|-----------|-------|------------------|
| Claude Code | `~/.claude/` exists (`CLAUDE_CONFIG_DIR` moves it) | `~/.claude/skills/nodespace/SKILL.md` | Plugin, in the skill folder |
| Codex | `~/.codex/` exists (`CODEX_HOME` moves it) | `~/.codex/skills/nodespace/SKILL.md` | Instructions block in `~/.codex/AGENTS.md` |
| Antigravity CLI | `~/.gemini/antigravity-cli/` exists | `~/.gemini/antigravity-cli/skills/nodespace/SKILL.md` | Instructions block in `~/.gemini/config/AGENTS.md` |
| OpenCode | `~/.config/opencode/` exists (`XDG_CONFIG_HOME` moves it) | `~/.config/opencode/skills/nodespace/SKILL.md` | Plugin, in `~/.config/opencode/plugins/` |
| Pi | `~/.pi/agent/` exists (`PI_CODING_AGENT_DIR` moves it) | `~/.pi/agent/skills/nodespace/SKILL.md` | Extension, in `~/.pi/agent/extensions/nodespace/` |

Each install copies `SKILL.md` and every `references/*.md` file in the package,
then the agent's harness plugin into the folder that harness loads code from,
or, for a harness with no plugin, one marked block into its user-level
instructions file. It writes `.nodespace-install.json` beside `SKILL.md`
listing exactly what it wrote: the skill's files, the plugin's files where they
sit in a folder of their own, and the instructions file. Uninstall removes what
that record lists, and a reinstall removes any listed file the new skill no
longer ships. An install from before the record existed is cleaned up from the
fixed file list the installer wrote back then.

A plugin folder may be one the user keeps plugins of their own in. A file
already there that no install recorded, and that does not hold what this skill
ships, is not replaced: the plugin is not installed, with a warning naming the
file. An uninstall with no record removes a plugin file only when it holds
exactly what this skill ships. An instructions file that is not UTF-8 text is
left as it is, with a warning.

OpenCode also reads the Claude skills folder. It keeps one skill per `name`, so
the skill installed in both is listed to its agent once.

`install.js status` reports, per agent, whether the skill is present and
whether the plugin or the instructions block is installed with it.

## The instructions block

Codex and Antigravity get no plugin. The installer writes one block into the
harness's own user-level instructions file, between two HTML comment markers,
creating the file when it is absent. The block holds the same orientation and
confirmation rules the plugins add, and tells the agent to list the graph's
skills (`nodespace skill guidance`) before starting work.

The installer owns the bytes from the begin marker to the end marker and
nothing else in the file. A reinstall replaces the block where it is. An
uninstall removes it and leaves every other byte as it was; a file the block
was the whole of is deleted. Codex reads `AGENTS.override.md` instead of
`AGENTS.md` when a user keeps one, and the block is not read while it exists.

Nothing makes an agent follow the block, so its wording is what is tested. To
check a change to it, give a model the block (`renderBlock()` in
`src/instructions-block.ts`) as standing instructions, a `nodespace` command
that only logs its arguments, and a request to record something; its first
`nodespace` command must be `skill guidance`, before any write.

## Shipped text

The orientation and the confirmation rules are shipped, never read from the
graph. `src/shipped-text.ts` is the one source. A plugin is installed as files
of its own and cannot import it, so each holds a copy: the Claude Code plugin
in `hooks/register.ts`, the Pi extension and the OpenCode plugin in the module
they share. `src/tests/shipped-text.test.ts` fails when a copy differs.

## The Claude Code plugin

`plugins/claude-code/` is a Claude Code plugin of function hooks: a manifest
(`.claude-plugin/plugin.json`), a hooks file (`hooks/hooks.json`), one hooks
module (`hooks/register.ts`) and the type contract for the session state the
module keeps (`types/index.d.ts`). The installer copies those four files into
`<claude config dir>/skills/nodespace/`, beside `SKILL.md`, and Claude Code
loads a plugin it finds in a skill folder with no flag.

In a session it checks that the `nodespace` CLI and the daemon answer, finds
the project whose `repository` is the checkout's remote, adds one section to
the system prompt (an orientation and the confirmation rules, both shipped
here, and the graph's skill list, marked as graph data), and from then on
tells the agent when the skill list changes or when the item it is working on
changes under it. Every piece of content comes from a `nodespace` command;
the module holds no retrieval logic. `NODESPACE_DATABASE` in the session's
environment selects the database for every command it runs. The one option,
`watch_interval_seconds` (default 60), is how often at most a tool call checks
the item being worked on.

The item being worked on is the node of the session's latest context read
(`nodespace node context <id>`, or a `query run --with-context` that returned
one item), whatever its type: reading another node's context moves the watch
to that node. The plugin reads the agent's shell lines to learn this, and to
tell the session's own writes from someone else's. A write made where it
cannot see one (inside a script, or by a command left running in the
background) is read as someone else's and stops the session until the user
replies.

### Testing the plugin

The plugin's tests run inside Claude Code's own engine, so they are not part of
`bun run test` or the merge gate. Run them by hand after changing anything
under `plugins/claude-code/`:

```bash
bun run --cwd packages/skill test:plugin
```

That runs `claude plugin validate` and then `claude plugin test` on the
folder, and needs the `claude` CLI on `$PATH`. The plugin API is early access
and changes between Claude Code releases: run it again after an upgrade.

## The Pi extension and the OpenCode plugin

Both do what the Claude Code plugin does, through their own harness's hooks.
`plugins/shared/nodespace-session.ts` holds the behaviour behind three things a
harness supplies (a way to run a command, the environment, a clock), and each
harness file is the hooks around it:

| | Pi (`plugins/pi/index.ts`) | OpenCode (`plugins/opencode/nodespace.ts`) |
|---|---|---|
| Session start | `session_start` | `session.created`, or the first hook of a session that was resumed |
| System prompt | a `nodespace` section, set on every `before_agent_start` | pushed in `experimental.chat.system.transform` on each request |
| Skill list changed | a message returned from `before_agent_start` | a text part added in `chat.message` |
| Refusing a tool call | `{ block: true, reason }` from `tool_call` | an error thrown from `tool.execute.before` |
| A note on a tool result | content appended in `tool_result` | text appended in `tool.execute.after` |
| Status | a status entry, when the session has a UI | one toast: OpenCode has no status line |

The skill list is read again on each prompt, so the section always holds the
list as of the latest prompt. `NODESPACE_DATABASE` selects the database for
every command, and `NODESPACE_WATCH_INTERVAL_SECONDS` (default 60) is how
often at most a tool call checks the item being worked on.

The installed layout differs from the repository's. Pi loads
`extensions/nodespace/index.ts` and the shared module sits beside it. OpenCode
loads every file directly in `plugins/` and calls each export of one as a
plugin, so only `nodespace.ts` sits there and the shared module is one folder
down, in `plugins/nodespace/`. In the repository each plugin imports the
shared module through a one-line re-export at that same relative path
(`plugins/pi/nodespace-session.ts`, `plugins/opencode/nodespace/nodespace-session.ts`);
the installer puts the module itself there.

### Testing them

`bun run --cwd packages/skill test` drives both through their hooks over a fake
`nodespace` command (`src/tests/plugin-session.test.ts`), and
`bun run --cwd packages/skill quality:check` type-checks each against its
harness's published types (`tsconfig.plugins.json`). Neither runs a harness.

To see one working in its harness, after changing anything under
`plugins/pi/`, `plugins/opencode/` or `plugins/shared/`, or after either
harness changes its plugin API:

1. Point the harness's home at an empty folder (`PI_CODING_AGENT_DIR`, or
   `XDG_CONFIG_HOME` for OpenCode), create the harness's directory in it, and
   run `bun packages/skill/src/install.ts install pi` (or `opencode`). Always
   name the agent: with none, the installer writes to every harness on the
   machine.
2. Start the harness in a checkout whose `origin` is a project's `repository`
   in a running NodeSpace, and send a prompt.
3. Check that the status (Pi) or the toast (OpenCode) names the project, that
   the agent can say which skills the graph holds without running a command,
   and that after `nodespace node context <id>` on a task, changing that task
   from another terminal makes the agent's next tool call fail with the reason.

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
