#!/usr/bin/env bun

/**
 * Pure TypeScript GitHub API Client for NodeSpace
 * 
 * NO SHELL COMMANDS - Uses only direct API calls to eliminate Claude Code prompts
 * Authentication via gh CLI token automatically detected
 */

import { Octokit, type RestEndpointMethodTypes } from "@octokit/rest";
import { readFileSync, existsSync, statSync } from "fs";
import { homedir } from "os";
import path from "path";

interface GitHubIssue {
  number: number;
  title: string;
  state: string;
  assignees: Array<{ login: string }>;
  labels: Array<{ name: string }>;
  body: string;
}

export class GitHubClient {
  private octokit: Octokit;
  
  private readonly owner = "NodeSpaceAI";
  private readonly repo = "nodespace-core";

  constructor(token?: string) {
    const authToken = token || this.getGitHubToken();
    this.octokit = new Octokit({
      auth: authToken,
    });
  }

  /**
   * Get GitHub token from environment or gh CLI
   */
  private getGitHubToken(): string {
    // Try environment variable first
    if (process.env.GITHUB_TOKEN) {
      return process.env.GITHUB_TOKEN;
    }

    // Try gh CLI token command (modern keyring authentication)
    try {
      const result = Bun.spawnSync(["gh", "auth", "token"], {
        stdout: "pipe",
        stderr: "pipe"
      });
      
      if (result.exitCode === 0) {
        const token = result.stdout.toString().trim();
        if (token && token.startsWith("gh")) {
          return token;
        }
      }
    } catch {
      // Continue trying other methods
    }

    // Try gh CLI config file (legacy token storage)
    const possibleConfigPaths = [
      path.join(homedir(), ".config", "gh", "hosts.yml"),
      path.join(homedir(), ".config", "gh", "config.yml")
    ];

    for (const configPath of possibleConfigPaths) {
      if (existsSync(configPath)) {
        try {
          const configContent = readFileSync(configPath, "utf-8");
          const tokenMatch = configContent.match(/oauth_token:\s*([^\s\n\r]+)/);
          
          if (tokenMatch) {
            return tokenMatch[1];
          }
        } catch {
          // Continue trying other methods
        }
      }
    }

    throw new Error(`GitHub token not found. Options:
1. Set GITHUB_TOKEN environment variable
2. Run: gh auth login
3. Ensure gh CLI is properly configured`);
  }

  /**
   * List issues (no shell commands)
   */
  async listIssues(options: {
    state?: "open" | "closed" | "all";
    labels?: string[];
    assignee?: string;
  } = {}): Promise<GitHubIssue[]> {
    const params: RestEndpointMethodTypes["issues"]["listForRepo"]["parameters"] = {
      owner: this.owner,
      repo: this.repo,
      state: options.state || "open",
      per_page: 100
    };

    if (options.labels?.length) {
      params.labels = options.labels.join(",");
    }

    if (options.assignee) {
      params.assignee = options.assignee;
    }

    // Paginated (not a single 100-item page): a caller doing exact-title
    // dedup against "every open issue" (findOrCreateTrackingIssue) must not
    // silently miss a match past the first page once the repo has more
    // open issues than that -- the hand-rolled `gh issue list` this
    // replaced used `--limit 200` specifically to stay ahead of the real
    // count, and a narrower cap here would reintroduce the same
    // duplicate-tracking-issue risk that helper exists to prevent.
    const data = await this.octokit.paginate(this.octokit.rest.issues.listForRepo, params);

    // The REST "list issues" endpoint returns pull requests mixed in with
    // real issues (GitHub represents every PR as an issue under the hood);
    // an issue-only caller must filter them out itself, keyed on the
    // `pull_request` field only a PR carries. `gh issue list` (what this
    // client replaced) already excludes PRs, so this keeps the same
    // contract -- without it, an open PR whose title happens to match an
    // issue title could be mistaken for that issue by an exact-title match
    // (e.g. findOrCreateTrackingIssue).
    return data
      .filter(issue => !("pull_request" in issue))
      .map(issue => ({
        number: issue.number,
        title: issue.title,
        state: issue.state,
        assignees: issue.assignees?.map(a => ({ login: a.login })) || [],
        labels: issue.labels?.map(l => ({ name: (typeof l === 'string' ? l : l.name) || "" })) || [],
        body: issue.body || ""
      }));
  }

