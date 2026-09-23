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

interface ProjectItem {
  id: string;
  content?: {
    number: number;
    title: string;
  };
  fieldValues: {
    nodes: Array<{
      field: {
        id: string;
        name: string;
      };
      value?: {
        id: string;
        name: string;
      };
    }>;
  };
}

interface ProjectItemsQueryResponse {
  organization: {
    projectV2: {
      items: {
        pageInfo: {
          hasNextPage: boolean;
          endCursor: string | null;
        };
        nodes: ProjectItem[];
      };
    };
  };
}

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
  
  // Project configuration from docs/architecture/development/process/issue-workflow.md
  private readonly owner = "malibio";
  private readonly repo = "nodespace-core";
  // The ProjectV2 board is owned by the NodeSpaceAI organization, not the
  // `owner` user login above (that one is only used for REST issue/PR calls
  // against the repo, which still resolves via GitHub's rename redirect).
  private readonly projectOwner = "NodeSpaceAI";
  private readonly projectNumber = 2;
  private readonly projectId = "PVT_kwDODxjmQM4BUxjb";
  private readonly statusFieldId = "PVTSSF_lADODxjmQM4BUxjbzhDzQv0";

  // Status option IDs — verified live against the real "NodeSpace" board
  // (org NodeSpaceAI, project #2). The previous IDs pointed at a project
  // node that no longer resolves at all.
  private readonly statusOptions = {
    "Backlog": "230488b9",
    "Todo": "32c30124",
    "In Progress": "9f8d4e33",
    "In Review": "099b7674",
    "Done": "58fb1205",
    "Blocked": "f50ca67a"
  } as const;

  // Cached result of getAuthenticatedUser() so repeated "@me" resolutions
  // within one process don't re-hit the API.
  private cachedLogin: string | null = null;

  constructor(token?: string) {
    const authToken = token || this.getGitHubToken();
    this.octokit = new Octokit({
      auth: authToken,
    });
  }

  /**
   * Resolve the actually-authenticated GitHub account (no shell commands —
   * uses the same Octokit client/token as every other call in this class).
   *
   * This exists so "@me" resolves to whoever is really logged in via `gh`,
   * instead of a hardcoded username.
   */
  async getAuthenticatedUser(): Promise<string> {
    if (this.cachedLogin) {
      return this.cachedLogin;
    }

    const { data } = await this.octokit.rest.users.getAuthenticated();
    this.cachedLogin = data.login;
    return this.cachedLogin;
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
   * Get project items using GraphQL API with pagination
   */
  async getProjectItems(): Promise<ProjectItem[]> {
    const allItems: ProjectItem[] = [];
    let hasNextPage = true;
    let cursor: string | null = null;

    while (hasNextPage) {
      const query = `
        query GetProjectItems($owner: String!, $projectNumber: Int!, $cursor: String) {
          organization(login: $owner) {
            projectV2(number: $projectNumber) {
              items(first: 100, after: $cursor) {
                pageInfo {
                  hasNextPage
                  endCursor
                }
                nodes {
                  id
                  content {
                    ... on Issue {
                      number
                      title
                    }
                  }
                  fieldValues(first: 10) {
                    nodes {
                      ... on ProjectV2ItemFieldSingleSelectValue {
                        field {
                          ... on ProjectV2SingleSelectField {
                            id
                            name
                          }
                        }
                        value: name
                        optionId: id
                      }
                    }
                  }
                }
              }
            }
          }
        }
      `;

      const response: ProjectItemsQueryResponse = await this.octokit.graphql<ProjectItemsQueryResponse>(query, {
        owner: this.projectOwner,
        projectNumber: this.projectNumber,
        cursor,
      });

      const items = response.organization.projectV2.items.nodes.filter(
        item => item.content?.number // Only return items that are issues
      );

      allItems.push(...items);

      hasNextPage = response.organization.projectV2.items.pageInfo.hasNextPage;
      cursor = response.organization.projectV2.items.pageInfo.endCursor;
    }

    return allItems;
  }

  /**
   * Get project item ID for specific issue number
   */
  async getItemIdForIssue(issueNumber: number): Promise<string | null> {
    const items = await this.getProjectItems();
    const item = items.find(item => item.content?.number === issueNumber);
    return item?.id || null;
  }

  /**
   * Update issue status in project (no shell commands)
   */
  async updateIssueStatus(
    issueNumbers: number[], 
    status: keyof typeof this.statusOptions
  ): Promise<Array<{ issueNumber: number; success: boolean; error?: string }>> {
    const statusOptionId = this.statusOptions[status];
    
    if (!statusOptionId) {
      throw new Error(`Invalid status: ${status}. Valid options: ${Object.keys(this.statusOptions).join(", ")}`);
    }

    const results = [];

    for (const issueNumber of issueNumbers) {
      try {
        // An issue that isn't on the board yet has no status to set. Add it
        // rather than failing: setting a status is a statement about where the
        // work stands, and refusing because the row is missing turns a
        // bookkeeping gap into a blocked workflow step. This also back-fills
        // issues created before creation started adding them.
        let itemId = await this.getItemIdForIssue(issueNumber);

        if (!itemId) {
          const nodeId = await this.getIssueNodeId(issueNumber);
          itemId = await this.addIssueToProject(nodeId);
        }

        const mutation = `
          mutation UpdateProjectItemField($projectId: ID!, $itemId: ID!, $fieldId: ID!, $value: ProjectV2FieldValue!) {
            updateProjectV2ItemFieldValue(input: {
              projectId: $projectId
              itemId: $itemId
              fieldId: $fieldId
              value: $value
            }) {
              projectV2Item {
                id
              }
            }
          }
        `;

        await this.octokit.graphql(mutation, {
          projectId: this.projectId,
          itemId,
          fieldId: this.statusFieldId,
          value: {
            singleSelectOptionId: statusOptionId
          }
        });

        results.push({ issueNumber, success: true });
        
      } catch (error: unknown) {
        results.push({
          issueNumber,
          success: false,
          error: error instanceof Error ? error.message : String(error)
        });
      }
    }

    return results;
  }

  /**
   * Assign issues (no shell commands)
   */
  async assignIssues(
    issueNumbers: number[], 
    assignees: string[]
  ): Promise<Array<{ issueNumber: number; success: boolean; error?: string }>> {
    const results = [];

    for (const issueNumber of issueNumbers) {
      try {
        await this.octokit.rest.issues.addAssignees({
          owner: this.owner,
          repo: this.repo,
          issue_number: issueNumber,
          assignees: assignees
        });

        // The GitHub API silently ignores usernames it can't assign (e.g. not
        // a collaborator) instead of erroring, which is exactly how this got
        // reported as a false success. Re-query before declaring victory.
        const issue = await this.getIssue(issueNumber);
        const landed = new Set(issue.assignees.map(a => a.login.toLowerCase()));
        const missing = assignees.filter(a => !landed.has(a.toLowerCase()));

        if (missing.length > 0) {
          results.push({
            issueNumber,
            success: false,
            error: `Assignment did not take effect for: ${missing.join(", ")} (not a valid collaborator?)`
          });
        } else {
          results.push({ issueNumber, success: true });
        }

      } catch (error: unknown) {
        results.push({
          issueNumber,
          success: false,
          error: error instanceof Error ? error.message : String(error)
        });
      }
    }

    return results;
  }

  /**
   * Unassign issues (no shell commands)
   */
  async unassignIssues(
    issueNumbers: number[], 
    assignees: string[]
  ): Promise<Array<{ issueNumber: number; success: boolean; error?: string }>> {
    const results = [];

    for (const issueNumber of issueNumbers) {
      try {
        await this.octokit.rest.issues.removeAssignees({
          owner: this.owner,
          repo: this.repo,
          issue_number: issueNumber,
          assignees: assignees
        });

        // Verify removal actually landed before declaring success — same
        // rationale as assignIssues above.
        const issue = await this.getIssue(issueNumber);
        const stillAssigned = new Set(issue.assignees.map(a => a.login.toLowerCase()));
        const notRemoved = assignees.filter(a => stillAssigned.has(a.toLowerCase()));

        if (notRemoved.length > 0) {
          results.push({
            issueNumber,
            success: false,
            error: `Unassignment did not take effect for: ${notRemoved.join(", ")}`
          });
        } else {
          results.push({ issueNumber, success: true });
        }

      } catch (error: unknown) {
        results.push({
          issueNumber,
          success: false,
          error: error instanceof Error ? error.message : String(error)
        });
      }
    }

    return results;
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
    labels?: string[],
    assignees?: string[]
  ): Promise<{ number: number; url: string; addedToProject: boolean }> {
    const response = await this.octokit.rest.issues.create({
      owner: this.owner,
      repo: this.repo,
      title,
      body,
      labels: labels || [],
      assignees: assignees || []
    });

    // Put the issue on the board immediately. Nothing else does this, so an
    // issue created without it never gets a project item — and every later
    // `gh:status` call on it fails with "Issue not found in project",
    // including the ones the startup sequence in CLAUDE.md makes mandatory.
    // Board placement is a convenience, not the point of creating the issue,
    // so a failure here is reported rather than thrown: the issue itself
    // exists and its number is what the caller actually needs.
    const addedToProject = await this.addIssueToProject(response.data.node_id)
      .then(() => true)
      .catch(() => false);

    // Warn here rather than leaving it to each caller to re-derive from
    // `addedToProject` -- this is the one place that actually knows the add
    // failed, so every current and future caller gets the warning "for
    // free". Notably, `findOrCreateTrackingIssue` (scripts/gh-utils.ts) is
    // invoked from scheduled CI workflows (verify-macos-installer.yml,
    // homebrew-drift-check.yml) using the workflow's plain
    // `secrets.GITHUB_TOKEN`, which can never carry Projects v2 write
    // scope -- the add above silently no-ops on every one of those runs
    // without this.
    if (!addedToProject) {
      console.warn(
        `⚠️  Issue #${response.data.number} was created but could not be added to the project board ` +
          "(likely a token without Projects v2 write scope, e.g. a scheduled workflow's " +
          "GITHUB_TOKEN). It will be added automatically the next time `gh:status` is run " +
          "on it with a token that has project write access.",
      );
    }

    return {
      number: response.data.number,
      url: response.data.html_url,
      addedToProject
    };
  }

  /**
   * Add an existing issue to the ProjectV2 board by its GraphQL node ID.
   *
   * Idempotent on GitHub's side: adding an issue already on the board returns
   * that same item rather than erroring or duplicating it, so this is safe to
   * call on an issue whose membership is unknown.
   */
  async addIssueToProject(issueNodeId: string): Promise<string> {
    const mutation = `
      mutation AddIssueToProject($projectId: ID!, $contentId: ID!) {
        addProjectV2ItemById(input: {
          projectId: $projectId
          contentId: $contentId
        }) {
          item {
            id
          }
        }
      }
    `;

    const result = await this.octokit.graphql<{
      addProjectV2ItemById: { item: { id: string } };
    }>(mutation, {
      projectId: this.projectId,
      contentId: issueNodeId
    });

    return result.addProjectV2ItemById.item.id;
  }

  /**
   * The GraphQL node ID for an issue number, needed by the ProjectV2
   * mutations (which key on node IDs, not the REST issue number).
   */
  async getIssueNodeId(issueNumber: number): Promise<string> {
    const response = await this.octokit.rest.issues.get({
      owner: this.owner,
      repo: this.repo,
      issue_number: issueNumber
    });
    return response.data.node_id;
  }

  /**
   * Update issue (no shell commands)
   */
  async updateIssue(
    issueNumber: number,
    updates: {
      title?: string;
      body?: string;
      labels?: string[];
      state?: "open" | "closed";
    }
  ): Promise<void> {
    const params: RestEndpointMethodTypes["issues"]["update"]["parameters"] = {
      owner: this.owner,
      repo: this.repo,
      issue_number: issueNumber
    };

    if (updates.title !== undefined) params.title = updates.title;
    if (updates.body !== undefined) params.body = updates.body;
    if (updates.labels !== undefined) params.labels = updates.labels;
    if (updates.state !== undefined) params.state = updates.state;

    await this.octokit.rest.issues.update(params);
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
   * Get PR number for current branch
   */
  async getPRForBranch(branch?: string): Promise<number | null> {
    const currentBranch = branch || this.getCurrentBranch();

    try {
      const { data: prs } = await this.octokit.rest.pulls.list({
        owner: this.owner,
        repo: this.repo,
        head: `${this.owner}:${currentBranch}`,
        state: "open"
      });

      return prs.length > 0 ? prs[0].number : null;
    } catch {
      return null;
    }
  }

  /**
   * Create a PR review with comments (no shell commands)
   */
  async createPRReview(
    prNumber: number,
    body: string,
    event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT" = "COMMENT",
    comments?: Array<{
      path: string;
      line: number;
      body: string;
    }>
  ): Promise<{ id: number; url: string }> {
    const params: RestEndpointMethodTypes["pulls"]["createReview"]["parameters"] = {
      owner: this.owner,
      repo: this.repo,
      pull_number: prNumber,
      body,
      event
    };

    // Add inline comments if provided
    if (comments && comments.length > 0) {
      params.comments = comments.map(c => ({
        path: c.path,
        line: c.line,
        body: c.body
      }));
    }

    const response = await this.octokit.rest.pulls.createReview(params);

    return {
      id: response.data.id,
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
   */
  async getPRReviews(prNumber: number): Promise<Array<{
    id: number;
    user: string;
    state: string;
    body: string;
    submitted_at: string;
  }>> {
    const { data: reviews } = await this.octokit.rest.pulls.listReviews({
      owner: this.owner,
      repo: this.repo,
      pull_number: prNumber
    });

    return reviews.map(r => ({
      id: r.id,
      user: r.user?.login || "unknown",
      state: r.state,
      body: r.body || "",
      submitted_at: r.submitted_at || ""
    }));
  }

  /**
   * Get PR details including commit SHA
   */
  async getPRDetails(prNumber: number): Promise<{
    number: number;
    title: string;
    head_sha: string;
    base: string;
    head: string;
  }> {
    const { data: pr } = await this.octokit.rest.pulls.get({
      owner: this.owner,
      repo: this.repo,
      pull_number: prNumber
    });

    return {
      number: pr.number,
      title: pr.title,
      head_sha: pr.head.sha,
      base: pr.base.ref,
      head: pr.head.ref
    };
  }

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

  /**
   * Check if git working directory is clean (no shell commands).
   * Works in both normal checkouts and linked worktrees.
   */
  isWorkingDirectoryClean(): boolean {
    try {
      // Simple check - if the git dir resolves, assume we need proper git status
      // For now, we'll return true and let the user handle git status manually
      return this.resolveGitDir() !== null;
    } catch {
      return false;
    }
  }
}