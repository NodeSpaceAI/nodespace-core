/**
 * Reads the Rust core node type registry
 * (`packages/nodespace-types/src/core_type.rs`) so frontend mirrors of it can be
 * pinned to it by tests.
 */

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export interface RustCoreType {
  /** The `CoreNodeType` variant name, e.g. `AiChat`. */
  variant: string;
  /** The stored `node_type`, e.g. `ai-chat`. */
  id: string;
  /** The parent's id, or null. */
  parent: string | null;
  abstract: boolean;
  /** The effective rule: a type is mentionable only when its whole chain is. */
  mentionable: boolean;
}

/** A file in the `nodespace-types` crate, resolved from this file so any runner cwd works. */
export function nodespaceTypesSource(file: string): string {
  return readFileSync(resolve(__dirname, '../../../../nodespace-types/src', file), 'utf8');
}

interface Arm {
  id: string;
  parent: string | null;
  abstract: boolean;
  ownMentionable: boolean;
}

/** Every core type in `CoreNodeType::ALL` order, as the Rust registry declares it. */
export function rustCoreTypes(): RustCoreType[] {
  const source = nodespaceTypesSource('core_type.rs');

  const allStart = source.indexOf('pub const ALL: [CoreNodeType;');
  if (allStart < 0) throw new Error('CoreNodeType::ALL not found in core_type.rs');
  const allBody = source.slice(allStart, source.indexOf('];', allStart));
  const order = [...allBody.matchAll(/CoreNodeType::(\w+)/g)].map((m) => m[1]);

  const infoStart = source.indexOf('pub const fn info(self)');
  if (infoStart < 0) throw new Error('CoreNodeType::info not found in core_type.rs');
  const matchStart = source.indexOf('match self {', infoStart);
  const infoBody = source.slice(matchStart, source.indexOf('pub const fn as_str', matchStart));

  const arms = new Map<string, Arm>();
  const starts = [...infoBody.matchAll(/^\s*Self::(\w+) =>/gm)];
  starts.forEach((m, i) => {
    const chunk = infoBody.slice(m.index, starts[i + 1]?.index ?? infoBody.length);
    const id = chunk.match(/"([a-z][a-z-]*)"/)?.[1];
    if (!id) throw new Error(`no id literal in the ${m[1]} arm of CoreNodeType::info`);
    arms.set(m[1], {
      id,
      parent: chunk.match(/parent:\s*Some\((?:CoreNodeType|Self)::(\w+)\)/)?.[1] ?? null,
      abstract: /is_abstract:\s*true/.test(chunk),
      ownMentionable: !chunk.includes('not_mentionable()')
    });
  });

  const effectiveMentionable = (variant: string): boolean => {
    const arm = arms.get(variant);
    if (!arm) throw new Error(`unknown variant ${variant}`);
    return arm.ownMentionable && (arm.parent === null || effectiveMentionable(arm.parent));
  };

  return order.map((variant) => {
    const arm = arms.get(variant);
    if (!arm) throw new Error(`${variant} is in CoreNodeType::ALL but has no info() arm`);
    return {
      variant,
      id: arm.id,
      parent: arm.parent === null ? null : (arms.get(arm.parent)?.id ?? null),
      abstract: arm.abstract,
      mentionable: effectiveMentionable(variant)
    };
  });
}

/** `CoreNodeType` variant name to stored id, e.g. `AiChat` to `ai-chat`. */
export function rustVariantIds(): Record<string, string> {
  return Object.fromEntries(rustCoreTypes().map((t) => [t.variant, t.id]));
}
