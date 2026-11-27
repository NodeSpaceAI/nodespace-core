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

### What a scratch launch isolates, and what it still shares

| Thing | Moved by a scratch `HOME` / `NODESPACE_HOME`? |
|---|---|
| Daemon state: `databases.toml`, database files, embedding model, logs, marker | Yes, by `NODESPACE_HOME` (the parent of `.nodespace/`, so `NODESPACE_HOME=/tmp/nsh` gives `/tmp/nsh/.nodespace/...`) |
| The socket the app dials, the sidecar install, the app's own files | Only by `HOME`. The app never reads `NODESPACE_HOME` for its socket, so `NODESPACE_HOME` alone leaves the app dialing the account's own daemon |
| launchd label `app.nodespace.daemon.dev` | No. Per user |
| Chat models | Resolved under the state directory (`NODESPACE_HOME`), unless `NODESPACE_MODELS_DIR` names the directory. A Linux daemon with `NODESPACE_HOME` set created `models/` in the scratch home; a model load was not run |
| Webview storage (`localStorage`, IndexedDB, cookies, cache) | No. See below |

Use `HOME` and `NODESPACE_HOME` set to the same scratch directory (`/tmp/nsh`),
so the app's socket and the daemon's state agree: the socket is
`/tmp/nsh/.nodespace/daemon-dev.sock` for both, and the registry is
`/tmp/nsh/.nodespace/databases.toml`. A test pins this
(`default_registry_agreement_test`): with one home both sides use the same
socket and the same registry default; with `NODESPACE_HOME` apart from `HOME`
only the daemon moves, and the app does not reach it.

The webview's storage is **not** relocated by `HOME`. WKWebView's default data
store lives under the account's real home, in
`~/Library/WebKit/<bundle id>/WebsiteData/` and `~/Library/Caches/<bundle id>/`
(the bundle id is `com.nodespace.desktop`; a dev run is keyed by it, and
`localStorage` by its origin, `http://localhost:1422` under `dev:tauri`). The
storage process is started by the system, not by the app, so it does not see the
app's environment. A scratch launch therefore starts with the previous install's
tab state, active database id, theme and settings. The app discards a remembered
database id the daemon has not registered, and tabs saved for another database
are never restored into this one, so the carry-over does not break the launch; to
start from empty storage regardless, run `localStorage.clear()` in the app's web
inspector (or `webview-execute-js`). Clearing it also clears the real install's
dev-origin storage: the store is shared, not copied. The way to a separate store
is a window built with a data store identifier (macOS 14 and later; not
available through `HOME`); the app does not build its window that way yet.

**Linux, `systemctl --user`.** Run on Ubuntu 24.04 arm64 (systemd user session,
linger on) with the released v0.3.14 `nodespaced` and `nodespace`. The desktop app
has no Linux artifact, so the unit was written by hand from `systemd_unit_text`,
and the app's own launch and its webview (WebKitGTK) were **not** run. Observed:

| Thing | Result with a scratch `HOME` and `NODESPACE_HOME` |
|---|---|
| The user manager's environment | Its own, fixed at login: `HOME` is the account's real home and `systemctl --user show-environment` has no `NODESPACE_*`. A `HOME` exported to the app or to `systemctl` does not reach a unit |
| Where the app writes the unit | `$HOME/.config/systemd/user/nodespace.service`, so under the scratch home. The user manager does not read it: `daemon-reload` returns 0, then `enable` fails with "Unit file nodespace.service does not exist", `restart` with "Unit nodespace.service not found", and `systemctl --user cat nodespace.service` prints "No files found". The scratch launch gets **no daemon from the service manager**, and the real home is untouched |
| The unit in the account's `~/.config/systemd/user` | The manager runs it, but `enable` writes `default.target.wants/nodespace.service` into the real home, and a unit of that name there is the user's own. **A scratch launch cannot use it without replacing it** |
| Unit with `Environment=HOME=` and `Environment=NODESPACE_HOME=` (what the app writes) | The daemon's environment has both. `databases.toml`, `database/`, `models/`, the socket, the lock and the logs all sit under the scratch home. The real home gained nothing but the `enable` symlink. `nodespace database list` (it asks the daemon, so the path is the daemon's) printed `/tmp/nsh/.nodespace/database/nodespace.db` when the CLI had `HOME` and `NODESPACE_HOME` set, `NODESPACE_HOME` alone, or `HOME` alone. With neither set it dialed the real `~/.nodespace/daemon.sock`. A scratch daemon is reachable only by a client that resolves the scratch home: set `HOME` or `NODESPACE_HOME`, or `NODESPACED_SOCKET`, for the CLI too |
| Unit with only `NODESPACE_HOME=` | The daemon keeps the real `HOME` but state still lands under the scratch home, and the real home gained nothing but the `enable` symlink |
| Unit with neither (a registration made by an older build) | The daemon created `~/.nodespace/databases.toml`, `database/nodespace.db` (with `-wal` and `-shm`) and `models/` under the **real** home. Only the socket, lock and logs, which the unit names outright, were scratch |
| `~/.config`, `~/.local/share`, `~/.cache` | In the runs above, the only change under them was the `systemd/user` entries named above; the headless daemon wrote nothing to `~/.local/share` or `~/.cache` |
| `.model-verify-key` | Not created: nothing verified a model, and no model was loaded |
| The `--tray` unit, as the app writes it, on a host with no display | The daemon panics with "Failed to initialize gtk backend" and `Restart=on-failure` restarts it until the journal reads "Start request repeated too quickly" and the unit is `failed`. A headless host needs a display (a virtual one) or a unit without `--tray` |
| Linking against GTK | The daemon needs `libgtk-3`, `libxdo` and `libgomp` even to print `--version`; on a minimal image install `libgtk-3-0t64 libxdo3 libgomp1` |
| Secret store | `keyring` is a dependency on macOS and Windows only (`packages/core/Cargo.toml`); no run was made against a Linux secret service |

