---
name: run-app
description: Launch the NodeSpace desktop app (dev:tauri) and drive it through the debug bridge to see a change working in the real app. Use when asked to run, start, or screenshot the app, or to confirm a change works outside the test suite.
allowed-tools: Bash, Read, Write, Edit
---

## What this skill is for

Running the real desktop app so you can look at actual rendered nodes. There is
one backend path, the Tauri command layer; no browser mode and no HTTP dev-proxy
exist (ADR-103). Use it when tests pass but you need to *see* something: a
visual change, a layout, an interaction. The app runs in the WKWebView it ships
with and is driven through the debug bridge.

## CRITICAL: never launch the daemon against the user's real database

The daemon defaults to `~/.nodespace/database/nodespace.db` — the user's actual
notes. Two traps, both of which have bitten before:

1. **A bare `nodespaced` starts a daemon against the real database.**
   `nodespaced --help` does not print help. A binary built from a current
   checkout refuses any argument other than `--tray` and `--version` with a
   one-line error and exit status 2 before anything starts, but an older one
   (the installed `/usr/local/bin` or Homebrew daemon, or a stale
   `target/debug/nodespaced`) ignores unknown flags and
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

## Launch and inspect (debug bridge)

`bun run dev` runs `bun run dev:tauri`, which carries a development-only bridge,
so you can inspect the WKWebView the app ships with. Drive it with `tauri-mcp`.

**Launching the app cannot be isolated from the user's service registration.**
It installs the sidecars into `~/.nodespace/bin-dev`, kills a daemon on the default
socket, and registers the debug launchd label (`app.nodespace.daemon.dev`). That
label is per user, so neither `HOME` nor `NODESPACE_HOME` scopes it, and the app
re-registers it whenever it rebuilds or finds a changed sidecar. A scratch
`HOME` moves the files the app itself writes; it does **not** stop
the app replacing the user's loaded job with one pointing at the scratch install.
The daemon that job starts is run by launchd, which gives it the account's real
`HOME`, so the app writes `HOME` (and `NODESPACE_HOME`, when set) into the job's
environment whenever they differ from the account's; a daemon registered by an
older build lacks them and would create `databases.toml` and `nodespace.db` under
the real `~/.nodespace`. After a scratch launch, check that `~/.nodespace` gained no
`databases.toml`, `database/` or `.model-verify-key`.

The webview's storage is **not** relocated by `HOME`: WKWebView keeps
`localStorage` under the account's real home, so a scratch launch starts with the
previous install's tab state and active database id. The app discards a remembered
database id the daemon has not registered, and tabs saved for another database are
never restored into this one; to start from empty storage regardless, run
`localStorage.clear()` in the app's web inspector (or `webview-execute-js`).
Pre-seeding the sidecars only avoids re-registration while they stay unchanged
and validly signed, and `dev:tauri` rebuilds them first, so do not rely on it.
Get the user's go-ahead before launching it, and restore afterwards.

1. Note the user's job: `launchctl print gui/$(id -u)/app.nodespace.daemon.dev`
   (its `path`, `program` and `pid`).
2. Pick a short scratch home (`/tmp/nsh`) and pre-seed it: copy
   `target/debug/binaries/nodespaced-<triple>` and `nodespace-<triple>` to
   `/tmp/nsh/.nodespace/bin-dev/nodespaced` and `nodespace`.
3. Start that daemon with `HOME=/tmp/nsh`,
   `NODESPACED_SOCKET=/tmp/nsh/.nodespace/daemon-dev.sock`, `NODESPACE_HOME=/tmp/nsh/.nodespace`.
   The log must say `/tmp/nsh/...`, never `/Users/<you>/.nodespace/`.
4. Launch with `HOME=/tmp/nsh CARGO_HOME=… RUSTUP_HOME=… bun run dev:tauri`
   (literal paths for the two; a scratch HOME otherwise downloads a toolchain).
   Do not set `NODESPACED_SOCKET` for the app: the app registers a daemon on its
   home socket, so the two must agree.
5. Afterwards restore the registration: `launchctl bootout
   gui/$(id -u)/app.nodespace.daemon.dev`, then `launchctl bootstrap
   gui/$(id -u) <the plist path noted in step 1>`, and confirm `path` and
   `program` match step 1 (the `pid` will differ: the job restarts). Stop the
   scratch daemon and remove `/tmp/nsh`.

A session that cannot set `HOME` (a worktree-isolated agent) cannot do step 3
or 4; have the user run them in their own terminal.

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

Create nodes by typing into the real textarea through the bridge
(`webview-interact` to click `.node__content--view`, then `webview-keyboard`
to type). Markdown prefixes (`# `, `[] `, `> `, `1. `) change node type, but
typing them fast races the input handler and mangles the text; do not trust the
resulting text, and do not report it as correct content.

Routes are a dead end: the root layout renders the app shell for every path.

## Toggle themes

`.dark` / `.light` live on `documentElement`; set them with `webview-execute-js`:

```js
document.documentElement.classList.remove('dark');
document.documentElement.classList.add('light');
```

Always check both — light-mode-only verification misses the case where
something inherits `--foreground` and turns near-white on dark.

## Clean up

End the driver session, restore the launchd registration as above, and delete
stray screenshots (and `.playwright-mcp/`) from the repo before committing.
Confirm `~/.nodespace/database/nodespace.db` has an mtime that predates your
session.

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
| Launch | `bun run dev` (= `bun run dev:tauri`) |
| Bridge | `bunx tauri-mcp`, port 9223 |
| Launchd label (per user, not isolable) | `app.nodespace.daemon.dev` |
| Isolation lever for a standalone daemon | `NODESPACE_HOME` (**not** `NODESPACED_DB_PATH` alone) |
| Socket path | Must be short — never the scratchpad |
| Real DB (never touch) | `~/.nodespace/database/nodespace.db` |
