#!/usr/bin/env bun

/**
 * Test Command Validator
 *
 * Ensures tests are run with the correct command (bunx vitest) not (bun test)
 * This prevents the common issue where Happy-DOM doesn't load with bun test.
 *
 * Usage:
 *   Call this before running tests to validate the environment
 */

const CORRECT_TEST_COMMAND = 'bunx vitest run';
const INCORRECT_COMMAND = 'bun test';
const PKG_JSON_TEST_SCRIPT = 'bun run test';

function showWarning() {
  console.error(`
╔═══════════════════════════════════════════════════════════════╗
║                    ⚠️  INCORRECT TEST COMMAND ⚠️               ║
╟───────────────────────────────────────────────────────────────╢
║                                                               ║
║  DO NOT use: ${INCORRECT_COMMAND.padEnd(48)} ║
║                                                               ║
║  This command doesn't support Happy-DOM environment           ║
║  configuration and will cause tests to fail!                  ║
║                                                               ║
║  ✅ CORRECT COMMANDS:                                         ║
║     • ${CORRECT_TEST_COMMAND.padEnd(52)} ║
║     • ${PKG_JSON_TEST_SCRIPT.padEnd(52)} ║
║                                                               ║
║  Why? Vitest is configured with Happy-DOM in vitest.config.ts ║
║  Bun's native test runner doesn't read this configuration.    ║
║                                                               ║
╚═══════════════════════════════════════════════════════════════╝
`);
}

function validateTestEnvironment() {
  // Check if running via bun test directly
  const argv = process.argv;
  const isBunTest = argv.includes('test') && !argv.includes('vitest');

  if (isBunTest) {
    showWarning();
    console.error('\nℹ️  Run this instead:');
    console.error(`   cd ${process.cwd()}`);
    console.error(`   ${PKG_JSON_TEST_SCRIPT}\n`);
    process.exit(1);
  }

  // All good!
  return true;
}

// When run as script
if (import.meta.main) {
  validateTestEnvironment();
  console.log('✅ Test environment validated - using correct test runner');
}

export { validateTestEnvironment, showWarning };
