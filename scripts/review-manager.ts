#!/usr/bin/env bun

/**
 * Code Review Manager - Delta-Aware Reviews with GitHub Integration
 *
 * Features:
 * - Track review state across multiple review cycles
 * - Delta-aware: Only review changes since last review
 * - GitHub integration: Post reviews and inline comments to PRs
 *
 * Usage:
 *   bun run scripts/review-manager.ts --help
 *   bun run scripts/review-manager.ts --mode full
 *   bun run scripts/review-manager.ts --mode delta
 *   bun run scripts/review-manager.ts --post-to-github
 */

import { GitHubClient } from "./github-client.ts";
import { existsSync, readFileSync, writeFileSync } from "fs";
import path from "path";
import { $ } from "bun";

interface ReviewState {
  reviews: ReviewRecord[];
  currentBranch: string | null;
  lastReviewedCommit: string | null;
}

interface ReviewRecord {
  timestamp: string;
  commit: string;
  branch: string;
  mode: "full" | "delta";
  filesReviewed: string[];
  prNumber?: number;
  reviewUrl?: string;
}

interface ReviewFinding {
  severity: "critical" | "important" | "suggestion";
  title: string;
  description: string;
  file?: string;
  line?: number;
}

interface ReviewReport {
  recommendation: "APPROVE" | "REQUEST_CHANGES" | "COMMENT";
  findings: ReviewFinding[];
  summary: string;
}

export class ReviewManager {
  private client: GitHubClient;
  private stateFilePath: string;
  private state: ReviewState;

  constructor() {
    this.client = new GitHubClient();
    this.stateFilePath = path.join(process.cwd(), ".git", ".review-state.json");
    this.state = this.loadState();
  }

  /**
   * Load review state from .git/.review-state.json
   */
  private loadState(): ReviewState {
    if (existsSync(this.stateFilePath)) {
      try {
        const content = readFileSync(this.stateFilePath, "utf-8");
        return JSON.parse(content);
      } catch (error) {
        console.warn("Failed to load review state, starting fresh");
      }
    }

    return {
      reviews: [],
      currentBranch: null,
      lastReviewedCommit: null
    };
  }

  /**
   * Save review state to .git/.review-state.json
   */
  private saveState(): void {
    writeFileSync(this.stateFilePath, JSON.stringify(this.state, null, 2));
  }

  /**
   * Get current git commit SHA
   */
  private async getCurrentCommit(): Promise<string> {
    const result = await $`git rev-parse HEAD`.text();
    return result.trim();
  }

  /**
   * Get files changed since last review
   */
  async getChangedFilesSinceLastReview(): Promise<string[]> {
    if (!this.state.lastReviewedCommit) {
      // No previous review - return all files in PR
      const result = await $`git diff --name-only origin/main...HEAD`.text();
      return result.trim().split("\n").filter(f => f.length > 0);
    }

    // Get files changed since last reviewed commit
    const result = await $`git diff --name-only ${this.state.lastReviewedCommit}..HEAD`.text();
    return result.trim().split("\n").filter(f => f.length > 0);
  }

  /**
   * Get commits since last review
   */
  async getCommitsSinceLastReview(): Promise<string[]> {
    if (!this.state.lastReviewedCommit) {
      // No previous review - return all commits in PR
      const result = await $`git log --oneline origin/main..HEAD`.text();
      return result.trim().split("\n").filter(c => c.length > 0);
    }

    // Get commits since last reviewed commit
    const result = await $`git log --oneline ${this.state.lastReviewedCommit}..HEAD`.text();
    return result.trim().split("\n").filter(c => c.length > 0);
  }

  /**
   * Get diff for review (full or delta)
   */
  async getDiffForReview(mode: "full" | "delta"): Promise<string> {
    if (mode === "full" || !this.state.lastReviewedCommit) {
      // Full review: all changes from main
      return await $`git diff origin/main...HEAD`.text();
    }

    // Delta review: only changes since last review
    return await $`git diff ${this.state.lastReviewedCommit}..HEAD`.text();
  }

