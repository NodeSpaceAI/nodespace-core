// The NodeSpace hooks for Codex (ADR-093 §5).
//
// Codex runs each hook event as a command of its own: nothing stays in memory
// between events. This file is that command. It reads the event from stdin,
// restores the session's state from a file named for the Codex session, hands
// the event to `./nodespace-session.ts` (which does the work, as it does for
// Pi and OpenCode), writes the state back, and answers on stdout.
//
// It is run with `bun`, or with `node --experimental-strip-types`, so it uses
// only syntax that a type stripper removes, and names its import with the
// extension.

import { execFile } from 'node:child_process'
import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

import { createSession, DEFAULT_WATCH_INTERVAL_SECONDS } from './nodespace-session.ts'
import type { Host, NodespaceSession, SessionState } from './nodespace-session.ts'

/** What this file keeps for a session: the shared module's state, and a note not yet delivered. */
type Saved = { session: SessionState; pendingNote: string | null }

/** A hook's stdin: only the fields this file reads. */
type Payload = {
  hook_event_name?: unknown
  session_id?: unknown
  cwd?: unknown
  source?: unknown
  tool_name?: unknown
  tool_input?: unknown
  tool_response?: unknown
}

const SESSION_ID = /^[A-Za-z0-9_-]{1,128}$/
/** The tool that runs a shell line, with the line in `command`. */
const SHELL_TOOL = 'Bash'
/** A state file no event touched for this long belongs to a session that is gone. */
const STALE_AFTER_MS = 7 * 24 * 60 * 60 * 1000

