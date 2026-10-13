// What the hooks of a harness that runs one command per event have in common
// (ADR-093 §5): the host the shared session module runs against, the file a
// conversation's state is kept in between events, and reading stdin and
// answering on stdout. Each harness's `hook.ts` supplies its event names and
// the shapes it answers in.
//
// It is run with `bun`, or with `node --experimental-strip-types`, so it uses
// only syntax that a type stripper removes, and names its import with the
// extension.

import { execFile } from 'node:child_process'
import { mkdir, readFile, readdir, rename, rm, stat, utimes, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

import { DEFAULT_WATCH_INTERVAL_SECONDS } from './nodespace-session.ts'
import type { Host, SessionState } from './nodespace-session.ts'

/** What a harness's hooks keep for a conversation: the shared module's state, and a note not yet delivered. */
export type Saved = { session: SessionState; pendingNote: string | null }

/** A conversation id this module names a file after. */
export const SESSION_ID = /^[A-Za-z0-9_-]{1,128}$/
/** A state file no event touched for this long belongs to a conversation that is gone. */
const STALE_AFTER_MS = 7 * 24 * 60 * 60 * 1000

export function text(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** The folder one harness's conversation states are kept in, under NodeSpace's home. */
export function stateDir(name: string): string {
  const home = process.env.NODESPACE_HOME || process.env.HOME || process.env.USERPROFILE || homedir()

  return join(home, '.nodespace', name)
}

export function watchIntervalMs(): number {
  const seconds = Number(process.env.NODESPACE_WATCH_INTERVAL_SECONDS ?? '')

  return (Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_WATCH_INTERVAL_SECONDS) * 1000
}

export const host: Host = {
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
    (value.project === null || (isRecord(value.project) && typeof value.project.id === 'string')) &&
    isNullable(value.section, 'string') &&
    Array.isArray(value.skills) &&
    value.skills.every(skill => isRecord(skill) && typeof skill.id === 'string') &&
    typeof value.listVersion === 'string' &&
    Array.isArray(value.fetched) &&
    (value.item === null ||
      (isRecord(value.item) &&
        typeof value.item.id === 'string' &&
        Array.isArray(value.item.paths) &&
        Array.isArray(value.item.parts) &&
        isRecord(value.item.fields))) &&
    typeof value.lastCheckedAt === 'number' &&
    isNullable(value.blocked, 'string') &&
    isNullable(value.opening, 'string') &&
    isNullable(value.journalName, 'string')
  )
}

/** The saved state and the text it was read from, which a write that changes nothing is skipped by. */
export async function load(path: string): Promise<{ saved: Saved; raw: string } | null> {
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
export async function save(dir: string, path: string, saved: Saved, before: string | null): Promise<void> {
  const body = JSON.stringify(saved)

  if (body === before) {
    // Nothing to write, but the event happened: the file's age is how long the
    // conversation has been idle, which is what decides when it is pruned.
    try {
      const now = new Date()

      await utimes(path, now, now)
    } catch {
      // The file is gone: the next event starts the session again.
    }

    return
  }

  await mkdir(dir, { recursive: true })

  const temporary = `${path}.${process.pid}.tmp`

  await writeFile(temporary, body)
  await rename(temporary, path)
}

/**
 * Removes the state of conversations no event has touched for a week, except
 * the one this event is for: it was read before this runs, and is touched after.
 */
export async function pruneStale(dir: string, keep: string): Promise<void> {
  try {
    for (const name of await readdir(dir)) {
      const path = join(dir, name)

      if (path !== keep && Date.now() - (await stat(path)).mtimeMs > STALE_AFTER_MS) {
        await rm(path, { force: true })
      }
    }
  } catch {
    // Nothing to prune.
  }
}

export async function readStdin(): Promise<string> {
  let body = ''

  for await (const chunk of process.stdin) {
    body += String(chunk)
  }

  return body
}

export function emit(output: Record<string, unknown>): void {
  process.stdout.write(`${JSON.stringify(output)}\n`)
}

/**
 * Reads the event from stdin as a JSON object and hands it to `handle`. A hook
 * must neither stop its harness nor fail an event: whatever goes wrong, it says
 * nothing, unless asked to.
 */
export async function run(handle: (payload: Record<string, unknown>) => Promise<void>): Promise<void> {
  try {
    const parsed: unknown = JSON.parse(await readStdin())

    if (isRecord(parsed)) {
      await handle(parsed)
    }
  } catch (err) {
    if (process.env.NODESPACE_HOOK_DEBUG) {
      process.stderr.write(`nodespace hook: ${String(err)}\n`)
    }
  }
}