  /**
   * Parse review report markdown to extract findings
   */
  parseReviewReport(markdown: string): ReviewReport {
    const findings: ReviewFinding[] = [];
    let recommendation: ReviewReport["recommendation"] = "COMMENT";

    // Extract recommendation
    if (markdown.includes("**APPROVE**") || markdown.includes("Recommendation**: APPROVE")) {
      recommendation = "APPROVE";
    } else if (markdown.includes("**REQUEST CHANGES**") || markdown.includes("Recommendation**: REQUEST CHANGES")) {
      recommendation = "REQUEST_CHANGES";
    }

    // Parse findings by severity
    const criticalMatches = markdown.matchAll(/🔴\s*Critical[:\s]+(.+?)(?=\n\n|🟡|🟢|$)/gs);
    for (const match of criticalMatches) {
      findings.push({
        severity: "critical",
        title: match[1].split("\n")[0].trim(),
        description: match[1].trim()
      });
    }

    const importantMatches = markdown.matchAll(/🟡\s*Important[:\s]+(.+?)(?=\n\n|🔴|🟢|$)/gs);
    for (const match of importantMatches) {
      findings.push({
        severity: "important",
        title: match[1].split("\n")[0].trim(),
        description: match[1].trim()
      });
    }

    const suggestionMatches = markdown.matchAll(/🟢\s*(?:Suggestion|Nice-to-have)[:\s]+(.+?)(?=\n\n|🔴|🟡|$)/gs);
    for (const match of suggestionMatches) {
      findings.push({
        severity: "suggestion",
        title: match[1].split("\n")[0].trim(),
        description: match[1].trim()
      });
    }

    // Extract line references from findings (e.g., "Line 14:", "lines 44-69")
    for (const finding of findings) {
      const lineMatch = finding.description.match(/(?:Line|line)s?\s+(\d+)/);
      if (lineMatch) {
        finding.line = parseInt(lineMatch[1]);
      }

      const fileMatch = finding.description.match(/(?:file|File):\s*([^\s\n]+)/);
      if (fileMatch) {
        finding.file = fileMatch[1];
      }
    }

    return {
      recommendation,
      findings,
      summary: markdown
    };
  }

  /**
   * Post review to GitHub PR
   */
  async postReviewToGitHub(
    prNumber: number,
    report: ReviewReport,
    commitSha: string
  ): Promise<{ id: number; url: string }> {
    console.log(`\n📤 Posting review to PR #${prNumber}...`);

    // Determine GitHub review event
    let event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT" = "COMMENT";
    if (report.recommendation === "APPROVE") {
      event = "APPROVE";
    } else if (report.recommendation === "REQUEST_CHANGES") {
      event = "REQUEST_CHANGES";
    }

    // Build inline comments for findings with file/line info
    const inlineComments: Array<{ path: string; line: number; body: string }> = [];

    for (const finding of report.findings) {
      if (finding.file && finding.line) {
        const severityEmoji = {
          critical: "🔴",
          important: "🟡",
          suggestion: "🟢"
        }[finding.severity];

        inlineComments.push({
          path: finding.file,
          line: finding.line,
          body: `${severityEmoji} **${finding.severity.toUpperCase()}**: ${finding.title}\n\n${finding.description}`
        });
      }
    }

    // Post review with inline comments
    const review = await this.client.createPRReview(
      prNumber,
      report.summary,
      event,
      inlineComments.length > 0 ? inlineComments : undefined
    );

    console.log(`✅ Review posted: ${review.url}`);
    console.log(`   Event: ${event}`);
    console.log(`   Findings: ${report.findings.length}`);
    console.log(`   Inline comments: ${inlineComments.length}`);

    return review;
  }

  /**
   * Record a completed review
   */
  async recordReview(
    mode: "full" | "delta",
    filesReviewed: string[],
    prNumber?: number,
    reviewUrl?: string
  ): Promise<void> {
    const currentBranch = this.client.getCurrentBranch();
    const currentCommit = await this.getCurrentCommit();

    const record: ReviewRecord = {
      timestamp: new Date().toISOString(),
      commit: currentCommit,
      branch: currentBranch,
      mode,
      filesReviewed,
      prNumber,
      reviewUrl
    };

    this.state.reviews.push(record);
    this.state.currentBranch = currentBranch;
    this.state.lastReviewedCommit = currentCommit;

    this.saveState();

    console.log(`\n📝 Review recorded:`);
    console.log(`   Mode: ${mode}`);
    console.log(`   Commit: ${currentCommit.substring(0, 7)}`);
    console.log(`   Files: ${filesReviewed.length}`);
    if (prNumber) console.log(`   PR: #${prNumber}`);
    if (reviewUrl) console.log(`   URL: ${reviewUrl}`);
  }

  /**
   * Get review history for current branch
   */
  getReviewHistory(): ReviewRecord[] {
    const currentBranch = this.client.getCurrentBranch();
    return this.state.reviews.filter(r => r.branch === currentBranch);
  }

