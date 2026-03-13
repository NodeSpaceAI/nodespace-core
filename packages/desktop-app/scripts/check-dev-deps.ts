#!/usr/bin/env bun

/**
 * Development Dependencies Checker
 *
 * Validates that all required development dependencies are installed before
 * starting the development server.
 */

import { $ } from 'bun';

interface DependencyCheck {
  name: string;
  command: string;
  installUrl: string;
  required: boolean;
}

const dependencies: DependencyCheck[] = [
  {
    name: 'SurrealDB',
    command: 'surreal version',
    installUrl: 'https://surrealdb.com/install',
    required: true
  },
  {
    name: 'Bun',
    command: 'bun --version',
    installUrl: 'https://bun.sh/install',
    required: true
  }
];

async function checkDependency(dep: DependencyCheck): Promise<boolean> {
  try {
    const result = await $`sh -c ${dep.command}`.quiet();
    const output = result.stdout.toString().trim();

    // For SurrealDB, enforce minimum 3.x (crate requires 3.x on-disk format)
    if (dep.name === 'SurrealDB') {
      const match = output.match(/(\d+)\./);
      if (match && parseInt(match[1]) < 3) {
        console.error(`❌ ${dep.name} version too old: found ${output}, need 3.x+`);
        console.error(`   Upgrade: curl -sSf https://install.surrealdb.com | sh`);
        return false;
      }
    }

    console.log(`✅ ${dep.name} is installed`);
    return true;
  } catch {
    console.error(`❌ ${dep.name} is not installed or not in PATH`);
    console.error(`   Install from: ${dep.installUrl}`);
    return false;
  }
}

async function main() {
  console.log('🔍 Checking development dependencies...\n');

  const results = await Promise.all(dependencies.map((dep) => checkDependency(dep)));

  const allInstalled = results.every((result) => result);

  console.log();

  if (!allInstalled) {
    console.error('❌ Some required dependencies are missing.');
    console.error('   Please install them before running `bun run dev`\n');
    process.exit(1);
  }

  console.log('✅ All development dependencies are installed\n');
  process.exit(0);
}

main();
