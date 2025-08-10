/// <reference types="vitest" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
  plugins: [sveltekit()],
  
  test: {
    include: ['src/tests/**/*.{test,spec}.{js,ts}'],
    environment: 'jsdom',
    globals: true,
    setupFiles: ['src/tests/setup.ts'],
    
    // Simple coverage configuration
    coverage: {
      reporter: ['text', 'html'],
      exclude: [
        'node_modules/',
        'src/tests/',
        '**/*.d.ts',
        '**/*.config.ts',
        'build/',
        'dist/'
      ],
      thresholds: {
        lines: 80,
        functions: 80,
        branches: 75,
        statements: 80
      }
    },
    
    // Keep tests fast and simple
    testTimeout: 5000,
    hookTimeout: 5000
  }
});