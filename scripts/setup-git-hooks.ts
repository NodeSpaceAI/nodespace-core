#!/usr/bin/env bun
/**
 * Setup Git Hooks for NodeSpace
 *
 * Installs pre-commit hooks to enforce code quality standards.
 * Run: bun run scripts/setup-git-hooks.ts
 */

import { existsSync, mkdirSync, writeFileSync, chmodSync } from 'fs';
import { join } from 'path';

const HOOKS_DIR = join(process.cwd(), '.git', 'hooks');
const PRE_COMMIT_HOOK = join(HOOKS_DIR, 'pre-commit');

const preCommitScript = `#!/usr/bin/env bun
/**
 * Pre-commit hook to ensure code quality before commits
 *
 * Runs:
 * - bun run quality:fix (includes linting, formatting, and TypeScript checking)
 * - Prevents commits if quality checks fail
 */

console.log('\\n🔍 Running pre-commit quality checks...\\n');

const proc = Bun.spawn(['bun', 'run', 'quality:fix'], {
  cwd: process.cwd(),
  stdio: ['inherit', 'inherit', 'inherit'],
});

const exitCode = await proc.exited;

if (exitCode !== 0) {
  console.error('\\n❌ Quality checks failed! Please fix the issues above before committing.\\n');
  process.exit(1);
}

console.log('\\n✅ Quality checks passed! Proceeding with commit.\\n');
process.exit(0);
`;

function main() {
  console.log('🔧 Setting up Git hooks for NodeSpace...\n');

  // Check if .git directory exists
  if (!existsSync(join(process.cwd(), '.git'))) {
    console.error('❌ Error: Not a Git repository. Run this from the repository root.');
    process.exit(1);
  }

  // Create hooks directory if it doesn't exist
  if (!existsSync(HOOKS_DIR)) {
    mkdirSync(HOOKS_DIR, { recursive: true });
  }

  // Write pre-commit hook
  writeFileSync(PRE_COMMIT_HOOK, preCommitScript, { mode: 0o755 });
  chmodSync(PRE_COMMIT_HOOK, 0o755);

  console.log('✅ Pre-commit hook installed successfully!');
  console.log('\n📝 The hook will run `bun run quality:fix` before each commit.');
  console.log('   This ensures all code meets quality standards before being committed.\n');
  console.log('💡 To bypass the hook in emergencies, use: git commit --no-verify\n');
}

main();