How to get a scratch daemon on Linux that writes nothing to the real home: do not
use the unit file. Start the daemon as a transient unit, which the manager keeps in
`/run/user/$UID/systemd/transient/` and which writes no unit file or symlink. This
is a **headless** daemon: no `--tray` and no `NODESPACE_UI_BINARY`, unlike the
app's unit, so it also avoids the GTK panic above. `append:` does not create the
log directory, so make it first, and set `NODESPACED` to the binary under test:

```bash
SCRATCH=/tmp/nsh                       # short: the socket path has a length limit
NODESPACED=/path/to/nodespaced         # the binary you built or downloaded
mkdir -p "$SCRATCH/.nodespace/logs"
systemd-run --user --unit=nsh-daemon --collect \
  -E HOME="$SCRATCH" -E NODESPACE_HOME="$SCRATCH" \
  -E NODESPACED_SOCKET="$SCRATCH/.nodespace/daemon.sock" \
  -p StandardOutput="append:$SCRATCH/.nodespace/logs/nodespaced.log" \
  -p StandardError="append:$SCRATCH/.nodespace/logs/nodespaced-error.log" \
  "$NODESPACED"
```

That recipe, pasted as written, left a before and after listing of the real home
identical. Reach the daemon with `HOME` or `NODESPACE_HOME` set for the CLI. Stop it
with `systemctl --user stop nsh-daemon`. To exercise the app's own unit writing, set
`HOME` and `NODESPACE_HOME`, check `systemctl --user cat nodespace.service` shows
the scratch unit, and expect the manager not to see it (above). To exercise the
unit itself, put it in the real `~/.config/systemd/user` only on a machine where no
`nodespace.service` exists, and remove it and its `default.target.wants` link
afterwards (`systemctl --user disable`, then delete the file and `daemon-reload`).
Take a `find "$HOME" -printf '%y %p %T+\n' | sort` listing before and after, and
diff them: `~/.nodespace` must not appear.

On Linux a scratch launch still shares the `enable` symlink in the real config
directory and the user's own `nodespace.service` name when the unit file is used;
the transient unit avoids both, and the app's own launch on Linux stays unverified.

Still unchecked on Linux: the app's launch, the webview's storage (WebKitGTK keeps
it under `$XDG_DATA_HOME` and `$XDG_CACHE_HOME`, which a scratch `HOME` moves only
when those are unset), and the secret store.

On macOS, get the user's go-ahead before launching it, and restore afterwards.

1. Note the user's job: `launchctl print gui/$(id -u)/app.nodespace.daemon.dev`
   (its `path`, `program` and `pid`).
2. Pick a short scratch home (`/tmp/nsh`) and pre-seed it: copy
   `target/debug/binaries/nodespaced-<triple>` and `nodespace-<triple>` to
   `/tmp/nsh/.nodespace/bin-dev/nodespaced` and `nodespace`.
3. Start that daemon with `HOME=/tmp/nsh`,
   `NODESPACED_SOCKET=/tmp/nsh/.nodespace/daemon-dev.sock`, `NODESPACE_HOME=/tmp/nsh`.
   The log must say `/tmp/nsh/...`, never `/Users/<you>/.nodespace/`.
4. Launch with `HOME=/tmp/nsh CARGO_HOME=… RUSTUP_HOME=… bun run dev:tauri`
   (literal paths for the two; a scratch HOME otherwise makes rustup download the
   toolchain `rust-toolchain.toml` pins afresh).
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
