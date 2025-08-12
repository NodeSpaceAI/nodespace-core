#!/usr/bin/env bun

/**
 * GitHub Project Management Utilities
 * 
 * Usage:
 *   bun run scripts/gh-utils.ts issues:status 57,58,59 "In Progress"
 *   bun run scripts/gh-utils.ts issues:assign 60,61,62 "@me"
 *   bun run scripts/gh-utils.ts issues:list --status open
 */

import { $ } from "bun";

interface ProjectItem {
  id: string;
  content: {
    number: number;
    title: string;
  };
}

interface ProjectData {
  items: ProjectItem[];
}

class GitHubProjectManager {
  private readonly projectId = "5";
  private readonly owner = "malibio";
  private readonly projectFieldId = "PVTSSF_lAHOADHu9M4A_nxNzgyq13o"; // From CLAUDE.md
  private readonly fullProjectId = "PVT_kwHOADHu9M4A_nxN"; // From CLAUDE.md

  // Status option IDs (you'll need to get these from your project)
  private readonly statusOptions = {
    "Todo": "f75ad846",
    "In Progress": "47fc9ee4", 
    "Ready for Review": "98d433e7",
    "Done": "6e7da6f0"
  } as const;

  async getProjectItems(): Promise<ProjectData> {
    const result = await $`gh project item-list ${this.projectId} --owner ${this.owner} --limit 200 --format=json`.quiet();
    return JSON.parse(result.text());
  }

  async updateIssueStatus(issueNumbers: number[], status: keyof typeof this.statusOptions) {
    const data = await this.getProjectItems();
    const statusOptionId = this.statusOptions[status];

    if (!statusOptionId) {
      throw new Error(`Invalid status: ${status}. Valid options: ${Object.keys(this.statusOptions).join(", ")}`);
    }

    const results = [];
    
    for (const issueNumber of issueNumbers) {
      const item = data.items.find(item => item.content?.number === issueNumber);
      
      if (item) {
        try {
          await $`gh project item-edit --id ${item.id} --project-id ${this.fullProjectId} --field-id ${this.projectFieldId} --single-select-option-id ${statusOptionId}`.quiet();
          results.push({ issueNumber, success: true, itemId: item.id });
          console.log(`✅ Issue #${issueNumber} updated to '${status}'`);
        } catch (error) {
          results.push({ issueNumber, success: false, error: error.message });
          console.error(`❌ Failed to update issue #${issueNumber}: ${error.message}`);
        }
      } else {
        results.push({ issueNumber, success: false, error: "Issue not found in project" });
        console.error(`❌ Issue #${issueNumber} not found in project`);
      }
    }

    return results;
  }

  async assignIssues(issueNumbers: number[], assignee: string = "@me") {
    const results = [];
    
    for (const issueNumber of issueNumbers) {
      try {
        await $`gh issue edit ${issueNumber} --add-assignee ${assignee}`.quiet();
        results.push({ issueNumber, success: true });
        console.log(`✅ Issue #${issueNumber} assigned to ${assignee}`);
      } catch (error) {
        results.push({ issueNumber, success: false, error: error.message });
        console.error(`❌ Failed to assign issue #${issueNumber}: ${error.message}`);
      }
    }

    return results;
  }

  async listIssues(status?: string, label?: string) {
    const args = ["issue", "list", "--json", "number,title,state,assignees,labels"];
    
    if (status) {
      args.push("--state", status);
    }
    
    if (label) {
      args.push("--label", label);
    }

    const result = await $`gh ${args}`.quiet();
    const issues = JSON.parse(result.text());
    
    console.log("\n📋 Issues:");
    console.log("=" .repeat(60));
    
    for (const issue of issues) {
      const assigneeNames = issue.assignees.map(a => a.login).join(", ") || "Unassigned";
      const labelNames = issue.labels.map(l => l.name).join(", ") || "No labels";
      
      console.log(`#${issue.number}: ${issue.title}`);
      console.log(`   State: ${issue.state} | Assignees: ${assigneeNames}`);
      console.log(`   Labels: ${labelNames}\n`);
    }

    return issues;
  }
}

// CLI Interface
async function main() {
  const args = process.argv.slice(2);
  const command = args[0];
  const manager = new GitHubProjectManager();

  try {
    switch (command) {
      case "issues:status": {
        const issueNumbers = args[1]?.split(",").map(n => parseInt(n.trim()));
        const status = args[2] as keyof typeof manager["statusOptions"];
        
        if (!issueNumbers || !status) {
          console.error("Usage: bun run scripts/gh-utils.ts issues:status 57,58,59 \"In Progress\"");
          process.exit(1);
        }
        
        await manager.updateIssueStatus(issueNumbers, status);
        break;
      }
      
      case "issues:assign": {
        const issueNumbers = args[1]?.split(",").map(n => parseInt(n.trim()));
        const assignee = args[2] || "@me";
        
        if (!issueNumbers) {
          console.error("Usage: bun run scripts/gh-utils.ts issues:assign 60,61,62 \"@me\"");
          process.exit(1);
        }
        
        await manager.assignIssues(issueNumbers, assignee);
        break;
      }
      
      case "issues:list": {
        const statusIndex = args.indexOf("--status");
        const labelIndex = args.indexOf("--label");
        
        const status = statusIndex !== -1 ? args[statusIndex + 1] : undefined;
        const label = labelIndex !== -1 ? args[labelIndex + 1] : undefined;
        
        await manager.listIssues(status, label);
        break;
      }
      
      default:
        console.log(`
GitHub Project Manager

Usage:
  bun run scripts/gh-utils.ts issues:status 57,58,59 "In Progress"
  bun run scripts/gh-utils.ts issues:assign 60,61,62 "@me"  
  bun run scripts/gh-utils.ts issues:list --status open
  bun run scripts/gh-utils.ts issues:list --label foundation

Available statuses: Todo, In Progress, Ready for Review, Done
        `);
        break;
    }
  } catch (error) {
    console.error(`❌ Error: ${error.message}`);
    process.exit(1);
  }
}

if (import.meta.main) {
  main();
}

export { GitHubProjectManager };
