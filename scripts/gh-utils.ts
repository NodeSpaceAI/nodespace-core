#!/usr/bin/env bun

/**
 * GitHub utilities for NodeSpace, for the public-intake side only.
 *
 * GitHub issues are the public intake: the team's own work is tracked in
 * NodeSpace, never filed, assigned or given a status on GitHub. This script
 * views an intake issue, comments on one, opens a pull request, and files or
 * updates the de-duplicated issue a failing scheduled check reports.
 *
 * PURE TYPESCRIPT - NO SHELL COMMANDS
 * Eliminates Claude Code approval prompts by using direct API calls only
 *
 * Usage:
 *   bun run gh:view 45
 *   bun run gh:comment 45 --body "Comment text"
 *   bun run gh:tracking-issue --title "Title" --body-file body.md
 */

import { GitHubClient } from "./github-client.ts";

class NodeSpaceGitHubManager {
  private client: GitHubClient;

  // Accepts an injected client so tests can stub GitHub API access instead
  // of hitting the network (and instead of depending on which account is
  // actually authenticated when the suite runs).
  constructor(client?: GitHubClient) {
    this.client = client ?? new GitHubClient();
  }

  async viewIssue(issueNumber: number, web: boolean = false) {
    try {
      if (web) {
        console.log(`🌐 View issue #${issueNumber} at: https://github.com/NodeSpaceAI/nodespace-core/issues/${issueNumber}`);
        return;
      }
      
      const issue = await this.client.getIssue(issueNumber);
      
      console.log(`#${issue.number}: ${issue.title}`);
      console.log(`State: ${issue.state}`);
      console.log(`Assignees: ${issue.assignees.map(a => a.login).join(", ") || "None"}`);
      console.log(`Labels: ${issue.labels.map(l => l.name).join(", ") || "None"}`);
      console.log(`\nBody:\n${issue.body}`);
      
    } catch (error: unknown) {
      console.error(`❌ Failed to view issue #${issueNumber}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  async createPR(title: string, body: string, draft: boolean = false) {
    try {
      const currentBranch = this.client.getCurrentBranch();
      const pr = await this.client.createPullRequest(title, body, currentBranch, "main", draft);
      
      console.log("✅ Pull request created:");
      console.log(`#${pr.number}: ${title}`);
      console.log(`URL: ${pr.url}`);
      
      return pr;
    } catch (error: unknown) {
      console.error(`❌ Failed to create PR: ${error instanceof Error ? error.message : String(error)}`);
      throw error;
    }
  }

  // Complete PR workflow
  async createPRWorkflow(issueNumber: number, title?: string) {
    console.log(`📝 Creating PR for issue #${issueNumber}...\n`);

    // Remind about quality checks
    console.log("⚠️  Ensure you've run: bun run quality:fix");

    try {
      // Get issue details for PR
      const issue = await this.client.getIssue(issueNumber);

      const prTitle = title || issue.title;
      // The closing keyword is for an issue that came from outside the team,
      // so its reporter is notified when the change lands.
      const prBody = `Closes #${issueNumber}`;

      // Create the PR
      const pr = await this.createPR(prTitle, prBody);

      console.log("\n🎉 PR workflow completed successfully!");
      return pr;

    } catch (error: unknown) {
      console.error(`❌ PR workflow failed: ${error instanceof Error ? error.message : String(error)}`);
      return false;
    }
  }

  // Add comment to issue or PR
  async addComment(issueNumber: number, body: string) {
    try {
      const result = await this.client.addPRComment(issueNumber, body);

      console.log(`✅ Comment added to issue/PR #${issueNumber}`);
      console.log(`Comment ID: ${result.id}`);
      console.log(`URL: ${result.url}`);

      return result;
    } catch (error: unknown) {
      console.error(`❌ Failed to add comment to #${issueNumber}: ${error instanceof Error ? error.message : String(error)}`);
      throw error;
    }
  }

  // Find an open issue with this exact title and comment on it instead of
  // filing a duplicate; create a new one only if no match exists.
  //
  // Built for scheduled monitoring workflows (e.g. a nightly Gatekeeper or
  // Homebrew-tap-drift check) whose failure step would otherwise file a
  // fresh tracking issue on every single failed run. Title matching is
  // exact (`===`), never a substring/prefix match — a title that merely
  // starts with or contains `title` must not be treated as the same
  // tracking issue.
  //
  // Matches via `GitHubClient#listIssues` (REST `issues.listForRepo`), the
  // regular strongly-consistent issues-list endpoint -- deliberately never
  // GitHub's `--search`/search-index endpoint, which is only eventually
  // consistent and could still report "no match" for an issue this same
  // helper just created moments earlier (e.g. two closely-spaced runs
  // around a schedule boundary), causing a duplicate anyway.
  async findOrCreateTrackingIssue(options: {
    title: string;
    body: string;
    labels?: string[];
  }): Promise<{ number: number; url: string; action: "commented" | "created" }> {
    try {
      const openIssues = await this.client.listIssues({ state: "open" });
      // `issue.state === "open"` is checked again here, redundantly with the
      // `{ state: "open" }` request above -- defense in depth against ever
      // matching a closed issue (and silently absorbing a comment into it
      // instead of filing a fresh one) if that server-side filter is ever
      // loosened, mocked incorrectly in a future test, or an API contract
      // changes out from under this call.
      const existing = openIssues.find((issue) => issue.state === "open" && issue.title === options.title);

      if (existing) {
        const comment = await this.client.addPRComment(existing.number, options.body);
        console.log(`Existing open issue #${existing.number} -- commenting instead of filing a duplicate.`);
        console.log(`Comment URL: ${comment.url}`);
        return { number: existing.number, url: comment.url, action: "commented" };
      }

      const issue = await this.client.createIssue(options.title, options.body, options.labels);
      console.log(`No existing open issue found -- created #${issue.number}`);
      console.log(`URL: ${issue.url}`);
      // GitHubClient.createIssue itself warns (console.warn) when the
      // issue could not be added to the project board, or its status
      // could not be set -- e.g. every
      // invocation of this helper from a scheduled CI workflow, whose
      // token can never carry Projects v2 write scope. No need to
      // re-derive that from `issue.addedToProject` here too.
      return { number: issue.number, url: issue.url, action: "created" };
    } catch (error: unknown) {
      console.error(
        `❌ Failed to find-or-create tracking issue "${options.title}": ${error instanceof Error ? error.message : String(error)}`,
      );
      throw error;
    }
  }
}

// CLI Interface

// Shared --body / --body-file parsing for the CLI cases below
// (issues:comment/pr:comment, tracking-issue:find-or-create).
// Prefers --body when both are given, matching each call site's previous
// behavior. Returns undefined only when NEITHER flag is present -- an
// explicit `--body ""` is returned as `""`, not treated as absent.
//
// Each call site used to re-implement this with `args[bodyIndex + 1]`
// truthiness checks, which conflate "the flag wasn't given" with "the flag
// was given an empty string": for issues:comment/pr:comment specifically,
// that meant an explicit `--body ""` fell through to that case's own
// positional-arg fallback and silently picked up a stale `args[2]` value
// instead of honoring the empty override.
async function parseBodyArg(args: string[]): Promise<string | undefined> {
  const bodyIndex = args.indexOf("--body");
  const bodyFileIndex = args.indexOf("--body-file");

  if (bodyIndex !== -1 && args[bodyIndex + 1] !== undefined) {
    return args[bodyIndex + 1];
  }

  if (bodyFileIndex !== -1 && args[bodyFileIndex + 1] !== undefined) {
    try {
      return await Bun.file(args[bodyFileIndex + 1]).text();
    } catch (error: unknown) {
      console.error(`❌ Failed to read body file: ${error instanceof Error ? error.message : String(error)}`);
      process.exit(1);
    }
  }

  return undefined;
}

async function main() {
  const args = process.argv.slice(2);
  const command = args[0];
  const manager = new NodeSpaceGitHubManager();

  try {
    switch (command) {
      case "issues:view": {
        const issueNumber = parseInt(args[1]);
        const web = args.includes("--web");
        
        if (!issueNumber) {
          console.error("Usage: bun run scripts/gh-utils.ts issues:view 45 [--web]");
          process.exit(1);
        }
        
        await manager.viewIssue(issueNumber, web);
        break;
      }

      case "pr:create": {
        const issueNumber = parseInt(args[1]);
        const title = args[2];

        if (!issueNumber) {
          console.error('Usage: bun run scripts/gh-utils.ts pr:create 45 "Optional title"');
          process.exit(1);
        }

        await manager.createPRWorkflow(issueNumber, title);
        break;
      }

      case "issues:comment":
      case "pr:comment": {
        const issueNumber = parseInt(args[1]);
        const parsedBody = await parseBodyArg(args);
        // Backward compatibility: a direct string as the second positional
        // arg, only when neither --body nor --body-file was given.
        const body = parsedBody !== undefined ? parsedBody : args[2] || "";

        if (!issueNumber || !body) {
          console.error('Usage:');
          console.error('  bun run gh:comment 45 --body "Comment text"');
          console.error('  bun run gh:comment 45 --body-file /path/to/comment.md');
          console.error('  bun run gh:comment 45 "Comment text"  # Direct (backward compat)');
          process.exit(1);
        }

        await manager.addComment(issueNumber, body);
        break;
      }

      case "tracking-issue:find-or-create": {
        const titleIndex = args.indexOf("--title");
        const labelsIndex = args.indexOf("--labels");

        const title = titleIndex !== -1 ? args[titleIndex + 1] : undefined;
        const body = await parseBodyArg(args);

        // `.filter(Boolean)` drops empty segments from something like
        // "bug,,foundation" (a stray comma) -- GitHub's API 422s on an
        // empty-string label, which a malformed `--labels` value would
        // otherwise only surface as an opaque API error at request time.
        const labels =
          labelsIndex !== -1 && args[labelsIndex + 1]
            ? args[labelsIndex + 1]
                .split(",")
                .map((l) => l.trim())
                .filter(Boolean)
            : undefined;

        // `body === undefined` (not `!body`): parseBodyArg's contract is
        // that an explicit `--body ""` comes back as `""`, distinct from
        // neither flag being given at all -- checking truthiness here
        // would silently reintroduce the exact "empty is treated as
        // absent" ambiguity that helper exists to eliminate.
        if (!title || body === undefined) {
          console.error("Usage:");
          console.error('  bun run gh:tracking-issue --title "Title" --body "Body" [--labels "label1,label2"]');
          console.error('  bun run gh:tracking-issue --title "Title" --body-file /path/to/body.md [--labels "label1,label2"]');
          process.exit(1);
        }

        await manager.findOrCreateTrackingIssue({ title, body, labels });
        break;
      }

      case "help":
      default:
        console.log(`
NodeSpace GitHub utilities (public intake only)

GitHub issues are the public intake. The team's own work is tracked in
NodeSpace: do not file, assign or set the status of a GitHub issue for it.

  bun run gh:view 45                          # View an intake issue
  bun run gh:view 45 --web                    # Print its browser URL
  bun run gh:comment 45 --body "Comment text" # Reply on an issue or pull request
  bun run gh:comment 45 --body-file /path/to/comment.md
  bun run gh:pr 45 ["Title"]                  # Open a pull request with body "Closes #45"
                                              # (only for an issue that came from outside);
                                              # works only from a branch named like its
                                              # remote branch. From a worktree- branch use:
                                              # gh pr create --head <remote-branch> --body "Closes #45"
  bun run gh:tracking-issue --title "Title" --body-file body.md --labels "bug"
                                              # For a scheduled check that fails: comment
                                              # on the open issue with this exact title,
                                              # or create one if none exists

All commands use the TypeScript API (no Claude Code approval prompts).
        `);
        break;
    }
  } catch (error: unknown) {
    console.error(`❌ Error: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}

if (import.meta.main) {
  main();
}

export { NodeSpaceGitHubManager };
