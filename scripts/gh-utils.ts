#!/usr/bin/env bun

/**
 * GitHub Project Management Utilities for NodeSpace
 * 
 * All status IDs and commands extracted from /docs/architecture/development/process/
 * 
 * Usage:
 *   bun run gh:status 57,58,59 "In Progress"
 *   bun run gh:assign 60,61,62 "@me"
 *   bun run gh:startup 45 "Brief description"
 *   bun run gh:list --status open
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

class NodeSpaceGitHubManager {
  // Project configuration from docs/architecture/development/process/issue-workflow.md
  private readonly projectId = "5";
  private readonly owner = "malibio";
  private readonly fullProjectId = "PVT_kwHOADHu9M4A_nxN";
  private readonly statusFieldId = "PVTSSF_lAHOADHu9M4A_nxNzgyq13o";

  // Actual status option IDs from issue-workflow.md
  private readonly statusOptions = {
    "Todo": "f75ad846",
    "In Progress": "47fc9ee4", 
    "Waiting for Input": "db18cb7f",
    "Ready for Review": "b13f9084",
    "In Review": "bd055968",
    "Done": "98236657",
    "Ready to Merge": "414430c1"
  } as const;

  async getProjectItems(): Promise<ProjectData> {
    const result = await $`gh project item-list ${this.projectId} --owner ${this.owner} --limit 200 --format=json`.quiet();
    return JSON.parse(result.text());
  }

  async getItemIdForIssue(issueNumber: number): Promise<string | null> {
    const data = await this.getProjectItems();
    const item = data.items.find(item => item.content?.number === issueNumber);
    return item?.id || null;
  }

  async updateIssueStatus(issueNumbers: number[], status: keyof typeof this.statusOptions) {
    const statusOptionId = this.statusOptions[status];

    if (!statusOptionId) {
      throw new Error(`Invalid status: ${status}. Valid options: ${Object.keys(this.statusOptions).join(", ")}`);
    }

    const results = [];
    
    for (const issueNumber of issueNumbers) {
      const itemId = await this.getItemIdForIssue(issueNumber);
      
      if (itemId) {
        try {
          await $`gh project item-edit --id ${itemId} --project-id ${this.fullProjectId} --field-id ${this.statusFieldId} --single-select-option-id ${statusOptionId}`.quiet();
          results.push({ issueNumber, success: true, itemId, status });
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

  async listIssues(options: { status?: string, label?: string, assignee?: string } = {}) {
    const args = ["issue", "list", "--json", "number,title,state,assignees,labels"];
    
    if (options.status) {
      args.push("--state", options.status);
    }
    
    if (options.label) {
      args.push("--label", options.label);
    }

    if (options.assignee) {
      args.push("--assignee", options.assignee);
    }

    const result = await $`gh ${args}`.quiet();
    const issues = JSON.parse(result.text());
    
    console.log(`\\n📋 Issues (${issues.length} found):`);
    console.log("=" .repeat(80));
    
    for (const issue of issues) {
      const assigneeNames = issue.assignees.map(a => a.login).join(", ") || "Unassigned";
      const labelNames = issue.labels.map(l => l.name).join(", ") || "No labels";
      
      console.log(`#${issue.number}: ${issue.title}`);
      console.log(`   State: ${issue.state} | Assignees: ${assigneeNames}`);
      console.log(`   Labels: ${labelNames}\\n`);
    }

    return issues;
  }

  async viewIssue(issueNumber: number, web: boolean = false) {
    try {
      const args = ["issue", "view", issueNumber.toString()];
      if (web) {
        args.push("--web");
        console.log(`🌐 Opening issue #${issueNumber} in web browser...`);
      }
      
      const result = await $`gh ${args}`;
      if (!web) {
        console.log(result.text());
      }
    } catch (error) {
      console.error(`❌ Failed to view issue #${issueNumber}: ${error.message}`);
    }
  }

  async createPR(title: string, body: string, draft: boolean = false) {
    try {
      const args = ["pr", "create", "--title", title, "--body", body];
      if (draft) {
        args.push("--draft");
      }
      
      const result = await $`gh ${args}`;
      console.log("✅ Pull request created:");
      console.log(result.text());
      return result.text();
    } catch (error) {
      console.error(`❌ Failed to create PR: ${error.message}`);
      throw error;
    }
  }

  // Complete startup sequence for an issue
  async startupSequence(issueNumber: number, branchDescription?: string) {
    console.log(`🚀 Starting startup sequence for issue #${issueNumber}...\\n`);

    try {
      // 1. Check git status
      console.log("1️⃣ Checking git status...");
      const gitStatus = await $`git status --porcelain`;
      if (gitStatus.text().trim()) {
        console.log("⚠️  You have uncommitted changes. Please commit them first:");
        const fullStatus = await $`git status`;
        console.log(fullStatus.text());
        return false;
      }
      console.log("✅ Git status clean\\n");

      // 2. Get issue details first
      console.log("2️⃣ Getting issue details...");
      const issueResult = await $`gh issue view ${issueNumber} --json title,body,labels`;
      const issue = JSON.parse(issueResult.text());
      console.log(`📄 Issue: ${issue.title}\\n`);

      // 3. Create branch
      const branchName = branchDescription 
        ? `feature/issue-${issueNumber}-${branchDescription.toLowerCase().replace(/\\s+/g, '-')}`
        : `feature/issue-${issueNumber}`;
      
      console.log(`3️⃣ Creating branch: ${branchName}`);
      await $`git checkout -b ${branchName}`;
      console.log("✅ Branch created\\n");

      // 4. Assign issue
      console.log("4️⃣ Assigning issue to self...");
      await this.assignIssues([issueNumber], "@me");
      console.log("✅ Issue assigned\\n");

      // 5. Update project status to In Progress
      console.log("5️⃣ Updating project status to 'In Progress'...");
      await this.updateIssueStatus([issueNumber], "In Progress");
      console.log("✅ Project status updated\\n");

      console.log(`🎉 Startup sequence completed for issue #${issueNumber}!`);
      console.log(`📋 Next steps:`);
      console.log(`   - Read the issue requirements carefully`);
      console.log(`   - Plan your self-contained implementation`);
      console.log(`   - Use appropriate specialized agents via Task tool`);
      console.log(`   - Begin implementation\\n`);

      return true;

    } catch (error) {
      console.error(`❌ Startup sequence failed: ${error.message}`);
      return false;
    }
  }

  // Quality gate check before PR creation
  async qualityCheck() {
    console.log("🔍 Running quality checks...\\n");

    try {
      console.log("Checking linting and formatting...");
      // Use the existing quality command from nodespace-app
      const qualityResult = await $`bun run --cwd nodespace-app quality:fix`;
      console.log(qualityResult.text());
      
      console.log("✅ Quality checks passed!");
      return true;
    } catch (error) {
      console.error("❌ Quality checks failed:");
      console.error(error.message);
      console.log("\\n🚨 Fix these issues before creating a PR");
      return false;
    }
  }

  // Complete PR workflow
  async createPRWorkflow(issueNumber: number, title?: string) {
    console.log(`📝 Creating PR for issue #${issueNumber}...\\n`);

    // Run quality checks first
    const qualityPassed = await this.qualityCheck();
    if (!qualityPassed) {
      console.log("❌ Cannot create PR until quality checks pass");
      return false;
    }

    try {
      // Get issue details for PR
      const issueResult = await $`gh issue view ${issueNumber} --json title`;
      const issue = JSON.parse(issueResult.text());
      
      const prTitle = title || issue.title;
      const prBody = `Closes #${issueNumber}`;

      // Create the PR
      await this.createPR(prTitle, prBody);

      // Update project status to Ready for Review
      console.log("\\n📊 Updating project status to 'Ready for Review'...");
      await this.updateIssueStatus([issueNumber], "Ready for Review");

      console.log("\\n🎉 PR workflow completed successfully!");
      return true;

    } catch (error) {
      console.error(`❌ PR workflow failed: ${error.message}`);
      return false;
    }
  }
}

// CLI Interface
async function main() {
  const args = process.argv.slice(2);
  const command = args[0];
  const manager = new NodeSpaceGitHubManager();

  try {
    switch (command) {
      case "issues:status": {
        const issueNumbers = args[1]?.split(",").map(n => parseInt(n.trim()));
        const status = args[2] as keyof typeof manager["statusOptions"];
        
        if (!issueNumbers || !status) {
          console.error("Usage: bun run gh:status 57,58,59 \\"In Progress\\"");
          process.exit(1);
        }
        
        await manager.updateIssueStatus(issueNumbers, status);
        break;
      }
      
      case "issues:assign": {
        const issueNumbers = args[1]?.split(",").map(n => parseInt(n.trim()));
        const assignee = args[2] || "@me";
        
        if (!issueNumbers) {
          console.error("Usage: bun run gh:assign 60,61,62 \\"@me\\"");
          process.exit(1);
        }
        
        await manager.assignIssues(issueNumbers, assignee);
        break;
      }
      
      case "issues:list": {
        const options: any = {};
        
        const statusIndex = args.indexOf("--status");
        const labelIndex = args.indexOf("--label");
        const assigneeIndex = args.indexOf("--assignee");
        
        if (statusIndex !== -1) options.status = args[statusIndex + 1];
        if (labelIndex !== -1) options.label = args[labelIndex + 1];
        if (assigneeIndex !== -1) options.assignee = args[assigneeIndex + 1];
        
        await manager.listIssues(options);
        break;
      }

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

      case "issues:startup": {
        const issueNumber = parseInt(args[1]);
        const branchDescription = args[2];
        
        if (!issueNumber) {
          console.error("Usage: bun run gh:startup 45 \\"Brief description\\"");
          process.exit(1);
        }
        
        await manager.startupSequence(issueNumber, branchDescription);
        break;
      }

      case "pr:create": {
        const issueNumber = parseInt(args[1]);
        const title = args[2];
        
        if (!issueNumber) {
          console.error("Usage: bun run scripts/gh-utils.ts pr:create 45 \\"Optional title\\"");
          process.exit(1);
        }
        
        await manager.createPRWorkflow(issueNumber, title);
        break;
      }

      case "quality:check": {
        await manager.qualityCheck();
        break;
      }
      
      default:
        console.log(`
🚀 NodeSpace GitHub Project Manager

📋 Issue Management:
  bun run gh:status 57,58,59 "In Progress"     # Update status
  bun run gh:assign 60,61,62 "@me"             # Assign issues  
  bun run gh:list --status open                # List issues
  bun run gh:list --label foundation           # Filter by label
  bun run gh:list --assignee "@me"             # Your issues
  
🎯 Issue Workflow:
  bun run gh:startup 45 "Brief description"    # Complete startup sequence
  bun run scripts/gh-utils.ts issues:view 45   # View issue details
  bun run scripts/gh-utils.ts issues:view 45 --web  # Open in browser

📝 PR Management:
  bun run scripts/gh-utils.ts pr:create 45     # Create PR workflow
  bun run scripts/gh-utils.ts quality:check    # Run quality gates

📊 Available Statuses:
  ${Object.keys(manager["statusOptions"]).map(s => `"${s}"`).join(", ")}

📖 Based on docs/architecture/development/process/ documentation
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

export { NodeSpaceGitHubManager };
