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
| Codex | `~/.codex/` exists (`CODEX_HOME` moves it) | `~/.codex/skills/nodespace/SKILL.md` | Hooks, in `~/.codex/nodespace/` and `~/.codex/hooks.json`, and the instructions block in `~/.codex/AGENTS.md` |
| Antigravity CLI | `~/.gemini/antigravity-cli/` exists | `~/.gemini/config/skills/nodespace/SKILL.md` | Instructions block in `~/.gemini/config/AGENTS.md` |
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

Antigravity gets no plugin, and Codex gets the block beside its hooks, because
Codex runs a hook only once the user has trusted it (see below): a session
whose hooks are not trusted still has the block. The installer writes one block
into the harness's own user-level instructions file, between two HTML comment markers,
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
to that node. The plugin reads the agent's shell lines to learn this, and
nothing else about them.

The plugin tells the session's own writes from someone else's by a journal the
CLI keeps. At session start it sets `NODESPACE_WRITE_JOURNAL` to the session's
id, and every command the agent starts inherits it: a `nodespace` command that
writes a node, wherever it runs (a script, a loop, a command left running in
the background), appends the node id, the database and the node's new version
to `<NodeSpace home>/.nodespace/journals/<session>.jsonl`. The journal holds
ids and versions, never content, and no database records who changed a node.
When the item's version differs from the one last seen, the plugin reads the
journal: a version it holds is the session's own, and any other is a change
made under the session, which stops it until the user replies. The same goes
for an item that no longer exists: `nodespace node context <id> --json` prints
`{"error": "not_found", ...}` for it, and any other failed read says nothing.
The journal is removed when the session ends, and one a crashed session left is
removed, once it has been idle for an hour, by the next session's start.

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

## The Codex hooks

Codex runs each hook event as a command of its own, so nothing stays in memory
between events. `plugins/codex/hook.ts` is that command: it reads the event
from stdin, restores the session's state from
`<NodeSpace home>/.nodespace/codex-sessions/<session id>.json`, hands the event
to the same `plugins/shared/nodespace-session.ts` the Pi extension and the
OpenCode plugin use (`createSession(host, interval, state)` and
`session.snapshot()` carry the state across processes), writes the state back
and answers on stdout. The command runs with `bun` when it is on the path and
otherwise with `node --experimental-strip-types`. Nothing is registered for
the end of a session: `codex resume` continues a thread in a new process, so
its state file stays (one that no event has touched for a week is pruned), and
the CLI removes a journal idle for an hour.

| | Codex hook |
|---|---|
| Session start, project, orientation, consent rules, skill list | `SessionStart`: `additionalContext`; the reach text is a `systemMessage` |
| A changed skill list, and the launched task | `UserPromptSubmit`: `additionalContext` (with the new list) |
| Refusing a tool call | `PreToolUse`: `permissionDecision: "deny"` with the reason |
| A note on a tool result | `PostToolUse`: `additionalContext`, held in the state file from the `PreToolUse` that found it |

The installer puts `hook.ts` and the module beside it in `~/.codex/nodespace/`
and registers one handler per event in `~/.codex/hooks.json`, leaving every
other entry of that file as it is. Its handlers are found by the path of
`hook.ts` in their command, so an uninstall removes those and nothing else; a
file that is not valid JSON is left alone with a warning.

**Codex does not run a hook until the user has trusted it**, and installing
does not do that: the installer prints the step (start Codex and run `/hooks`),
and the instructions block is what a session without trusted hooks has.
Trust is tied to the hook's hash, so a reinstall that changes a handler asks
again.

Codex has no status line, so the reach text is only a `systemMessage`.

