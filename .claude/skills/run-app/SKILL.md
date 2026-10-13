---
name: run-app
description: Launch NodeSpace (daemon + dev-proxy + frontend) and drive it in a browser to see a change working in the real app. Use when asked to run, start, or screenshot the app, or to confirm a change works outside the test suite.
allowed-tools: Bash, Read, Write, Edit
---

## What this skill is for

Running the real app — daemon, dev-proxy and frontend — so you can look at
actual rendered nodes. Use it when tests pass but you need to *see* something:
a visual change, a layout, an interaction.

## CRITICAL: never launch the daemon against the user's real database

The daemon defaults to `~/.nodespace/database/nodespace.db` — the user's actual
notes. Two traps, both of which have bitten before:

1. **A bare `nodespaced` starts a daemon against the real database.**
   `nodespaced --help` does not print help. A binary built from a current
   checkout refuses any argument other than `--tray` and `--version` with a
   one-line error and exit status 2 before anything starts, but an older one
   (the installed `/usr/local/bin` or Homebrew daemon, or a stale
   `target/debug/nodespaced` that step 1 reuses) ignores unknown flags and
   starts serving. Do not probe a binary you did not just build. The flags are
   constants in `packages/daemon/src/main.rs`; the environment variables it
   honours are resolved in `packages/daemon/src/lib.rs`.
2. **`NODESPACED_DB_PATH` alone is not isolation.** The source says so directly
   (`resolve_db_path` doc comment): setting it while inheriting the real home
   "would seed the real `~/.nodespace/databases.toml` with a throwaway path
   (ADR-053)". Observed behavior: the daemon still served the real DB.

