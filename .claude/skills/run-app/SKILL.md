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

1. **`nodespaced --help` does not print help.** It ignores unknown flags and
   **starts a daemon against the real database**. Do not probe the binary with
   `--help` / `--version`. Read `packages/daemon/src/lib.rs` instead.
2. **`NODESPACED_DB_PATH` alone is not isolation.** The source says so directly
   (`resolve_db_path` doc comment): setting it while inheriting the real home
   "would seed the real `~/.nodespace/databases.toml` with a throwaway path
   (ADR-053)". Observed behavior: the daemon still served the real DB.

**`NODESPACE_HOME` is the correct lever.** It relocates the whole state
directory — database, registry, models — in one variable.

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
NODESPACED_HEADLESS=1 \
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

Wait for readiness with an `until` loop, not a fixed sleep — first boot loads an
embedding model and seeds ~37 agent roots:

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
| Headless | `NODESPACED_HEADLESS=1` |
| Ports | Vite 5173, dev-proxy 3001 |
| Real DB (never touch) | `~/.nodespace/database/nodespace.db` |
