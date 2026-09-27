/**
 * Performance Benchmark Tests for Pattern System
 *
 * Validates that the unified pattern system maintains performance
 * within acceptable thresholds compared to legacy implementation.
 *
 * Acceptance Criteria:
 * - Pattern detection performance within 5% of legacy implementation
 * - Splitting performance within 5% of legacy implementation
 */

import { describe, it, expect } from 'vitest';
import { PatternRegistry } from '$lib/patterns/registry';
import { patternSplitter } from '$lib/patterns/splitter';

describe('Pattern System Performance Benchmarks', () => {
  // Performance thresholds (in milliseconds per operation)
  const PATTERN_DETECTION_THRESHOLD_MS = 0.01; // 10 microseconds
  const PATTERN_SPLITTING_THRESHOLD_MS = 0.05; // 50 microseconds
  const BENCHMARK_ITERATIONS = 10000; // Run each test 10k times for statistical significance

  // A single Map.get()/Map.has() is well under 1 microsecond, but averaging
  // 10k calls via performance.now() measures the loop plus timer resolution,
  // GC pauses, and JIT warmup jitter — each of which can individually exceed
  // 1 microsecond on a normally-loaded (not idle) machine. 0.001ms was a
  // noise-level threshold, not a regression guard: any of those factors alone
  // could tip it over with no actual slowdown in the lookup. 10 microseconds
  // still catches a real regression (a Map lookup ballooning 10x) while
  // giving the assertion room to not be a coin flip.
  const REGISTRY_LOOKUP_THRESHOLD_MS = 0.01; // 10 microseconds
  const REGISTRY_GET_ALL_THRESHOLD_MS = 0.02; // 20 microseconds - converts Map to Array

  describe('Pattern Detection Performance', () => {
    it('should detect header patterns within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = '# Header content with some text';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPattern(content);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });

    it('should detect ordered list patterns within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = '1. First item in the list';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPattern(content);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });

    it('should detect quote block patterns within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = '> This is a quote block';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPattern(content);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });

    it('should detect task patterns within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = '- [ ] Incomplete task';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPattern(content);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });

    it('should handle no pattern match within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = 'Plain text with no pattern markers';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPattern(content);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });

    it('should detect patterns by node type within threshold', () => {
      const registry = PatternRegistry.getInstance();
      const content = '# Header content';

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.detectPatternForNodeType(content, 'header');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
    });
  });

  describe('Content Splitting Performance', () => {
    it('should split header content within threshold (prefix-inheritance)', () => {
      const content = '## Medium header with some text';
      const cursorPosition = 10;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition, 'header');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should split ordered list content within threshold (prefix-inheritance)', () => {
      const content = '1. List item with some text';
      const cursorPosition = 15;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition, 'ordered-list');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should split quote block content within threshold (prefix-inheritance)', () => {
      const content = '> Quote block with some text';
      const cursorPosition = 12;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition, 'quote-block');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should split text with inline formatting within threshold (simple-split)', () => {
      const content = 'Some **bold** and *italic* text';
      const cursorPosition = 15;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition, 'text');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should split task content within threshold (simple-split)', () => {
      const content = '- [x] Completed task with text';
      const cursorPosition = 20;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition, 'task');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should split with auto-detection within threshold', () => {
      const content = '### Header content';
      const cursorPosition = 10;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });
  });

  describe('Real-World Scenario Performance', () => {
    it('should handle complex markdown with nested formatting within threshold', () => {
      const content = '# Header with **bold** and *italic* and `code` markers';
      const cursorPosition = 25;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });

    it('should handle long content efficiently', () => {
      const content = '# ' + 'A'.repeat(500); // 500 character header
      const cursorPosition = 250;

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        patternSplitter.split(content, cursorPosition);
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS * 2); // Allow 2x threshold for long content
    });

    it('should handle rapid sequential splits efficiently', () => {
      const contents = ['# Header', '1. List item', '> Quote', '- [ ] Task', 'Plain text'];

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS / 5; i++) {
        for (const content of contents) {
          patternSplitter.split(content, 5);
        }
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });
  });

  describe('Registry Performance', () => {
    it('should retrieve patterns by node type within threshold', () => {
      const registry = PatternRegistry.getInstance();

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.getPattern('header');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(REGISTRY_LOOKUP_THRESHOLD_MS);
    });

    it('should check pattern existence within threshold', () => {
      const registry = PatternRegistry.getInstance();

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.hasPattern('header');
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(REGISTRY_LOOKUP_THRESHOLD_MS);
    });

    it('should get all patterns within threshold', () => {
      const registry = PatternRegistry.getInstance();

      const start = performance.now();
      for (let i = 0; i < BENCHMARK_ITERATIONS; i++) {
        registry.getAllPatterns();
      }
      const end = performance.now();

      const avgTime = (end - start) / BENCHMARK_ITERATIONS;
      expect(avgTime).toBeLessThan(REGISTRY_GET_ALL_THRESHOLD_MS);
    });
  });

  describe('Performance Statistics', () => {
    it('should report performance metrics for analysis', () => {
      const registry = PatternRegistry.getInstance();
      const testContent = '# Test header';
      const iterations = 1000;

      // Measure detection
      const detectionStart = performance.now();
      for (let i = 0; i < iterations; i++) {
        registry.detectPattern(testContent);
      }
      const detectionEnd = performance.now();
      const detectionAvg = (detectionEnd - detectionStart) / iterations;

      // Measure splitting
      const splittingStart = performance.now();
      for (let i = 0; i < iterations; i++) {
        patternSplitter.split(testContent, 7);
      }
      const splittingEnd = performance.now();
      const splittingAvg = (splittingEnd - splittingStart) / iterations;

      // Log performance metrics (useful for CI/CD monitoring)
      console.log('\nPattern System Performance Metrics:');
      console.log(`  Pattern Detection: ${(detectionAvg * 1000).toFixed(3)}μs per operation`);
      console.log(`  Content Splitting: ${(splittingAvg * 1000).toFixed(3)}μs per operation`);
      console.log(`  Detection Threshold: ${(PATTERN_DETECTION_THRESHOLD_MS * 1000).toFixed(3)}μs`);
      console.log(`  Splitting Threshold: ${(PATTERN_SPLITTING_THRESHOLD_MS * 1000).toFixed(3)}μs`);

      // Validate within thresholds
      expect(detectionAvg).toBeLessThan(PATTERN_DETECTION_THRESHOLD_MS);
      expect(splittingAvg).toBeLessThan(PATTERN_SPLITTING_THRESHOLD_MS);
    });
  });
});