A Codex hook cannot set `NODESPACE_WRITE_JOURNAL` for the commands that follow
it, but Codex puts `CODEX_THREAD_ID` (the same id its hooks receive as
`session_id`) in the environment of every command. The CLI names a write's
journal after `NODESPACE_WRITE_JOURNAL` and, where that is unset or empty,
after `CODEX_THREAD_ID`, so the hook's journal and the CLI's agree. A Codex
started from a shell that already has `NODESPACE_WRITE_JOURNAL` set (inside
another harness's session) inherits it, and the CLI journals under that name:
the hook then finds no journal for its own thread and reads the session's own
writes as foreign. Unset the variable for such a Codex, as the scenario driver
does. The reverse nesting has the mirror problem: a harness started from a
Codex command with no variable of its own inherits `CODEX_THREAD_ID`, and its
writes are journaled under the Codex thread.

## The Antigravity hooks

Antigravity runs each hook event as a command of its own, as Codex does.
`plugins/antigravity/hook.ts` is that command, over the same
`plugins/shared/nodespace-session.ts` and the process plumbing Codex's hook
shares (`plugins/shared/hook-process.ts`: the host, the state file, stdin and
stdout). Its state is
`<NodeSpace home>/.nodespace/antigravity-sessions/<conversation id>.json`.

Antigravity differs from Codex in what a hook is given:

- The payload does not name its event. The registered command does: each
  handler's command ends with the event's name, which the hook reads as its
  first argument.
- There is no user-prompt event. `PreInvocation` runs before every model call,
  and its `invocationNum` is 0 for the first call after a user message and
  counts up while the model's tool calls come back. The call numbered 0 is the
  prompt: the skill list is compared there, and a refusal in force is lifted
  there. A call that follows a tool result is not the user.
- A `PostToolUse` hook must answer exactly `{}`: any other field makes
  Antigravity abort the tool call. It carries the tool call and no output, so
  the output of a `nodespace` command is read from the conversation's
  transcript (`transcriptPath`, the step at `stepIdx`), and a note for the
  agent waits for the next `PreInvocation`.
- A `PreToolUse` hook must answer with a `decision`, and `{}` denies the call.
  The hook answers `allow`, which leaves the permission system to decide as if
  no hook ran (a command that needs a permission is still asked for, or denied
  in print mode), and `deny` with the reason to refuse.

| | Antigravity hook |
|---|---|
| Session start, project, orientation, consent rules, skill list | `SessionStart`: `injectSteps` with a `systemMessage` |
| A changed skill list, and the launched task | `PreInvocation` numbered 0: `injectSteps` with a `systemMessage` (with the new list) |
| Refusing a tool call | `PreToolUse`: `{"decision": "deny", "reason": ...}` |
| A note on a tool result | the next `PreInvocation`: `injectSteps`, held in the state file from the `PreToolUse` that found it |

The installer puts `hook.ts` and the two modules beside it in
`~/.gemini/config/nodespace/` and registers one hook, named `nodespace`, in
`~/.gemini/config/hooks.json`: the hook's key is the one thing it owns in that
file, and every other hook is left as it is. A file that is not valid JSON is
left alone with a warning. Antigravity runs a registered hook with no trust
step, so the installer says nothing more than where it wrote.

Antigravity has no status line and no place for the reach text, so the status
duty is not available: a daemon that is not running means the section is not
told at start, and the first prompt after it is running tells it.

A hook always answers: a `PreToolUse` hook that fails answers `allow`, because
Antigravity reads an empty answer as a denial, and a hook that fails to read
or write its state does not stop the call. The registered command is the same
POSIX shell form the Codex hooks use; Antigravity's hook shell on Windows has
not been checked. Events that overlap each rewrite the state file and the last
writer wins, so a note held for the agent can be lost when two overlap.

A hook cannot set `NODESPACE_WRITE_JOURNAL` for the commands that follow it,
but Antigravity puts `ANTIGRAVITY_CONVERSATION_ID` (the id its hooks receive as
`conversationId`) in the environment of every command. The CLI names a write's
journal after `NODESPACE_WRITE_JOURNAL`, then `CODEX_THREAD_ID`, then
`ANTIGRAVITY_CONVERSATION_ID`, so the hook's journal and the CLI's agree. The
nesting caveats of the Codex hooks apply the same way.

## The Pi extension and the OpenCode plugin

Both do what the Claude Code plugin does, through their own harness's hooks.
`plugins/shared/nodespace-session.ts` holds the behaviour behind four things a
harness supplies (a way to run a command, the environment, a clock, a way to
read a file), and each harness file is the hooks around it:

| | Pi (`plugins/pi/index.ts`) | OpenCode (`plugins/opencode/nodespace.ts`) |
|---|---|---|
| Session start | `session_start` | `session.created`, or the first hook of a session that was resumed |
| The harness's session id | `ctx.sessionManager.getSessionId()` | the `sessionID` the hooks carry (`info.id` of `session.created`) |
| Session end | `session_shutdown` | `session.deleted` |
| System prompt | a `nodespace` section, set on every `before_agent_start` | pushed in `experimental.chat.system.transform` on each request |
| Skill list changed, and the launched task | a message returned from `before_agent_start` | a text part added in `chat.message` |
| Refusing a tool call | `{ block: true, reason }` from `tool_call` | an error thrown from `tool.execute.before` |
| A note on a tool result | content appended in `tool_result` | text appended in `tool.execute.after` |
| A command's environment | the bash tool's `spawnHook`: Pi's own bash tool is registered again with it | the `shell.env` hook |
| Status | a status entry, when the session has a UI | one toast: OpenCode has no status line |

The skill list is read again on each prompt, so the section always holds the
list as of the latest prompt. `NODESPACE_DATABASE` selects the database for
every command, and `NODESPACE_WATCH_INTERVAL_SECONDS` (default 60) is how
often at most a tool call checks the item being worked on.

In a session NodeSpace launched (`NODESPACE_SESSION` is set), a session start
reports the harness's own session id with `nodespace session report-harness-session`,
so the conversation can be resumed from NodeSpace, and reports again for each
conversation the harness starts in the same process (Pi's `session_start` with
reason `new`, `resume` or `fork`). When the launch names a task
(`NODESPACE_LAUNCHED_FOR`), its context is handed to the agent with the first
prompt, once, inside the graph-data marker, and the task is the item watched
from then on. A launch for the session's own chat node opens with nothing and
watches nothing. A task that cannot be read adds nothing and blocks nothing.
A session started from a terminal reports nothing. OpenCode serves child
sessions from the one plugin instance: only the first session that is not a
child's takes the launch.

Writes are attributed through the CLI's journal, as the Claude Code plugin
does: each plugin names the session to the commands the agent runs
(`NODESPACE_WRITE_JOURNAL`, set to the harness's session id) and reads
`<NodeSpace home>/.nodespace/journals/<session>.jsonl` when the watched item's
version moved. The shared module reads no shell line to learn whether a command
wrote. The same hook removes `NODESPACE_SESSION` and `NODESPACE_LAUNCHED_FOR`
from those commands, so a harness started from the agent's shell does not take
itself for the launched session.

- **Pi** sets both through its bash tool's `spawnHook`. The `powershell` tool
  is not replaced: a write made from it is read as someone else's, and the
  launch variables reach it.
  Pi's own bash tool is registered again to carry the hook, built with the
  session's directory and none of Pi's `shellPath` or `shellCommandPrefix`
  settings: a Pi user who sets either loses it for bash while the extension is
  loaded. A Pi reload replaces the extension, so it starts again with no item
  watched; a conversation that continues is not opened a second time.
- **OpenCode**'s `shell.env` output is merged over the process's environment,
  never substituted for it, so a launch variable cannot be removed there and
  is set to empty instead. The plugin and the CLI both read an empty
  `NODESPACE_SESSION` as no launch: `session report-harness-session` refuses
  it as a missing `--session` and sends nothing. OpenCode
  sends no event when it exits, so a journal is removed on `session.deleted`
  or by the next session's start once it has been idle for an hour.

What a run of each showed, against a daemon-registered session with
`NODESPACE_SESSION` and `NODESPACE_LAUNCHED_FOR` set and a scripted model
(Pi 1.0.4, OpenCode 1.18.35):

- **Pi.** The task's context arrived in the first request, with no command.
  The id reported with `report-harness-session` was Pi's own session id, and
  the daemon accepted it. A command the agent ran had `NODESPACE_WRITE_JOURNAL`
  set to that id and neither launch variable, so registering the tool under
  the built-in name does replace Pi's bash tool and the `spawnHook`'s
  environment reaches the command. A write from `bash -c 'nodespace node
  update …'` did not stop the session. Pi 1.0.4 needs a Node with
  `zlib.createZstdDecompress` (22.15 or later): on Node 22.14 it exits at
  start on its first network call, unless it is offline (`PI_OFFLINE=1`).
- **OpenCode.** The same three checks held, with OpenCode's `ses_…` session id.
  The agent's commands had both launch variables set to empty and
  `NODESPACE_WRITE_JOURNAL` set, beside the process's own variables, which is
  how the merge shows.

The run did not read the `ai-chat-pty` node: a session started with
`nodespace session launch` has none. The daemon keeps the reported id on the
session and, by its design, writes it to the node when the session ends; that
write needs a launch from the desktop app to see.

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
4. Start it again with `NODESPACE_SESSION` (a launched session's id) and
   `NODESPACE_LAUNCHED_FOR` (a task's id) set, and check that the agent knows
   the task without a command, that the session's `ai-chat-pty` node holds the
   harness's session id, and that a write from a script
   (`bash -c 'nodespace node update <id> ...'`) does not stop the session.

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