**`NODESPACE_HOME` is the correct lever.** It relocates the whole state
directory — database, registry and embedding model — in one variable. Chat
models still load from `$HOME`. Provider settings and API keys are fields of
each database's own settings node (ADR-095), so an isolated daemon's database
starts with none; to test against a provider, add it in the isolated database
(the Settings screen, or the settings node's typed update), never by copying a
real database over it.

## Launch sequence

Run every step from the repo (or worktree) root.

### 1. Build the daemon if needed

```bash
ls target/debug/nodespaced || bun run build:sidecars --debug
```

A fresh worktree has no binary. `build:sidecars --debug` produces it (several
minutes cold).

### 2. Start the daemon on an isolated home + SHORT socket path

```bash
mkdir -p /tmp/nsd && rm -f /tmp/nsd/d.sock
NODESPACE_HOME=<scratchpad>/nshome \
NODESPACED_SOCKET=/tmp/nsd/d.sock \
  ./target/debug/nodespaced > <scratchpad>/daemon.log 2>&1
```

Run it with `run_in_background: true`.

**The socket path must be short.** Unix sockets cap at `SUN_LEN` (~104 chars),
and a session scratchpad path alone can exceed it. The failure is explicit:

```
Error: Failed to bind Unix socket: ...
Caused by: path must be shorter than SUN_LEN
```

So: **database** under the scratchpad (isolated), **socket** under `/tmp/nsd`
(short). They do not have to be co-located.

The isolated home holds no embedding model, so the daemon starts with semantic
search off and puts nothing on the GPU. To check a change that needs semantic
search, add `NODESPACED_MODEL_PATH=$HOME/.nodespace/models/nomic-embed-text-v1.5.Q8_0.gguf`,
which loads the model onto the GPU.

Wait for readiness with an `until` loop, not a fixed sleep — first boot seeds
~37 agent roots:

```bash
until [ -S /tmp/nsd/d.sock ] || grep -qiE "^Error|panic" <scratchpad>/daemon.log; do sleep 1; done
grep -iE "serving default database|gRPC server listening|^Error" <scratchpad>/daemon.log | tail -3
```

**Confirm the log line says the scratch path, not `/Users/<you>/.nodespace/`.**
That one line is the check that isolation worked.

### 3. Start the frontend pointed at the same socket

```bash
NODESPACED_SOCKET=/tmp/nsd/d.sock bun run dev:browser > <scratchpad>/dev.log 2>&1
```

Background it. `dev:browser` runs the dev-proxy (:3001) and Vite (:5173)
together. **The socket variable must match the daemon's** — the proxy defaults
to `~/.nodespace/daemon.sock` and will sit at "Connecting to local service…"
forever if it points somewhere else.

```bash
until curl -s -o /dev/null http://localhost:5173/; do sleep 1; done
curl -s -o /dev/null -w "proxy: HTTP %{http_code}\n" http://localhost:3001/health
```

Both up = the full chain (frontend → proxy → gRPC → daemon → SQLite) is live.

### 4. Drive it

Navigate to `http://localhost:5173/` with Playwright. A scratch DB opens on
today's Daily Journal with one empty node.

"Connecting to local service…" plus zero nodes means the daemon is not
reachable — fix that rather than working around it. Injecting your own markup
tests your markup, not the app.

Create nodes by typing into the real textarea:

```js
const ta = page.locator('textarea.node__content').first();
await ta.click();
await ta.pressSequentially('A text node', { delay: 5 });
await page.keyboard.press('Enter');
```

Markdown prefixes (`# `, `[] `, `> `, `1. `) change node type, but typing them
fast races the input handler and mangles the text. Add `waitForTimeout(400)`
between nodes, and do not trust the resulting text — for visual checks the
garbled text is usually irrelevant, but do not report it as correct content.

Routes are a dead end: the root layout renders the app shell for every path, so
a temporary `+page.svelte` never mounts.

### 5. Toggle themes

`.dark` / `.light` live on `documentElement`:

```js
document.documentElement.classList.remove('dark');
document.documentElement.classList.add('light');
```

Always check both — light-mode-only verification misses the case where
something inherits `--foreground` and turns near-white on dark.

### 6. Clean up

Stop both background tasks, then:

```bash
rm -rf /tmp/nsd
pgrep -fl "nodespaced|dev-proxy"     # expect nothing
ls -la ~/.nodespace/database/nodespace.db   # mtime must predate your session
```

Delete stray screenshots and `.playwright-mcp/` from the repo before
committing.

## Inspecting the real Tauri app (debug bridge)

`bun run dev:tauri` carries a development-only bridge, so you can inspect the
WKWebView the app ships with, not Chrome. Drive it with `tauri-mcp`.

**Launching the app cannot be isolated from the user's service registration.**
It installs the sidecars into `~/.nodespace/bin`, kills a daemon on the default
socket, and registers the debug launchd label (`app.nodespace.daemon.dev`). That
label is per user, so neither `HOME` nor `NODESPACE_HOME` scopes it, and the app
registers it again whenever it rebuilds or finds a changed sidecar. A scratch
`HOME` keeps the files and database out of the real home; it does **not** stop
the app replacing the user's loaded job with one pointing at the scratch install.
Get the user's go-ahead before launching it, and restore afterwards.

1. Note the user's job: `launchctl print gui/$(id -u)/app.nodespace.daemon.dev`
   (its `path`, `program` and `pid`).
2. Pick a short scratch home (`/tmp/nsh`) and pre-seed it: copy
   `target/debug/binaries/nodespaced-<triple>` and `nodespace-<triple>` to
   `/tmp/nsh/.nodespace/bin/nodespaced` and `nodespace`.
3. Start that daemon with `HOME=/tmp/nsh`,
   `NODESPACED_SOCKET=/tmp/nsh/.nodespace/daemon-dev.sock`, `NODESPACE_HOME=/tmp/nsh/.nodespace`.
   The log must say `/tmp/nsh/...`, never `/Users/<you>/.nodespace/`.
4. Launch with `HOME=/tmp/nsh CARGO_HOME=… RUSTUP_HOME=… bun run dev:tauri`
   (literal paths for the two; a scratch HOME otherwise downloads a toolchain).
   Do not set `NODESPACED_SOCKET` for the app: the app registers a daemon on its
   home socket, so the two must agree.
5. Afterwards restore the registration: `launchctl bootout
   gui/$(id -u)/app.nodespace.daemon.dev`, then `launchctl bootstrap
   gui/$(id -u) ~/Library/LaunchAgents/app.nodespace.daemon.dev.plist`, and
   confirm `path` and `program` match step 1. Stop the scratch daemon and remove
   `/tmp/nsh`.

A session that cannot set `HOME` (a worktree-isolated agent) cannot do step 2
or 3; have the user run them in their own terminal.

```bash
bunx tauri-mcp driver-session start      # 127.0.0.1:9223; --port N for another app
bunx tauri-mcp webview-dom-snapshot --type accessibility
bunx tauri-mcp webview-execute-js --script "(() => document.title)()"
bunx tauri-mcp webview-interact --action click --selector ".node__content--view"
bunx tauri-mcp webview-keyboard --action type --selector "textarea.node__content" --text "hi"
bunx tauri-mcp read-logs --source console --lines 20
bunx tauri-mcp ipc-execute-command --command list_databases
bunx tauri-mcp webview-screenshot        # writes a .jpg into the cwd: delete it
bunx tauri-mcp driver-session stop
```

- A fresh scratch database shows a "Who are you?" modal over the page; click
  `Skip` first (`--selector Skip --strategy text`).
- Clicking `.node__content--view` enters edit mode; the textarea exists only then.
- `ipc-monitor` sees only `window.__TAURI__.core.invoke` calls, not the app's own
  traffic, and a reload resets it.
- `hover` and `right-click` dispatch synthetic events (`focus` is untested; `isTrusted:
  false`): JS handlers fire, but CSS `:hover` does not apply, so hover styles
  cannot be verified this way.
- A second app takes the next free port from 9223; target it with `--port`.

## Reading colors accurately

Computed values beat eyeballing a screenshot. Measure on a real rendered
element:

```js
getComputedStyle(document.querySelector('.node-icon svg circle:last-of-type')).fill
```

A probe element resolves a variable without needing a rendered node:

```js
const p = document.createElement('div');
document.body.appendChild(p);
p.style.color = 'hsl(var(--node-text))';
const v = getComputedStyle(p).color;
p.remove();
```

## Quick reference

| Thing | Value |
|---|---|
| Isolation lever | `NODESPACE_HOME` (**not** `NODESPACED_DB_PATH` alone) |
| Socket var | `NODESPACED_SOCKET`, same value for daemon and proxy |
| Socket path | Must be short — `/tmp/nsd/d.sock`, never the scratchpad |
| Headless | Default — tray mode needs `--tray` |
| Ports | Vite 5173, dev-proxy 3001 |
| Real DB (never touch) | `~/.nodespace/database/nodespace.db` |
