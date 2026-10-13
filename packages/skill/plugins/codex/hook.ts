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

async function load(path: string): Promise<Saved | null> {
  try {
    const parsed: unknown = JSON.parse(await readFile(path, 'utf8'))

    if (isRecord(parsed) && isRecord(parsed.session)) {
      return {
        session: parsed.session as unknown as SessionState,
        pendingNote: typeof parsed.pendingNote === 'string' ? parsed.pendingNote : null,
      }
    }
  } catch {
    // No state, or state that cannot be read: the session starts again.
  }

  return null
}

/** Written whole and renamed into place, so a reader never sees half a file. */
async function save(path: string, saved: Saved): Promise<void> {
  await mkdir(stateDir(), { recursive: true })

  const temporary = `${path}.${process.pid}.tmp`

  await writeFile(temporary, JSON.stringify(saved))
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
  const saved = await load(path)
  const session: NodespaceSession = createSession(host, watchIntervalMs(), saved?.session)
  let pendingNote = saved?.pendingNote ?? null

  /** A session no earlier event started: hooks trusted after the session began, or a state file lost. */
  async function ensureStarted(): Promise<void> {
    if (!saved) {
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

    await save(path, { session: session.snapshot(), pendingNote })

    emit({
      ...context(event, session.section()),
      ...(reach ? { systemMessage: reach.text } : {}),
    })
  } else if (event === 'UserPromptSubmit') {
    await ensureStarted()

    const note = await session.prompt()
    const section = session.section()

    pendingNote = null
    await save(path, { session: session.snapshot(), pendingNote })

    // The section is not in a system prompt here: a changed skill list is
    // followed by the list itself, so the note and what it names agree.
    emit(context(event, note && section ? `${note}\n\n${section}` : note))
  } else if (event === 'PreToolUse') {
    await ensureStarted()

    const verdict = await session.beforeTool()

    if (verdict && 'note' in verdict) {
      pendingNote = pendingNote ? `${pendingNote}\n\n${verdict.note}` : verdict.note
    }

    await save(path, { session: session.snapshot(), pendingNote })

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
    await save(path, { session: session.snapshot(), pendingNote })
    emit(context(event, note))
  } else if (event === 'SessionEnd') {
    if (saved) {
      await session.end()
    }

    await rm(path, { force: true })
  }
}

async function main(): Promise<void> {
  try {
    const parsed: unknown = JSON.parse(await readStdin())

    if (isRecord(parsed)) {
      await handle(parsed)
    }
  } catch {
    // A hook must neither stop Codex nor fail its event: it says nothing.
  }
}

await main()