function text(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stateDir(): string {
  const home = process.env.NODESPACE_HOME || process.env.HOME || process.env.USERPROFILE || homedir()

  return join(home, '.nodespace', 'codex-sessions')
}

function watchIntervalMs(): number {
  const seconds = Number(process.env.NODESPACE_WATCH_INTERVAL_SECONDS ?? '')

  return (Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_WATCH_INTERVAL_SECONDS) * 1000
}

const host: Host = {
  // Run by argv, with no shell, and killed at the timeout.
  run: (argv, options) =>
    new Promise(resolve => {
      const [command = '', ...args] = argv

      execFile(
        command,
        args,
        { cwd: options.cwd, timeout: options.timeoutMs, maxBuffer: 16 * 1024 * 1024 },
        (error, stdout, stderr) => {
          if (!error) {
            resolve({ code: 0, stdout, stderr })
          } else if (typeof error.code === 'number') {
            resolve({ code: error.code, stdout, stderr })
          } else if (error.killed) {
            resolve({ code: 124, stdout, stderr: 'timed out' })
          } else if (error.code === 'ENOENT') {
            resolve(null)
          } else {
            resolve({ code: 1, stdout, stderr: stderr || error.message })
          }
        },
      )
    }),
  env: name => process.env[name],
  now: () => Date.now(),
  readFile: async path => {
    try {
      return await readFile(path, 'utf8')
    } catch (err) {
      if ((err as { code?: unknown }).code === 'ENOENT') {
        return null
      }

      throw err
    }
  },
}

/** Whether a parsed file has the fields, of the types, a session reads. A stale shape starts the session again. */
function isState(value: unknown): value is SessionState {
  if (!isRecord(value)) {
    return false
  }

  const isNullable = (field: unknown, kind: string) => field === null || typeof field === kind

  return (
    isNullable(value.database, 'string') &&
    typeof value.isReached === 'boolean' &&
    (value.project === null || isRecord(value.project)) &&
    isNullable(value.section, 'string') &&
    Array.isArray(value.skills) &&
    typeof value.listVersion === 'string' &&
    Array.isArray(value.fetched) &&
    (value.item === null || isRecord(value.item)) &&
    typeof value.lastCheckedAt === 'number' &&
    isNullable(value.blocked, 'string') &&
    isNullable(value.opening, 'string') &&
    isNullable(value.journalName, 'string')
  )
}

/** The saved state and the text it was read from, which a write that changes nothing is skipped by. */
async function load(path: string): Promise<{ saved: Saved; raw: string } | null> {
  try {
    const raw = await readFile(path, 'utf8')
    const parsed: unknown = JSON.parse(raw)

    if (isRecord(parsed) && isState(parsed.session)) {
      return {
        saved: {
          session: parsed.session,
          pendingNote: typeof parsed.pendingNote === 'string' ? parsed.pendingNote : null,
        },
        raw,
      }
    }
  } catch {
    // No state, or state that cannot be read: the session starts again.
  }

  return null
}

/**
 * Written whole and renamed into place, so a reader never sees half a file.
 * Skipped when nothing changed: tool calls dispatched together each rewrite
 * the file, and the last writer wins, so a write that changes nothing must
 * not undo one that did.
 */
async function save(path: string, saved: Saved, before: string | null): Promise<void> {
  const text = JSON.stringify(saved)

  if (text === before) {
    return
  }

  await mkdir(stateDir(), { recursive: true })

  const temporary = `${path}.${process.pid}.tmp`

  await writeFile(temporary, text)
  await rename(temporary, path)
}

/** Removes the state of sessions no event has touched for a week. */
async function pruneStale(): Promise<void> {
  try {
    for (const name of await readdir(stateDir())) {
      const path = join(stateDir(), name)

      if (Date.now() - (await stat(path)).mtimeMs > STALE_AFTER_MS) {
        await rm(path, { force: true })
      }
    }
  } catch {
    // Nothing to prune.
  }
}

async function readStdin(): Promise<string> {
  let body = ''

  for await (const chunk of process.stdin) {
    body += String(chunk)
  }

  return body
}

function shellLineOf(payload: Payload): string | null {
  if (payload.tool_name !== SHELL_TOOL || !isRecord(payload.tool_input)) {
    return null
  }

  return text(payload.tool_input.command) || null
}

/** What a shell tool answered, as the text the session reads for the skills it shows. */
function outputOf(response: unknown): string {
  if (typeof response === 'string') {
    return response
  }

  if (!isRecord(response)) {
    return ''
  }

  const parts = [response.output, response.stdout, response.stderr].map(text).filter(part => part !== '')

  return parts.length > 0 ? parts.join('\n') : JSON.stringify(response)
}

function emit(output: Record<string, unknown>): void {
  process.stdout.write(`${JSON.stringify(output)}\n`)
}

function context(event: string, additionalContext: string | null): Record<string, unknown> {
  return additionalContext ? { hookSpecificOutput: { hookEventName: event, additionalContext } } : {}
}

async function handle(payload: Payload): Promise<void> {
  const event = text(payload.hook_event_name)
  const sessionId = text(payload.session_id)

  if (!SESSION_ID.test(sessionId)) {
    return
  }

  const path = join(stateDir(), `${sessionId}.json`)
  const cwd = text(payload.cwd) || process.cwd()
  const loaded = await load(path)
  const saved = loaded?.saved ?? null
  const before = loaded?.raw ?? null
  const session: NodespaceSession = createSession(host, watchIntervalMs(), saved?.session)
  let pendingNote = saved?.pendingNote ?? null

  /**
   * A session no earlier event started: hooks trusted after the session
   * began, or a state file lost. A prompt also starts one that could not
   * reach NodeSpace, so a daemon started since is found; a tool call does
   * not, which would cost every call a timeout while it stays down.
   */
  async function ensureStarted(isPrompt = false): Promise<void> {
    if (!saved || (isPrompt && !saved.session.isReached)) {
      await session.start(cwd, { sessionId, opens: false })
    }
  }

  if (event === 'SessionStart') {
    const source = text(payload.source)
    // A resumed or compacted conversation is the one the state describes.
    const isContinued = saved !== null && (source === 'resume' || source === 'compact')

    await pruneStale()

    const reach = isContinued
      ? null
      : await session.start(cwd, { sessionId, opens: source === 'startup' || source === 'clear' })

    pendingNote = isContinued ? pendingNote : null

    await save(path, { session: session.snapshot(), pendingNote }, before)

    emit({
      ...context(event, session.section()),
      ...(reach ? { systemMessage: reach.text } : {}),
    })
  } else if (event === 'UserPromptSubmit') {
    await ensureStarted(true)

    const note = await session.prompt()
    const section = session.section()

    pendingNote = null
    await save(path, { session: session.snapshot(), pendingNote }, before)

    // The section is not in a system prompt here: a changed skill list is
    // followed by the list itself, so the note and what it names agree.
    emit(context(event, note && section ? `${note}\n\n${section}` : note))
  } else if (event === 'PreToolUse') {
    await ensureStarted()

    const verdict = await session.beforeTool()

    if (verdict && 'note' in verdict) {
      pendingNote = pendingNote ? `${pendingNote}\n\n${verdict.note}` : verdict.note
    }

    await save(path, { session: session.snapshot(), pendingNote }, before)

    if (verdict && 'deny' in verdict) {
      emit({
        hookSpecificOutput: {
          hookEventName: event,
          permissionDecision: 'deny',
          permissionDecisionReason: verdict.deny,
        },
      })
    }
  } else if (event === 'PostToolUse') {
    await ensureStarted()
    await session.afterTool(shellLineOf(payload), outputOf(payload.tool_response))

    const note = pendingNote

    pendingNote = null
    await save(path, { session: session.snapshot(), pendingNote }, before)
    emit(context(event, note))
  }
}

async function main(): Promise<void> {
  try {
    const parsed: unknown = JSON.parse(await readStdin())

    if (isRecord(parsed)) {
      await handle(parsed)
    }
  } catch (err) {
    // A hook must neither stop Codex nor fail its event: it says nothing,
    // unless asked to.
    if (process.env.NODESPACE_HOOK_DEBUG) {
      process.stderr.write(`nodespace hook: ${String(err)}\n`)
    }
  }
}

await main()
