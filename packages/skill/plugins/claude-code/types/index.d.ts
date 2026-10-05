/** One skill as the list names it. `modifiedAt` is what tells an edit apart. */
export type NodespaceSkill = {
  id: string
  title: string
  description: string
  modifiedAt: string
}

/**
 * What the plugin read at session start, and again after a compaction or a
 * `/clear`. `section` is the system prompt text built from that read: it is
 * kept, not rebuilt, so the prompt stays the same until the next read.
 */
export type NodespaceSession = {
  reach: 'ok' | 'no-cli' | 'unreachable'
  /** `NODESPACE_DATABASE` as the session's environment held it at the read. */
  database: string | null
  project: { id: string; title: string } | null
  section: string | null
  skills: NodespaceSkill[]
  listVersion: string
  /** Set when the conversation was compacted or cleared: read again before use. */
  stale: 'compact' | 'clear' | null
}

/** One thing a context read returned beside the item, and what marks its state. */
export type NodespaceContextPart = {
  key: string
  label: string
  stamp: string
}

/** The item the session is working on, as its context last read. */
export type NodespaceItem = {
  id: string
  /** The `--path` flags of the read, repeated on every later one. */
  paths: string[]
  title: string
  contextVersion: string
  nodeVersion: number
  /** The node's own values, flattened to `name -> JSON`, to say what changed. */
  fields: Record<string, string>
  parts: NodespaceContextPart[]
}

export type NodespaceWatch = {
  item: NodespaceItem | null
  lastCheckedAt: number
  /** Why tool calls are refused; cleared when the user next speaks. */
  blocked: string | null
}

declare module 'claude-code' {
  interface PluginState {
    nodespace: {
      session: NodespaceSession | null
      /** Ids of the skills this conversation has been handed in full. */
      fetched: string[]
      watch: NodespaceWatch
    }
  }
}
