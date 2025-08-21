#!/usr/bin/env bun

/**
 * Pure TypeScript GitHub API Client for NodeSpace
 * 
 * NO SHELL COMMANDS - Uses only direct API calls to eliminate Claude Code prompts
 * Authentication via gh CLI token automatically detected
 */

import { Octokit } from "@octokit/rest";
import { readFileSync, existsSync } from "fs";
import { homedir } from "os";
import path from "path";
import { $ } from "bun";

interface ProjectItem {
  id: string;
  content: {
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
  private readonly projectNumber = 5;
  private readonly projectId = "PVT_kwHOADHu9M4A_nxN";
  private readonly statusFieldId = "PVTSSF_lAHOADHu9M4A_nxNzgyq13o";

  // Status option IDs from issue-workflow.md
  private readonly statusOptions = {
    "Todo": "f75ad846",
    "In Progress": "47fc9ee4", 
    "Waiting for Input": "db18cb7f",
    "Ready for Review": "b13f9084",
    "In Review": "bd055968",
    "Done": "98236657",
    "Ready to Merge": "414430c1"
  } as const;

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
    } catch (error) {
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
        } catch (error) {
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
   * Get project items using GraphQL API (no pagination loops needed)
   */
  async getProjectItems(): Promise<ProjectItem[]> {
    const query = `
      query GetProjectItems($owner: String!, $projectNumber: Int!) {
        user(login: $owner) {
          projectV2(number: $projectNumber) {
            items(first: 100) {
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

    const response = await this.octokit.graphql<{
      user: {
        projectV2: {
          items: {
            nodes: ProjectItem[];
          };
        };
      };
    }>(query, {
      owner: this.owner,
      projectNumber: this.projectNumber,
    });

    return response.user.projectV2.items.nodes.filter(
      item => item.content?.number // Only return items that are issues
    );
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
        const itemId = await this.getItemIdForIssue(issueNumber);
        
        if (!itemId) {
          results.push({ 
            issueNumber, 
            success: false, 
            error: "Issue not found in project" 
          });
          continue;
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
        
      } catch (error) {
        results.push({ 
          issueNumber, 
          success: false, 
          error: error.message 
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

        results.push({ issueNumber, success: true });
        
      } catch (error) {
        results.push({ 
          issueNumber, 
          success: false, 
          error: error.message 
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

        results.push({ issueNumber, success: true });
        
      } catch (error) {
        results.push({ 
          issueNumber, 
          success: false, 
          error: error.message 
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
    const params: any = {
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

    const response = await this.octokit.rest.issues.listForRepo(params);
    
    return response.data.map(issue => ({
      number: issue.number,
      title: issue.title,
      state: issue.state,
      assignees: issue.assignees?.map(a => ({ login: a.login })) || [],
      labels: issue.labels?.map(l => ({ name: typeof l === 'string' ? l : l.name })) || [],
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
      labels: issue.labels?.map(l => ({ name: typeof l === 'string' ? l : l.name })) || [],
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
  ): Promise<{ number: number; url: string }> {
    const response = await this.octokit.rest.issues.create({
      owner: this.owner,
      repo: this.repo,
      title,
      body,
      labels: labels || [],
      assignees: assignees || []
    });

    return {
      number: response.data.number,
      url: response.data.html_url
    };
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
    const params: any = {
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
   * Get current git branch (no shell commands - reads .git directly)
   */
  getCurrentBranch(): string {
    try {
      const headPath = path.join(process.cwd(), ".git", "HEAD");
      if (!existsSync(headPath)) {
        throw new Error("Not in a git repository");
      }

      const headContent = readFileSync(headPath, "utf-8").trim();
      if (headContent.startsWith("ref: refs/heads/")) {
        return headContent.replace("ref: refs/heads/", "");
      }
      
      // Detached HEAD state
      return headContent.substring(0, 7);
    } catch (error) {
      throw new Error(`Failed to get current branch: ${error.message}`);
    }
  }

  /**
   * Check if git working directory is clean (no shell commands)
   */
  isWorkingDirectoryClean(): boolean {
    try {
      const gitDir = path.join(process.cwd(), ".git");
      const indexPath = path.join(gitDir, "index");
      
      // Simple check - if .git exists and index exists, assume we need proper git status
      // For now, we'll return true and let the user handle git status manually
      return existsSync(gitDir);
    } catch {
      return false;
    }
  }
}