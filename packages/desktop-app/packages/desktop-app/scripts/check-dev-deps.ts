#!/usr/bin/env bun

/**
 * Verify development dependencies are installed
 * Checks that SurrealDB CLI is available before starting dev server
 */

import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

async function checkSurrealDB(): Promise<boolean> {
  try {
    // Try common installation locations
    const surrealPaths = [
      `${process.env.HOME}/.surrealdb/surreal`,
      '/usr/local/bin/surreal',
      'surreal' // In PATH
    ];

    for (const surrealPath of surrealPaths) {
      try {
        const { stdout } = await execAsync(`${surrealPath} version`);
        console.log('✅ SurrealDB installed:', stdout.split('\n')[0]);
        console.log(`   Location: ${surrealPath}`);
        return true;
      } catch {
        continue;
      }
    }

    throw new Error('SurrealDB not found in any location');
  } catch {
    console.error('❌ SurrealDB not found');
    console.error('\nInstallation instructions:');
    console.error('  macOS/Linux: curl -sSf https://install.surrealdb.com | sh');
    console.error('  Windows:     iwr https://install.surrealdb.com -useb | iex');
    console.error('\nDocs: https://surrealdb.com/docs/installation');
    console.error('\nNote: After installation, you may need to:');
    console.error('  • Restart your terminal');
    console.error('  • Unix: Add to PATH with: export PATH="$HOME/.surrealdb:$PATH"');
    console.error('  • Windows: The installer should add to PATH automatically');
    return false;
  }
}

async function main() {
  console.log('🔍 Checking development dependencies...\n');

  const surrealOk = await checkSurrealDB();

  if (!surrealOk) {
    process.exit(1);
  }

  console.log('\n✅ All dev dependencies installed!\n');
}

main();
