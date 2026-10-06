/**
 * One skill as the list names it: its title and when to use it (`use_for` in
 * the CLI's listing). `modifiedAt` is what tells an edit apart.
 */
export type NodespaceSkill = {
  id: string
  title: string
  useFor: string
  modifiedAt: string
}

/**
 * What NodeSpace named when it launched this session: the PTY session's id
 * and what the session was launched for (`''` for nothing).
 */
export type NodespaceLaunch = {
  session: string
  launchedFor: string
}

/**
 * What the plugin read at session start, and again after a compaction or a
 * `/clear`. `section` is the system prompt text built from that read: it is
 * kept, not rebuilt, so the prompt stays the same until the next read.
 */
export type NodespaceSession = {
  /** `NODESPACE_DATABASE` as the session's environment held it at the read. */
  database: string | null
  project: { id: string; title: string } | null
  section: string | null
  skills: NodespaceSkill[]
  listVersion: string
  /**
   * Set in a session NodeSpace launched. Read from the environment once and
   * kept here, since the variables are then removed from it.
   */
  launch: NodespaceLaunch | null
  /**
   * In a session NodeSpace launched for an item: that item's context, as a
   * note for the next prompt. `null` once it has been delivered.
   */
  opening: string | null
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