  /**
   * Get single issue details (no shell commands)
   */
  async getIssue(issueNumber: number): Promise<GitHubIssue> {
    const response = await this.octokit.rest.issues.get({
      owner: this.owner,
      repo: this.repo,
      issue_number: issueNumber
    });

    const issue = response.data;
    return {
      number: issue.number,
      title: issue.title,
      state: issue.state,
      assignees: issue.assignees?.map(a => ({ login: a.login })) || [],
      labels: issue.labels?.map(l => ({ name: (typeof l === 'string' ? l : l.name) || "" })) || [],
      body: issue.body || ""
    };
  }

  /**
   * Create issue (no shell commands)
   */
  async createIssue(
    title: string,
    body: string,
    labels?: string[]
  ): Promise<{ number: number; url: string }> {
    const response = await this.octokit.rest.issues.create({
      owner: this.owner,
      repo: this.repo,
      title,
      body,
      labels: labels || []
    });

    return {
      number: response.data.number,
      url: response.data.html_url
    };
  }

  /**
   * Create pull request (no shell commands)
   */
  async createPullRequest(
    title: string, 
    body: string, 
    head: string, 
    base: string = "main",
    draft: boolean = false
  ): Promise<{ number: number; url: string }> {
    const response = await this.octokit.rest.pulls.create({
      owner: this.owner,
      repo: this.repo,
      title,
      body,
      head,
      base,
      draft
    });

    return {
      number: response.data.number,
      url: response.data.html_url
    };
  }

  /**
   * Add a single review comment to a PR
   */
  async addPRComment(
    prNumber: number,
    body: string,
    commitId?: string,
    path?: string,
    line?: number
  ): Promise<{ id: number; url: string }> {
    if (path && line && commitId) {
      // Inline comment on specific line
      const response = await this.octokit.rest.pulls.createReviewComment({
        owner: this.owner,
        repo: this.repo,
        pull_number: prNumber,
        body,
        commit_id: commitId,
        path,
        line
      });

      return {
        id: response.data.id,
        url: response.data.html_url
      };
    } else {
      // General PR comment
      const response = await this.octokit.rest.issues.createComment({
        owner: this.owner,
        repo: this.repo,
        issue_number: prNumber,
        body
      });

      return {
        id: response.data.id,
        url: response.data.html_url
      };
    }
  }

  /**
   * Get existing reviews for a PR

  /**
   * Resolve the real git directory for the current working directory.
   *
   * In a normal checkout `.git` is a directory. In a linked worktree (e.g.
   * `git worktree add`, EnterWorktree) `.git` is a *file* containing a
   * `gitdir: <path>` pointer to the worktree's git dir under the main repo's
   * `.git/worktrees/<name>`. Both cases hold `HEAD` directly, so resolving the
   * pointer lets the no-shell git reads work from worktrees too.
   *
   * Returns null when the cwd is not inside a git repository.
   */
  private resolveGitDir(): string | null {
    const dotGit = path.join(process.cwd(), ".git");
    if (!existsSync(dotGit)) {
      return null;
    }

    if (statSync(dotGit).isDirectory()) {
      return dotGit;
    }

    // Worktree: `.git` is a file with a `gitdir: <path>` pointer.
    const pointer = readFileSync(dotGit, "utf-8").trim();
    const match = pointer.match(/^gitdir:\s*(.+)$/);
    if (!match) {
      return null;
    }
    const gitDir = match[1].trim();
    return path.isAbsolute(gitDir)
      ? gitDir
      : path.resolve(process.cwd(), gitDir);
  }

  /**
   * Get current git branch (no shell commands - reads .git directly).
   * Works in both normal checkouts and linked worktrees.
   */
  getCurrentBranch(): string {
    try {
      const gitDir = this.resolveGitDir();
      if (!gitDir) {
        throw new Error("Not in a git repository");
      }

      const headPath = path.join(gitDir, "HEAD");
      if (!existsSync(headPath)) {
        throw new Error("Not in a git repository");
      }

      const headContent = readFileSync(headPath, "utf-8").trim();
      if (headContent.startsWith("ref: refs/heads/")) {
        return headContent.replace("ref: refs/heads/", "");
      }

      // Detached HEAD state
      return headContent.substring(0, 7);
    } catch (error: unknown) {
      throw new Error(`Failed to get current branch: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}
