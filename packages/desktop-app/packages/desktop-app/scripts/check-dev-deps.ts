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
		const { stdout } = await execAsync('surreal version');
		console.log('✅ SurrealDB installed:', stdout.split('\n')[0]);
		return true;
	} catch {
		console.error('❌ SurrealDB not found');
		console.error('\nInstallation instructions:');
		console.error('  macOS:   brew install surrealdb/tap/surreal');
		console.error('  Linux:   curl -sSf https://install.surrealdb.com | sh');
		console.error('  Windows: iwr https://install.surrealdb.com -useb | iex');
		console.error('\nDocs: https://surrealdb.com/docs/installation');
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
