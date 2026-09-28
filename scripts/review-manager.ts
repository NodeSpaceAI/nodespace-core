#!/usr/bin/env bun

/**
 * Code Review Manager - Delta-Aware Review State Tracking
 *
 * Features:
 * - Track review state across multiple review cycles
 * - Delta-aware: Only review changes since last review
 *
 * Usage:
 *   bun run scripts/review-manager.ts --help
 *   bun run scripts/review-manager.ts --mode full
 *   bun run scripts/review-manager.ts --mode delta
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
      } catch {
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

  const showStatus = args.includes("--status");
  const reset = args.includes("--reset");
  const help = args.includes("--help");

  if (help) {
    console.log(`
🔍 Code Review Manager - Delta-Aware Review State Tracking

USAGE:
  bun run scripts/review-manager.ts [OPTIONS]

OPTIONS:
  --mode <full|delta>    Review mode (default: full)
                         full: Review all changes from main
                         delta: Only review changes since last review

  --status               Show review status for current branch

  --reset                Reset review state for current branch

  --help                 Show this help message

EXAMPLES:
  # Full review (first time or comprehensive check)
  bun run scripts/review-manager.ts --mode full

  # Delta review (only new changes since last review)
  bun run scripts/review-manager.ts --mode delta

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

  const files = await manager.getChangedFilesSinceLastReview();
  const commits = await manager.getCommitsSinceLastReview();

  console.log(`   Files to review: ${files.length}`);
  console.log(`   Commits since last review: ${commits.length}`);

  // This is where the actual review would happen
  // For now, just output the information
  console.log(`\n📋 Review scope:`);
  console.log(`   Mode: ${mode}`);
  console.log(`   Files: ${files.join(", ")}`);
}

if (import.meta.main) {
  main();
}
