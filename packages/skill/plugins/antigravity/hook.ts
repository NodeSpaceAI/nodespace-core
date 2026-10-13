// The NodeSpace hooks for Antigravity (ADR-093 §5).
//
// Antigravity runs each hook event as a command of its own, as Codex does, so
// this file is that command: it restores the conversation's state from a file
// named for it, hands the event to `./nodespace-session.ts` (which does the
// work, as it does for every harness), writes the state back, and answers on
// stdout. What such harnesses' hooks share is in `./hook-process.ts`.
//
// What differs from Codex is what Antigravity gives a hook:
//
// - The event's name is not in the payload: the registered command names it,
//   and it is the first argument.
// - There is no user-prompt event. `PreInvocation` runs before every model
//   call; its `invocationNum` is 0 for the first call after a user message and
//   counts up as the model's tool calls come back. The first is the prompt.
// - A `PostToolUse` hook must answer exactly `{}`, and carries no tool output.
//   The output is read from the conversation's transcript, and a note for the
//   agent waits for the next `PreInvocation`.
// - A `PreToolUse` hook must answer with a `decision`. `allow` leaves the
//   permission system to decide, as if no hook ran.
//
// It is run with `bun`, or with `node --experimental-strip-types`, so it uses
// only syntax that a type stripper removes, and names its imports with the
// extension.

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

import { emit, host, isRecord, load, pruneStale, run, save, SESSION_ID, stateDir, text, watchIntervalMs } from './hook-process.ts'
import { createSession, nodespaceInvocations } from './nodespace-session.ts'
import type { NodespaceSession } from './nodespace-session.ts'

const STATE_NAME = 'antigravity-sessions'
/** The tool that runs a shell line, with the line in `CommandLine`. */
const SHELL_TOOL = 'run_command'
const OUTPUT_MARKER = 'Output:\n'

function shellLineOf(payload: Record<string, unknown>): string | null {
  const call = payload.toolCall

  if (!isRecord(call) || call.name !== SHELL_TOOL || !isRecord(call.args)) {
    return null
  }

  return text(call.args.CommandLine) || null
}

/**
 * What the tool call at `stepIdx` printed, from the conversation's transcript:
 * the step that follows the model's call holds the output after `Output:`.
 * Empty when the transcript or the step cannot be read.
 */
async function outputOf(payload: Record<string, unknown>): Promise<string> {
  const path = text(payload.transcriptPath)
  const index = payload.stepIdx

  if (path === '' || typeof index !== 'number') {
    return ''
  }

  try {
    for (const line of (await readFile(path, 'utf8')).split('\n')) {
      const step: unknown = line === '' ? null : JSON.parse(line)

      if (isRecord(step) && step.step_index === index) {
        const content = text(step.content)
        const at = content.indexOf(OUTPUT_MARKER)

        return at >= 0 ? content.slice(at + OUTPUT_MARKER.length) : content
      }
    }
  } catch {
    // No transcript, or a line that is not JSON: the session reads no output.
  }

  return ''
}

/** A step that puts `message` in the conversation as a system message. */
function injected(message: string | null): Record<string, unknown> {
  return message ? { injectSteps: [{ systemMessage: { systemMessage: message } }] } : {}
}

async function handle(payload: Record<string, unknown>): Promise<void> {
  const event = process.argv[2] ?? ''
  const sessionId = text(payload.conversationId)

  if (!SESSION_ID.test(sessionId)) {
    return
  }

  const dir = stateDir(STATE_NAME)
  const path = join(dir, `${sessionId}.json`)
  const folders = Array.isArray(payload.workspacePaths) ? payload.workspacePaths : []
  const cwd = text(folders[0]) || process.cwd()
  const loaded = await load(path)
  const saved = loaded?.saved ?? null
  const before = loaded?.raw ?? null
  const session: NodespaceSession = createSession(host, watchIntervalMs(), saved?.session)
  let pendingNote = saved?.pendingNote ?? null

  /**
   * A session no earlier event started: a conversation begun before the
   * hooks were installed, or a state file lost. A prompt also starts one that
   * could not reach NodeSpace, so a daemon started since is found; a tool call
   * does not, which would cost every call a timeout while it stays down.
   */
  async function ensureStarted(isPrompt = false): Promise<void> {
    if (!saved || (isPrompt && !saved.session.isReached)) {
      await session.start(cwd, { sessionId, opens: false })
    }
  }

  if (event === 'SessionStart') {
    // A conversation the state already describes was begun, and told, before.
    const isContinued = saved !== null

    await pruneStale(dir, path)

    if (!isContinued) {
      await session.start(cwd, { sessionId, opens: true })
      pendingNote = null
    }

    await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    emit(isContinued ? {} : injected(session.section()))
  } else if (event === 'PreInvocation') {
    const isPrompt = payload.invocationNum === 0
    // The section is told once, with the start that made it.
    const isStartingNow = !saved || !saved.session.isReached
    const parts: string[] = []

    if (isPrompt) {
      await ensureStarted(true)

      const note = await session.prompt()
      const section = session.section()

      if (note) {
        // A changed skill list is followed by the list itself, so the note
        // and what it names agree.
        parts.push(section ? `${note}\n\n${section}` : note)
      } else if (isStartingNow && section) {
        parts.push(section)
      }
    }

    if (pendingNote) {
      parts.push(pendingNote)
      pendingNote = null
    }

    await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    emit(injected(parts.length > 0 ? parts.join('\n\n') : null))
  } else if (event === 'PreToolUse') {
    await ensureStarted()

    const verdict = await session.beforeTool()

    if (verdict && 'note' in verdict) {
      pendingNote = pendingNote ? `${pendingNote}\n\n${verdict.note}` : verdict.note
    }

    await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    emit(verdict && 'deny' in verdict ? { decision: 'deny', reason: verdict.deny } : { decision: 'allow' })
  } else if (event === 'PostToolUse') {
    await ensureStarted()

    const line = shellLineOf(payload)
    // The transcript is read only for a line that runs `nodespace`: nothing
    // else's output is used.
    const output = line !== null && nodespaceInvocations(line).length > 0 ? await outputOf(payload) : ''

    await session.afterTool(line, output)
    await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    emit({})
  }
}

await run(handle)
