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
// - A `PreToolUse` hook must answer with a `decision`, and an empty answer
//   denies the call. `allow` leaves the permission system to decide, as if no
//   hook ran. So whatever goes wrong, this hook still answers.
//
// It is run with `bun`, or with `node --experimental-strip-types`, so it uses
// only syntax that a type stripper removes, and names its imports with the
// extension.

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

import { emit, host, isRecord, load, pruneStale, readStdin, save, SESSION_ID, stateDir, text, watchIntervalMs } from './hook-process.ts'
import { createSession, nodespaceInvocations } from './nodespace-session.ts'
import type { NodespaceSession } from './nodespace-session.ts'

const STATE_NAME = 'antigravity-sessions'
/** The tool that runs a shell line, with the line in `CommandLine`. */
const SHELL_TOOL = 'run_command'
const OUTPUT_MARKER = 'Output:\n'
const EVENT = process.argv[2] ?? ''

/** What a hook says when it has nothing else to: the one `PreToolUse` takes as no objection. */
const SILENCE: Record<string, unknown> = EVENT === 'PreToolUse' ? { decision: 'allow' } : {}

function shellLineOf(payload: Record<string, unknown>): string | null {
  const call = payload.toolCall

  if (!isRecord(call) || call.name !== SHELL_TOOL || !isRecord(call.args)) {
    return null
  }

  return text(call.args.CommandLine) || null
}

/**
 * What the tool call at `stepIdx` printed, from the conversation's transcript:
 * the step of that index holds the output after `Output:`. A line is tried as
 * JSON only when it is that step's, and one that does not parse is skipped.
 * Empty when the transcript or the step cannot be read.
 */
async function outputOf(payload: Record<string, unknown>): Promise<string> {
  const path = text(payload.transcriptPath)
  const index = payload.stepIdx

  if (path === '' || typeof index !== 'number') {
    return ''
  }

  const needle = `"step_index":${index}`

  try {
    for (const line of (await readFile(path, 'utf8')).split('\n')) {
      // Cheap first, so a long transcript is not parsed line by line.
      if (!line.includes(needle)) {
        continue
      }

      try {
        const step: unknown = JSON.parse(line)

        if (isRecord(step) && step.step_index === index) {
          const content = text(step.content)
          const at = content.indexOf(OUTPUT_MARKER)

          return at >= 0 ? content.slice(at + OUTPUT_MARKER.length) : content
        }
      } catch {
        // A line that was being written: the session reads no output.
      }
    }
  } catch {
    // No transcript: the session reads no output.
  }

  return ''
}

/** A step that puts `message` in the conversation as a system message. */
function injected(message: string | null): Record<string, unknown> {
  return message ? { injectSteps: [{ systemMessage: { systemMessage: message } }] } : {}
}

async function handle(payload: Record<string, unknown>, answer: (output: Record<string, unknown>) => void): Promise<void> {
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
   * The state is written before the answer, and a state that cannot be
   * written does not change it: a refusal is still a refusal.
   */
  async function persist(): Promise<void> {
    try {
      await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    } catch (err) {
      if (process.env.NODESPACE_HOOK_DEBUG) {
        process.stderr.write(`nodespace hook: ${String(err)}\n`)
      }
    }
  }

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

  if (EVENT === 'SessionStart') {
    // A conversation the state already describes was begun, and told, before.
    const isContinued = saved !== null

    await pruneStale(dir, path)

    if (!isContinued) {
      await session.start(cwd, { sessionId, opens: true })
      pendingNote = null
    }

    await persist()
    answer(isContinued ? {} : injected(session.section()))
  } else if (EVENT === 'PreInvocation') {
    // Strictly 0: a payload without the number is not taken for a user, which
    // would lift a refusal on every call.
    const parts: string[] = []

    if (payload.invocationNum === 0) {
      // What the agent was last told of the section, so the section follows a
      // note only when it is not what it was: a launched session's opening
      // note, handed over by the first prompt, is not a changed list.
      const told = saved?.session.isReached ? session.section() : null

      await ensureStarted(true)

      const note = await session.prompt()
      const section = session.section()

      if (note) {
        parts.push(section && section !== told ? `${note}\n\n${section}` : note)
      } else if (section && section !== told) {
        parts.push(section)
      }
    }

    // What a tool call found for the agent is told with the next call, the
    // user's message or the model's: the item's baseline has moved on.
    if (pendingNote) {
      parts.push(pendingNote)
      pendingNote = null
    }

    await persist()
    answer(injected(parts.length > 0 ? parts.join('\n\n') : null))
  } else if (EVENT === 'PreToolUse') {
    await ensureStarted()

    const verdict = await session.beforeTool()

    if (verdict && 'note' in verdict) {
      pendingNote = pendingNote ? `${pendingNote}\n\n${verdict.note}` : verdict.note
    }

    await persist()
    answer(verdict && 'deny' in verdict ? { decision: 'deny', reason: verdict.deny } : { decision: 'allow' })
  } else if (EVENT === 'PostToolUse') {
    await ensureStarted()

    const line = shellLineOf(payload)
    // The transcript is read only for a line that runs `nodespace`: nothing
    // else's output is used.
    const output = line !== null && nodespaceInvocations(line).length > 0 ? await outputOf(payload) : ''

    await session.afterTool(line, output)
    await persist()
    answer({})
  }
}

async function main(): Promise<void> {
  let isAnswered = false

  try {
    const parsed: unknown = JSON.parse(await readStdin())

    if (isRecord(parsed)) {
      await handle(parsed, output => {
        isAnswered = true
        emit(output)
      })
    }
  } catch (err) {
    // A hook must neither stop Antigravity nor fail its event: it says nothing
    // more than that it has no objection, unless asked to.
    if (process.env.NODESPACE_HOOK_DEBUG) {
      process.stderr.write(`nodespace hook: ${String(err)}\n`)
    }
  }

  if (!isAnswered) {
    emit(SILENCE)
  }
}

await main()
