import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { CONSENT_RULES, ORIENTATION } from '../shipped-text.js';
import { AGENTS, SHARED_PLUGIN_DIR } from '../agents.js';

const PACKAGE_ROOT = join(import.meta.dirname, '../..');

/**
 * The value of `const <name> = [ ...string literals ].join('\n')` in a
 * plugin's source. A plugin is installed as files of its own, so it holds the
 * text as a literal and cannot be imported here for it.
 */
function constantIn(source: string, name: string): string | undefined {
  const literal = new RegExp(`const ${name} = (\\[\\n(?:\\s+(?:'(?:[^'\\\\]|\\\\.)*'|"(?:[^"\\\\]|\\\\.)*"),\\n)+\\])\\.join\\('\\\\n'\\)`).exec(source)?.[1];
  if (literal === undefined) return undefined;
  return (new Function(`return ${literal}`)() as string[]).join('\n');
}

/**
 * The file each plugin keeps its copy in: the Claude Code plugin's hooks
 * module, and the module the Pi extension and the OpenCode plugin both install.
 */
const COPIES: Record<string, string> = {
  'claude-code': 'plugins/claude-code/hooks/register.ts',
  pi: `${SHARED_PLUGIN_DIR}/nodespace-session.ts`,
  opencode: `${SHARED_PLUGIN_DIR}/nodespace-session.ts`,
};

describe('the text shipped to every agent', () => {
  // One wording, whichever harness an agent runs in (ADR-093 §5, §7). A copy
  // edited alone would give one harness's agents different confirmation rules.
  it.each(Object.entries(COPIES))('is the same in the %s plugin as in the one source', (_name, file) => {
    const source = readFileSync(join(PACKAGE_ROOT, file), 'utf8');

    expect(constantIn(source, 'ORIENTATION'), `${file} holds no ORIENTATION literal`).toBe(ORIENTATION);
    expect(constantIn(source, 'CONSENT_RULES'), `${file} holds no CONSENT_RULES literal`).toBe(CONSENT_RULES);
  });

  it('checks a copy for every harness that has a plugin, in a file that plugin installs', () => {
    const withPlugin = AGENTS.filter(agent => agent.plugin).map(agent => agent.name);
    expect(Object.keys(COPIES).sort()).toEqual([...withPlugin].sort());

    for (const agent of AGENTS) {
      const plugin = agent.plugin;
      if (!plugin) continue;
      const installed = [
        ...plugin.files.map(file => `${plugin.dir}/${file}`),
        ...(plugin.shared ?? []).map(([file]) => `${SHARED_PLUGIN_DIR}/${file}`),
      ];
      expect(installed, `${agent.name} does not install ${COPIES[agent.name]}`).toContain(COPIES[agent.name]);
    }
  });

  it('reads a literal out of a source file as its value', () => {
    const source = "const SAMPLE = [\n  'a `b`',\n  \"it's\",\n].join('\\n')\n";
    expect(constantIn(source, 'SAMPLE')).toBe("a `b`\nit's");
    expect(constantIn(source, 'OTHER')).toBeUndefined();
  });
});
