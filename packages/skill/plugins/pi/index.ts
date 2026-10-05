// The NodeSpace extension for Pi (ADR-093 §5).
//
// Pi's hooks around `./nodespace-session`, which does the work and is
// installed beside this file. Pi loads `extensions/nodespace/index.ts` from
// its agent directory with no flag.

import type { ExtensionAPI } from '@earendil-works/pi-coding-agent'

import { createSession, DEFAULT_WATCH_INTERVAL_SECONDS } from './nodespace-session'
import type { Host } from './nodespace-session'

const STATUS_KEY = 'nodespace'
/** The tools that run a shell line, each with the line in `command`. */
const SHELL_TOOLS = ['bash', 'powershell']

function shellLine(tool: string, input: unknown): string | null {
  const command = SHELL_TOOLS.includes(tool) ? (input as { command?: unknown } | null)?.command : undefined

  return typeof command === 'string' ? command : null
}

function watchIntervalMs(): number {
  const seconds = Number(process.env.NODESPACE_WATCH_INTERVAL_SECONDS ?? '')

  return (Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_WATCH_INTERVAL_SECONDS) * 1000
}

export default function nodespace(pi: ExtensionAPI): void {
  // Nothing is run here: Pi also loads an extension where no session starts.
  const host: Host = {
    run: async (argv, options) => {
      const [command = '', ...args] = argv
      const ran = await pi.exec(command, args, {
        timeout: options.timeoutMs,
        ...(options.cwd ? { cwd: options.cwd } : {}),
      })

      // A command Pi had to kill reports no exit code of its own.
      return ran.killed ? { ...ran, code: ran.code || 1 } : ran
    },
    env: name => process.env[name],
    now: () => Date.now(),
  }
  const session = createSession(host, watchIntervalMs())
  /** A note for a tool call's result, by the call's id. */
  const notes = new Map<string, string>()
  const refused = new Set<string>()

  pi.on('session_start', async (_event, ctx) => {
    const reach = await session.start(ctx.cwd)

    if (ctx.hasUI) {
      ctx.ui.setStatus(STATUS_KEY, reach.text)
    }
  })

  // Pi builds the system prompt afresh for each agent run, so the section is
  // set on every one, from the read `prompt` has just made.
  pi.on('before_agent_start', async event => {
    const note = await session.prompt()
    const section = session.section()

    if (section) {
      event.systemPromptOptions.sections.nodespace = section
    }

    return note ? { message: { customType: 'nodespace', content: note, display: true } } : undefined
  })

  pi.on('tool_call', async event => {
    const verdict = await session.beforeTool(shellLine(event.toolName, event.input))

    if (verdict && 'deny' in verdict) {
      refused.add(event.toolCallId)

      return { block: true, reason: verdict.deny }
    }

    if (verdict) {
      notes.set(event.toolCallId, verdict.note)
    }

    return undefined
  })

  pi.on('tool_result', async event => {
    const note = notes.get(event.toolCallId)

    notes.delete(event.toolCallId)

    // A call this extension refused never ran: it says nothing about the work.
    if (refused.delete(event.toolCallId)) {
      return undefined
    }

    const output = event.content.map(part => (part.type === 'text' ? part.text : '')).join('')

    await session.afterTool(shellLine(event.toolName, event.input), output)

    return note
      ? {
          content: [...event.content, { type: 'text' as const, text: note }],
          ...(event.structuredContent === undefined ? {} : { structuredContent: event.structuredContent }),
        }
      : undefined
  })
}
