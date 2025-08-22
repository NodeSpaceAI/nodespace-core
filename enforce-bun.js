#!/usr/bin/env bun

/**
 * Enforce Bun Usage Script
 * 
 * This script prevents the use of npm, yarn, or pnpm in favor of Bun.
 * It's called as a preinstall hook to block non-Bun package managers.
 * 
 * Updated to use Bun shebang instead of Node.js
 */

const packageManager = process.env.npm_execpath || '';
const isNpm = packageManager.includes('npm') && !packageManager.includes('bun');
const isYarn = packageManager.includes('yarn');
const isPnpm = packageManager.includes('pnpm');

if (isNpm || isYarn || isPnpm) {
  const detected = isNpm ? 'npm' : isYarn ? 'yarn' : 'pnpm';
  
  console.error('\n❌ BLOCKED: Package manager not allowed');
  console.error(`Detected: ${detected}`);
  console.error('\n🚀 This project uses Bun for better performance and compatibility.');
  console.error('\nPlease use Bun instead:');
  console.error('  📦 Install Bun: curl -fsSL https://bun.sh/install | bash');
  console.error('  🔧 Install packages: bun install');
  console.error('  🏃 Run dev server: bun run dev');
  console.error('\nFor more info: https://bun.sh\n');
  
  process.exit(1);
}

console.log('✅ Using Bun - package manager check passed');