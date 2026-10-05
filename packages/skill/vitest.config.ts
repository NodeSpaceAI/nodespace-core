import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    // The harness plugin's tests run inside Claude Code (`bun run test:plugin`),
    // not here.
    include: ['src/**/*.test.ts'],
  },
});
