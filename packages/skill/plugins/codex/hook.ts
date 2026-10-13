// The NodeSpace hooks for Codex (ADR-093 §5).
//
// Codex runs each hook event as a command of its own: nothing stays in memory
// between events. This file is that command. It reads the event from stdin,
// restores the session's state from a file named for the Codex session, hands
// the event to `./nodespace-session.ts` (which does the work, as it does for
// Pi and OpenCode), writes the state back, and answers on stdout. What every
// such harness's hooks share is in `./hook-process.ts`.
//
// It is run with `bun`, or with `node --experimental-strip-types`, so it uses
// only syntax that a type stripper removes, and names its imports with the
// extension.

import { join } from 'node:path'

import { emit, host, isRecord, load, pruneStale, run, save, SESSION_ID, stateDir, text, watchIntervalMs } from './hook-process.ts'
import { createSession } from './nodespace-session.ts'
import type { NodespaceSession } from './nodespace-session.ts'

/** The tool that runs a shell line, with the line in `command`. */
const SHELL_TOOL = 'Bash'
const STATE_NAME = 'codex-sessions'

function shellLineOf(payload: Record<string, unknown>): string | null {
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

function context(event: string, additionalContext: string | null): Record<string, unknown> {
  return additionalContext ? { hookSpecificOutput: { hookEventName: event, additionalContext } } : {}
}

async function handle(payload: Record<string, unknown>): Promise<void> {
  const event = text(payload.hook_event_name)
  const sessionId = text(payload.session_id)

  if (!SESSION_ID.test(sessionId)) {
    return
  }

  const dir = stateDir(STATE_NAME)
  const path = join(dir, `${sessionId}.json`)
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

    await pruneStale(dir, path)

    const reach = isContinued
      ? null
      : await session.start(cwd, { sessionId, opens: source === 'startup' || source === 'clear' })

    pendingNote = isContinued ? pendingNote : null

    await save(dir, path, { session: session.snapshot(), pendingNote }, before)

    emit({
      ...context(event, session.section()),
      ...(reach ? { systemMessage: reach.text } : {}),
    })
  } else if (event === 'UserPromptSubmit') {
    await ensureStarted(true)

    const note = await session.prompt()
    const section = session.section()

    pendingNote = null
    await save(dir, path, { session: session.snapshot(), pendingNote }, before)

    // The section is not in a system prompt here: a changed skill list is
    // followed by the list itself, so the note and what it names agree.
    emit(context(event, note && section ? `${note}\n\n${section}` : note))
  } else if (event === 'PreToolUse') {
    await ensureStarted()

    const verdict = await session.beforeTool()

    if (verdict && 'note' in verdict) {
      pendingNote = pendingNote ? `${pendingNote}\n\n${verdict.note}` : verdict.note
    }

    await save(dir, path, { session: session.snapshot(), pendingNote }, before)

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
    await save(dir, path, { session: session.snapshot(), pendingNote }, before)
    emit(context(event, note))
  }
}

await run(handle)