  /**
   * Reset review state for current branch
   */
  resetReviewState(): void {
    const currentBranch = this.client.getCurrentBranch();
    this.state.reviews = this.state.reviews.filter(r => r.branch !== currentBranch);
    this.state.lastReviewedCommit = null;
    this.saveState();

    console.log(`\n🔄 Review state reset for branch: ${currentBranch}`);
  }

  /**
   * Get review status summary
   */
  async getReviewStatus(): Promise<{
    hasBeenReviewed: boolean;
    lastReview?: ReviewRecord;
    pendingCommits: number;
    pendingFiles: string[];
  }> {
    const history = this.getReviewHistory();
    const lastReview = history[history.length - 1];
    const pendingFiles = await this.getChangedFilesSinceLastReview();
    const pendingCommits = (await this.getCommitsSinceLastReview()).length;

    return {
      hasBeenReviewed: history.length > 0,
      lastReview,
      pendingCommits,
      pendingFiles
    };
  }
}

// CLI Interface
async function main() {
  const args = process.argv.slice(2);
  const manager = new ReviewManager();

  // Parse flags
  const mode = args.includes("--mode")
    ? args[args.indexOf("--mode") + 1] as "full" | "delta"
    : "full";

  const postToGitHub = args.includes("--post-to-github");
  const showStatus = args.includes("--status");
  const reset = args.includes("--reset");
  const help = args.includes("--help");

  if (help) {
    console.log(`
🔍 Code Review Manager - Delta-Aware Reviews with GitHub Integration

USAGE:
  bun run scripts/review-manager.ts [OPTIONS]

OPTIONS:
  --mode <full|delta>    Review mode (default: full)
                         full: Review all changes from main
                         delta: Only review changes since last review

  --post-to-github       Post review results to GitHub PR

  --status               Show review status for current branch

  --reset                Reset review state for current branch

  --help                 Show this help message

EXAMPLES:
  # Full review (first time or comprehensive check)
  bun run scripts/review-manager.ts --mode full

  # Delta review (only new changes since last review)
  bun run scripts/review-manager.ts --mode delta

  # Review and post to GitHub
  bun run scripts/review-manager.ts --mode delta --post-to-github

  # Check review status
  bun run scripts/review-manager.ts --status

  # Reset review state
  bun run scripts/review-manager.ts --reset
    `);
    return;
  }

  if (reset) {
    manager.resetReviewState();
    return;
  }

  if (showStatus) {
    const status = await manager.getReviewStatus();
    console.log(`\n📊 Review Status:`);
    console.log(`   Branch: ${manager["client"].getCurrentBranch()}`);
    console.log(`   Has been reviewed: ${status.hasBeenReviewed ? "Yes" : "No"}`);

    if (status.lastReview) {
      console.log(`   Last review: ${new Date(status.lastReview.timestamp).toLocaleString()}`);
      console.log(`   Last commit: ${status.lastReview.commit.substring(0, 7)}`);
      console.log(`   Mode: ${status.lastReview.mode}`);
    }

    console.log(`   Pending commits: ${status.pendingCommits}`);
    console.log(`   Pending files: ${status.pendingFiles.length}`);

    if (status.pendingFiles.length > 0) {
      console.log(`\n   Changed files since last review:`);
      status.pendingFiles.forEach(f => console.log(`     - ${f}`));
    }

    return;
  }

  // Get diff content based on mode
  console.log(`\n🔍 Running ${mode} review...`);

  const diff = await manager.getDiffForReview(mode);
  const files = await manager.getChangedFilesSinceLastReview();
  const commits = await manager.getCommitsSinceLastReview();

  console.log(`   Files to review: ${files.length}`);
  console.log(`   Commits since last review: ${commits.length}`);

  // This is where the actual review would happen
  // For now, just output the information
  console.log(`\n📋 Review scope:`);
  console.log(`   Mode: ${mode}`);
  console.log(`   Files: ${files.join(", ")}`);

  if (postToGitHub) {
    const prNumber = await manager["client"].getPRForBranch();
    if (prNumber) {
      console.log(`\n📤 Would post review to PR #${prNumber}`);
      console.log(`   (Review posting requires integration with review agent)`);
    } else {
      console.log(`\n⚠️  No open PR found for current branch`);
    }
  }
}

if (import.meta.main) {
  main();
}

export type { ReviewReport, ReviewFinding };
