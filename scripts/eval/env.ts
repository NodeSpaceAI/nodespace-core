/**
 * The eval environment contract, in one place.
 *
 * Both evals previously re-declared these defaults independently, so a change
 * to the test socket or default model had to be made in several files and was
 * silently wrong if it was not.
 *
 * The daemon, socket, and database are managed by the CALLER — this harness
 * never starts or seeds anything. It asserts the environment is usable
 * (see preflight.ts) and otherwise assumes it. Every variable this harness
 * reads, its default, and what overrides it, is `ENV_USAGE` below — that is
 * the full, current contract, not restated here.
 */

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Repository root, derived from this file's location. */
export const REPO_ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);

/** The prefix of a chat's model id when a configured remote provider serves it. */
export const REMOTE_MODEL_PREFIX = "openai-compat:";

/**
 * Where a chat's inference runs, from its model id: a remote provider's id is
 * `openai-compat:<config-id>:<model>`, anything else names a local model.
 */
export function providerOf(model: string): "native" | "openai-compat" {
  return model.startsWith(REMOTE_MODEL_PREFIX) ? "openai-compat" : "native";
}

export interface EvalEnv {
  /** Path to the `nodespace` CLI binary. */
  nsBin: string;
  /** Unix socket the CLI and daemon share. */
  socket: string;
  /**
   * Daemon log, scraped for per-turn tool calls.
   *
   * Consumed by scripts/aichat.ts (which reads NS_LOG from the environment
   * directly), not by the runner. Declared here so the contract lives in one
   * place rather than only in the child process.
   */
  log: string;
  /**
   * Model id recorded on chat nodes. A local model is asserted loaded by
   * preflight; a remote one (`openai-compat:<config-id>:<model>`) is asserted
   * configured on the served database.
   */
  model: string;
  /** Per-turn timeout in milliseconds. Consumed by aichat.ts, as with `log`. */
  timeoutMs: number;
  /** scripts/aichat.ts, the CLI driver every eval turn goes through. */
  aichat: string;
}

export function readEnv(): EvalEnv {
  return {
    nsBin: process.env.NS_BIN ?? join(REPO_ROOT, "target/release/nodespace"),
    socket: process.env.NODESPACED_SOCKET ?? "/tmp/nodespaced-test/daemon.sock",
    log: process.env.NS_LOG ?? "/tmp/nodespaced-test/daemon.log",
    model: process.env.NS_MODEL ?? "gemma-4-e4b-q4km",
    timeoutMs: Number(process.env.NS_TIMEOUT_MS ?? 180_000),
    aichat: join(REPO_ROOT, "scripts", "aichat.ts"),
  };
}

/** Human-readable rendering of the contract, for error messages and --help. */
export const ENV_USAGE = `Environment:
  NS_BIN             Path to the nodespace CLI (default: target/release/nodespace)
  NODESPACED_SOCKET  Socket shared with the daemon (default: /tmp/nodespaced-test/daemon.sock)
  NS_LOG             Daemon log, scraped for tool calls (default: /tmp/nodespaced-test/daemon.log)
  NS_MODEL           Model id to require (default: gemma-4-e4b-q4km), or a remote
                     provider's model, openai-compat:<config-id>:<model>, to run
                     against the provider configured on the served database
  NS_TIMEOUT_MS      Per-turn timeout in ms (default: 180000)`;
