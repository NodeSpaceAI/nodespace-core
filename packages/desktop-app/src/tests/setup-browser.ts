/**
 * Browser test environment setup
 *
 * This file runs once before each browser test file and sets up the necessary
 * environment for testing in a real browser context.
 */

// Global setup for browser tests
console.log('Setting up browser test environment...');

// Optionally add custom matchers or utilities here
// For example, if you need to set up custom expect matchers:
// import { expect } from 'vitest';
// expect.extend({
//   toHaveFocus(element: HTMLElement) {
//     return {
//       pass: document.activeElement === element,
//       message: () => `Expected element to ${this.isNot ? 'not ' : ''}have focus`
//     };
//   }
// });
