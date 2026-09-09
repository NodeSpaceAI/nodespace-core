/**
 * Pi extension shim — registers NodeSpace knowledge graph tools.
 *
 * Pi (pi.dev / earendil-works/pi) auto-discovers extensions from
 * `~/.pi/agent/extensions/*.ts` (global) or `.pi/extensions/*.ts`
 * (project-local) — a *different* directory from the skill folder this file
 * installs into (`~/.pi/agent/skills/nodespace/`). SKILL.md documents this
 * file's existence and where to copy it from; Pi does not auto-load a `.ts`
 * sitting inside a skill folder as an extension.
 *
 * `pi.registerTool()` is part of Pi's extension runtime (an `ExtensionAPI`
 * passed into this file's default export) and is available as `pi` at
 * module scope once Pi loads the file as an extension.
 *
 * Pi's own `registerTool` examples use TypeBox's `Type.Object()` for
 * `parameters`, which is a real npm dependency (`typebox`) Pi itself
 * declares. Extensions are copied here as standalone scripts with no npm
 * context (see the runCLI note below), so this shim passes plain
 * JSON-Schema-shaped objects instead of importing `typebox` — TypeBox
 * schemas are JSON Schema at their core, and the other three shims
 * (Codex/OpenCode's plugin runtimes, Claude Code's hook registration) all
 * accept the same shape. If Pi's tool executor turns out to require
 * TypeBox's own `[Kind]`-tagged runtime objects rather than duck-typing
 * plain JSON Schema, this will need the real `typebox` import plus a
 * package.json + install step alongside the extension.
 */

// runCLI and NodespaceCLIError are intentionally inlined (not imported) in each
// shim. Shims are copied as standalone scripts into agent session temp dirs with
// no npm context, so module resolution is unavailable at runtime. Any change to
// runCLI must be replicated across all four shims in packages/skill/shims/.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

class NodespaceCLIError extends Error {
  constructor(
    message: string,
    public readonly exitCode: number | null,
    public readonly stderr: string
  ) {
    super(message);
    this.name = 'NodespaceCLIError';
  }
}

async function runCLI(args: string[]): Promise<string> {
  try {
    const { stdout } = await execFileAsync('nodespace', ['--json', ...args], {
      env: process.env,
      timeout: 30_000
    });
    return stdout.trim();
  } catch (err: unknown) {
    if (
      err !== null &&
      typeof err === 'object' &&
      'code' in err &&
      (err as NodeJS.ErrnoException).code === 'ENOENT'
    ) {
      throw new NodespaceCLIError(
        'nodespace CLI not found on $PATH. Install NodeSpace and ensure the nodespace binary is accessible.',
        null,
        ''
      );
    }
    if (err !== null && typeof err === 'object' && 'stderr' in err && 'code' in err) {
      const e = err as { stderr: string; code: number | null; message: string };
      throw new NodespaceCLIError(e.message, e.code, e.stderr);
    }
    throw err;
  }
}

interface PiExtensionAPI {
  registerTool(spec: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
    execute: (
      toolCallId: string,
      params: Record<string, unknown>
    ) => Promise<{ content: Array<{ type: string; text: string }>; details: Record<string, unknown> }>;
  }): void;
}

function textResult(text: string): {
  content: Array<{ type: string; text: string }>;
  details: Record<string, unknown>;
} {
  return { content: [{ type: 'text', text }], details: {} };
}

export default function (pi: PiExtensionAPI): void {
  pi.registerTool({
    name: 'nodespace_search_semantic',
    description: 'Search the NodeSpace knowledge graph using natural language.',
    parameters: {
      type: 'object',
      properties: {
        query: { type: 'string', description: 'Natural language search query.' },
        limit: { type: 'number', description: 'Maximum number of results (default 10).' }
      },
      required: ['query']
    },
    execute: async (_toolCallId, { query, limit }) => {
      const args = ['search', String(query)];
      if (typeof limit === 'number') args.push('--limit', String(limit));
      return textResult(await runCLI(args));
    }
  });

  pi.registerTool({
    name: 'nodespace_get_node',
    description: 'Fetch a single NodeSpace node by its ID.',
    parameters: {
      type: 'object',
      properties: {
        node_id: { type: 'string', description: 'ID of the node to fetch.' }
      },
      required: ['node_id']
    },
    execute: async (_toolCallId, { node_id }) => {
      return textResult(await runCLI(['node', 'get', String(node_id)]));
    }
  });

  pi.registerTool({
    name: 'nodespace_create_node',
    description: 'Create a new node in the NodeSpace knowledge graph.',
    parameters: {
      type: 'object',
      properties: {
        type: { type: 'string', description: 'Node type (e.g. "text", "task").' },
        content: { type: 'string', description: 'Markdown content of the node.' },
        parent_id: { type: 'string', description: 'Parent node ID (optional).' }
      },
      required: ['type', 'content']
    },
    execute: async (_toolCallId, { type, content, parent_id }) => {
      const args = ['node', 'create', '--type', String(type), '--content', String(content)];
      if (parent_id !== undefined) args.push('--parent', String(parent_id));
      return textResult(await runCLI(args));
    }
  });

  pi.registerTool({
    name: 'nodespace_update_node',
    description: 'Update the content of an existing NodeSpace node.',
    parameters: {
      type: 'object',
      properties: {
        node_id: { type: 'string', description: 'ID of the node to update.' },
        content: { type: 'string', description: 'New markdown content.' }
      },
      required: ['node_id', 'content']
    },
    execute: async (_toolCallId, { node_id, content }) => {
      return textResult(await runCLI(['node', 'update', String(node_id), '--content', String(content)]));
    }
  });

  pi.registerTool({
    name: 'nodespace_get_children',
    description: 'List the direct children of a NodeSpace node.',
    parameters: {
      type: 'object',
      properties: {
        node_id: { type: 'string', description: 'ID of the parent node.' }
      },
      required: ['node_id']
    },
    execute: async (_toolCallId, { node_id }) => {
      return textResult(await runCLI(['node', 'children', String(node_id)]));
    }
  });
}
