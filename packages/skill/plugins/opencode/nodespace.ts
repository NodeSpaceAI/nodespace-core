// The NodeSpace plugin for OpenCode (ADR-093 §5).
//
// OpenCode's hooks around `./nodespace/nodespace-session`, which does the
// work and is installed one folder below this file. OpenCode loads every file
// directly in its `plugins/` folder with no configuration, and calls each
// export of one as a plugin: this file exports the plugin and nothing else.

import { execFile } from 'node:child_process'
import { readFile } from 'node:fs/promises'

import type { Plugin } from '@opencode-ai/plugin'

import { createSession, DEFAULT_WATCH_INTERVAL_SECONDS, LAUNCH_VARIABLES } from './nodespace/nodespace-session'
import type { Host, NodespaceSession } from './nodespace/nodespace-session'

/** The tool that runs a shell line, with the line in `command`. */
const SHELL_TOOL = 'bash'

function shellLine(tool: string, args: unknown): string | null {
  const command = tool === SHELL_TOOL ? (args as { command?: unknown } | null)?.command : undefined

  return typeof command === 'string' ? command : null
}

function watchIntervalMs(): number {
  const seconds = Number(process.env.NODESPACE_WATCH_INTERVAL_SECONDS ?? '')

  return (Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_WATCH_INTERVAL_SECONDS) * 1000
}

export const NodeSpace: Plugin = async ({ client, directory }) => {
  const host: Host = {
    // Run by argv, with no shell, and killed at the timeout. The shell
    // OpenCode hands a plugin has no timeout: a command the daemon never
    // answers would be left running, one more on every prompt.
    run: (argv, options) =>
      new Promise(resolve => {
        const [command = '', ...args] = argv

        execFile(
          command,
          args,
          { cwd: options.cwd ?? directory, timeout: options.timeoutMs, maxBuffer: 16 * 1024 * 1024 },
          (error, stdout, stderr) => {
            if (!error) {
              resolve({ code: 0, stdout, stderr })
            } else if (typeof error.code === 'number') {
              resolve({ code: error.code, stdout, stderr })
            } else if (error.killed) {
              resolve({ code: 124, stdout, stderr: 'timed out' })
            } else if (error.code === 'ENOENT') {
              // It could not be started: the command is not on the path.
              resolve(null)
            } else {
              // Ended by a signal, or it printed more than is kept.
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
  const intervalMs = watchIntervalMs()
  /** Whether the launch has been given to a session: only the one it started is the launched one. */
  let hasClaimedLaunch = false
  /** One per OpenCode session, started on first use: a resumed session sends no `session.created`. */
  const sessions = new Map<string, Promise<NodespaceSession>>()
  /** A note for a tool call's result, by the call's id. */
  const notes = new Map<string, string>()
  let hasShownReach = false

  function sessionFor(sessionID: string, isChild = false): Promise<NodespaceSession> {
    let held = sessions.get(sessionID)

    if (!held) {
      const session = createSession(host, intervalMs)
      // The first session that is not a child's takes the launch. A resumed
      // one is first seen in a hook other than `session.created`, and is not a child's.
      const isLaunched = !isChild && !hasClaimedLaunch

      hasClaimedLaunch ||= isLaunched

      held = session.start(directory, { sessionId: sessionID, isLaunched }).then(async reach => {
        // OpenCode has no status line. Reachability and the project are
        // shown once, for the first session the user opened themselves.
        if (!hasShownReach && !isChild) {
          hasShownReach = true

          // A notice that cannot be shown must not cost the session its plugin.
          try {
            await client.tui.showToast({
              body: {
                message: reach.text,
                variant: reach.kind === 'project' || reach.kind === 'no-project' ? 'info' : 'warning',
              },
            })
          } catch {
            // No terminal UI to show it in.
          }
        }

        return session
      })
      sessions.set(sessionID, held)
    }

    return held
  }

  /** A hook's step, which must neither stop OpenCode nor fail its request. */
  async function quietly(step: () => Promise<void>): Promise<void> {
    try {
      await step()
    } catch {
      // Nothing to add.
    }
  }

  return {
    event: ({ event }) =>
      quietly(async () => {
        if (event.type === 'session.created') {
          await sessionFor(event.properties.info.id, event.properties.info.parentID !== undefined)
        } else if (event.type === 'session.deleted') {
          const ended = sessions.get(event.properties.info.id)

          sessions.delete(event.properties.info.id)
          await (await ended)?.end()
        }
      }),

    // The environment of every command OpenCode runs for a session: without
    // the launch's variables, and with the one that names the session to the
    // CLI. OpenCode may merge this over its own environment, so a launch
    // variable is blanked here, which the CLI reads as no launch.
    'shell.env': async (input, output) => {
      await quietly(async () => {
        const env = input.sessionID ? (await sessionFor(input.sessionID)).commandEnv(output.env) : { ...output.env }

        for (const name of LAUNCH_VARIABLES) {
          delete env[name]

          if (process.env[name] !== undefined) {
            env[name] = ''
          }
        }

        output.env = Object.fromEntries(Object.entries(env).filter(([, value]) => value !== undefined)) as Record<
          string,
          string
        >
      })
    },

    // A user message: the skill list is read again, and a change to it is
    // added to the message as a part of its own.
    'chat.message': (input, output) =>
      quietly(async () => {
        const note = await (await sessionFor(input.sessionID)).prompt()

        if (note) {
          output.parts.push({
            id: `prt_nodespace_${Date.now().toString(36)}`,
            sessionID: input.sessionID,
            messageID: output.message.id,
            type: 'text',
            text: note,
            synthetic: true,
          })
        }
      }),

    // These two hooks carry an `experimental.` prefix: when OpenCode drops or
    // changes one, the section is not added and nothing else is affected.
    'experimental.chat.system.transform': (input, output) =>
      quietly(async () => {
        const section = input.sessionID ? (await sessionFor(input.sessionID)).section() : null

        if (section && Array.isArray(output.system)) {
          output.system.push(section)
        }
      }),

    'tool.execute.before': async input => {
      let refusal: string | null = null

      await quietly(async () => {
        const session = await sessionFor(input.sessionID)
        const verdict = await session.beforeTool()

        if (verdict && 'deny' in verdict) {
          refusal = verdict.deny
        } else if (verdict) {
          notes.set(input.callID, verdict.note)
        }
      })

      // Thrown outside `quietly`: this is how OpenCode refuses a tool call,
      // and the message is what the model reads.
      if (refusal !== null) {
        throw new Error(refusal)
      }
    },

    'tool.execute.after': (input, output) =>
      quietly(async () => {
        const note = notes.get(input.callID)

        notes.delete(input.callID)
        await (await sessionFor(input.sessionID)).afterTool(
          shellLine(input.tool, input.args),
          typeof output.output === 'string' ? output.output : '',
        )

        if (note) {
          output.output = `${typeof output.output === 'string' ? output.output : ''}\n\n${note}`
        }
      }),
  }
}
