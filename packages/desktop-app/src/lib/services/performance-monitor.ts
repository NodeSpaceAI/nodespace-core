/**
 * PerformanceMonitor - Comprehensive Performance Monitoring and Benchmarking
 *
 * Advanced performance monitoring system for the Universal Node Reference System
 * with automated benchmarking, regression detection, and memory usage tracking.
 *
 * Performance Targets:
 * - @ trigger detection: <10ms
 * - Autocomplete response: <50ms
 * - Node decoration rendering: <16ms (60fps)
 * - 500+ references handling efficiently
 * - Memory leak prevention in long editing sessions
 */

export interface PerformanceMetrics {
  // Core operation metrics
  triggerDetectionTime: number;
  autocompleteResponseTime: number;
  decorationRenderTime: number;
  uriResolutionTime: number;

  // Throughput metrics
  operationsPerSecond: number;
  cacheHitRatio: number;
  memoryUsage: number;

  // Quality metrics
  successRate: number;
  errorRate: number;

  // Session metrics
  sessionDuration: number;
  totalOperations: number;

  timestamp: number;
}

export interface PerformanceBenchmark {
  operation: string;
  target: number;
  actual: number;
  passed: boolean;
  percentile95: number;
  percentile99: number;
  samples: number;
}

export interface MemorySnapshot {
  heapUsed: number;
  heapTotal: number;
  external: number;
  arrayBuffers: number;
  timestamp: number;
}

export class PerformanceMonitor {
  private static instance: PerformanceMonitor | null = null;

  // Performance data collection
  private metrics = new Map<string, number[]>();
  private memorySnapshots: MemorySnapshot[] = [];
  private benchmarkTargets = new Map<string, number>();
  private sessionStartTime = performance.now();

  // Memory leak detection
  private memoryLeakThreshold = 50 * 1024 * 1024; // 50MB
  private memoryCheckInterval = 30000; // 30 seconds
  private memoryTimer: number | null = null;

  // Performance regression detection
  private baselineMetrics = new Map<string, PerformanceBenchmark>();
  private regressionThreshold = 0.2; // 20% performance degradation threshold

  private constructor() {
    this.setupBenchmarkTargets();
    this.startMemoryMonitoring();
  }

  public static getInstance(): PerformanceMonitor {
    if (!PerformanceMonitor.instance) {
      PerformanceMonitor.instance = new PerformanceMonitor();
    }
    return PerformanceMonitor.instance;
  }

  // ============================================================================
  // Core Performance Measurement
  // ============================================================================

  /**
   * Start measuring a performance operation
   */
  public startMeasurement(operation: string): { finish: () => number } {
    const startTime = performance.now();

    return {
      finish: () => {
        const duration = performance.now() - startTime;
        this.recordMetric(operation, duration);
        return duration;
      }
    };
  }

  /**
   * Record a performance metric
   */
  public recordMetric(operation: string, duration: number): void {
    if (!this.metrics.has(operation)) {
      this.metrics.set(operation, []);
    }

    const values = this.metrics.get(operation)!;
    values.push(duration);

    // Keep only last 1000 measurements for memory efficiency
    if (values.length > 1000) {
      values.splice(0, values.length - 1000);
    }

    // Check for performance regressions
    this.checkPerformanceRegression(operation, duration);
  }

  /**
   * Get performance statistics for an operation
   */
  public getOperationStats(operation: string): {
    avg: number;
    min: number;
    max: number;
    p95: number;
    p99: number;
    samples: number;
  } | null {
    const values = this.metrics.get(operation);
    if (!values || values.length === 0) {
      return null;
    }

    const sorted = [...values].sort((a, b) => a - b);
    const len = sorted.length;

    return {
      avg: values.reduce((sum, val) => sum + val, 0) / len,
      min: sorted[0],
      max: sorted[len - 1],
      p95: sorted[Math.floor(len * 0.95)],
      p99: sorted[Math.floor(len * 0.99)],
      samples: len
    };
  }

  // ============================================================================
  // Benchmarking System
  // ============================================================================

  private setupBenchmarkTargets(): void {
    this.benchmarkTargets.set('trigger-detection', 10); // <10ms
    this.benchmarkTargets.set('autocomplete-response', 50); // <50ms
    this.benchmarkTargets.set('decoration-render', 16); // <16ms (60fps)
    this.benchmarkTargets.set('uri-resolution', 5); // <5ms
    this.benchmarkTargets.set('cache-lookup', 1); // <1ms
    this.benchmarkTargets.set('search-nodes', 100); // <100ms
  }

  /**
   * Run comprehensive benchmarks (async)
   */
  public async runBenchmarks(): Promise<PerformanceBenchmark[]> {
    return this.runBenchmarksSync();
  }

  /**
   * Run comprehensive benchmarks synchronously
   */
  private runBenchmarksSync(): PerformanceBenchmark[] {
    const results: PerformanceBenchmark[] = [];

    for (const [operation, target] of Array.from(this.benchmarkTargets.entries())) {
      const stats = this.getOperationStats(operation);

      if (stats) {
        const benchmark: PerformanceBenchmark = {
          operation,
          target,
          actual: stats.avg,
          passed: stats.avg <= target,
          percentile95: stats.p95,
          percentile99: stats.p99,
          samples: stats.samples
        };

        results.push(benchmark);
        this.baselineMetrics.set(operation, benchmark);
      }
    }

    return results;
  }

  /**
   * Check if current performance meets benchmarks
   */
  public validatePerformanceTargets(): { passed: boolean; failures: string[] } {
    const failures: string[] = [];

    for (const [operation, target] of Array.from(this.benchmarkTargets.entries())) {
      const stats = this.getOperationStats(operation);

      if (stats && stats.avg > target) {
        failures.push(`${operation}: ${stats.avg.toFixed(2)}ms > ${target}ms target`);
      }
    }

    return {
      passed: failures.length === 0,
      failures
    };
  }

  // ============================================================================
  // Memory Monitoring and Leak Detection
  // ============================================================================

  private startMemoryMonitoring(): void {
    if (typeof window !== 'undefined') {
      this.memoryTimer = window.setInterval(() => {
        this.captureMemorySnapshot();
      }, this.memoryCheckInterval);
    } else if (typeof setInterval !== 'undefined') {
      // Node.js environment
      this.memoryTimer = setInterval(() => {
        this.captureMemorySnapshot();
      }, this.memoryCheckInterval) as unknown as number;
    }
  }

  private captureMemorySnapshot(): void {
    try {
      // Node.js environment
      if (typeof process !== 'undefined' && process?.memoryUsage) {
        const mem = process.memoryUsage();
        const snapshot: MemorySnapshot = {
          heapUsed: mem.heapUsed,
          heapTotal: mem.heapTotal,
          external: mem.external,
          arrayBuffers: mem.arrayBuffers,
          timestamp: Date.now()
        };

        this.memorySnapshots.push(snapshot);
        this.checkMemoryLeaks(snapshot);
      }
      // Browser environment - approximate using performance API
      else if (
        typeof performance !== 'undefined' &&
        'memory' in performance &&
        (
          performance as Performance & {
            memory?: { usedJSHeapSize: number; totalJSHeapSize: number; jsHeapSizeLimit: number };
          }
        ).memory
      ) {
        const perfMemory = (
          performance as Performance & {
            memory: { usedJSHeapSize: number; totalJSHeapSize: number; jsHeapSizeLimit: number };
          }
        ).memory;
        const mem = perfMemory;
        const snapshot: MemorySnapshot = {
          heapUsed: mem.usedJSHeapSize,
          heapTotal: mem.totalJSHeapSize,
          external: 0,
          arrayBuffers: 0,
          timestamp: Date.now()
        };

        this.memorySnapshots.push(snapshot);
        this.checkMemoryLeaks(snapshot);
      }

      // Keep only last 100 snapshots
      if (this.memorySnapshots.length > 100) {
        this.memorySnapshots.splice(0, this.memorySnapshots.length - 100);
      }
    } catch (error) {
      console.warn('PerformanceMonitor: Unable to capture memory snapshot', error);
    }
  }

  private checkMemoryLeaks(snapshot: MemorySnapshot): void {
    if (this.memorySnapshots.length < 10) {
      return; // Need at least 10 snapshots to detect trends
    }

    const recent = this.memorySnapshots.slice(-10);
    const growthTrend = recent.every((s, i) => i === 0 || s.heapUsed >= recent[i - 1].heapUsed);

    if (growthTrend && snapshot.heapUsed > this.memoryLeakThreshold) {
      console.warn('PerformanceMonitor: Potential memory leak detected', {
        currentUsage: (snapshot.heapUsed / 1024 / 1024).toFixed(2) + 'MB',
        threshold: (this.memoryLeakThreshold / 1024 / 1024).toFixed(2) + 'MB',
        snapshots: recent.length
      });
    }
  }

  public getMemoryStats(): {
    current: MemorySnapshot | null;
    trend: 'stable' | 'growing' | 'declining';
    averageUsage: number;
    peakUsage: number;
  } {
    if (this.memorySnapshots.length === 0) {
      return {
        current: null,
        trend: 'stable',
        averageUsage: 0,
        peakUsage: 0
      };
    }

    const current = this.memorySnapshots[this.memorySnapshots.length - 1];
    const peak = Math.max(...this.memorySnapshots.map((s) => s.heapUsed));
    const average =
      this.memorySnapshots.reduce((sum, s) => sum + s.heapUsed, 0) / this.memorySnapshots.length;

    // Calculate trend from last 5 snapshots
    let trend: 'stable' | 'growing' | 'declining' = 'stable';
    if (this.memorySnapshots.length >= 5) {
      const recent = this.memorySnapshots.slice(-5);
      const first = recent[0].heapUsed;
      const last = recent[recent.length - 1].heapUsed;
      const change = (last - first) / first;

      if (change > 0.1) trend = 'growing';
      else if (change < -0.1) trend = 'declining';
    }

    return {
      current,
      trend,
      averageUsage: average,
      peakUsage: peak
    };
  }

  // ============================================================================
  // Performance Regression Detection
  // ============================================================================

  private checkPerformanceRegression(operation: string, currentValue: number): void {
    const baseline = this.baselineMetrics.get(operation);
    if (!baseline) {
      return;
    }

    const regressionRatio = (currentValue - baseline.actual) / baseline.actual;

    if (regressionRatio > this.regressionThreshold) {
      console.warn('PerformanceMonitor: Performance regression detected', {
        operation,
        baseline: baseline.actual.toFixed(2) + 'ms',
        current: currentValue.toFixed(2) + 'ms',
        regression: (regressionRatio * 100).toFixed(1) + '%'
      });
    }
  }

  // ============================================================================
  // Comprehensive Metrics
  // ============================================================================

  /**
   * Get comprehensive performance metrics
   */
  public getComprehensiveMetrics(): PerformanceMetrics {
    const triggerStats = this.getOperationStats('trigger-detection');
    const autocompleteStats = this.getOperationStats('autocomplete-response');
    const decorationStats = this.getOperationStats('decoration-render');
    const uriStats = this.getOperationStats('uri-resolution');

    const sessionDuration = performance.now() - this.sessionStartTime;
    const totalOperations = Array.from(this.metrics.values()).reduce(
      (sum, values) => sum + values.length,
      0
    );

    const memoryStats = this.getMemoryStats();

    return {
      triggerDetectionTime: triggerStats?.avg || 0,
      autocompleteResponseTime: autocompleteStats?.avg || 0,
      decorationRenderTime: decorationStats?.avg || 0,
      uriResolutionTime: uriStats?.avg || 0,
      operationsPerSecond: totalOperations / (sessionDuration / 1000),
      cacheHitRatio: this.calculateCacheHitRatio(),
      memoryUsage: memoryStats.current?.heapUsed || 0,
      successRate: this.calculateSuccessRate(),
      errorRate: this.calculateErrorRate(),
      sessionDuration,
      totalOperations,
      timestamp: Date.now()
    };
  }

  private calculateCacheHitRatio(): number {
    const cacheHits = this.metrics.get('cache-hit')?.length || 0;
    const cacheMisses = this.metrics.get('cache-miss')?.length || 0;
    const total = cacheHits + cacheMisses;

    return total > 0 ? cacheHits / total : 0;
  }

  private calculateSuccessRate(): number {
    const successes = this.metrics.get('operation-success')?.length || 0;
    const failures = this.metrics.get('operation-failure')?.length || 0;
    const total = successes + failures;

    return total > 0 ? successes / total : 1;
  }

  private calculateErrorRate(): number {
    return 1 - this.calculateSuccessRate();
  }

  // ============================================================================
  // Report Generation
  // ============================================================================

  /**
   * Generate performance report
   */
  public generatePerformanceReport(): {
    summary: PerformanceMetrics;
    benchmarks: PerformanceBenchmark[];
    memoryAnalysis: {
      current: MemorySnapshot | null;
      trend: 'stable' | 'growing' | 'declining';
      averageUsage: number;
      peakUsage: number;
    };
    recommendations: string[];
  } {
    const summary = this.getComprehensiveMetrics();
    const benchmarks = this.runBenchmarksSync(); // Use sync version for reports
    const memoryAnalysis = this.getMemoryStats();
    const recommendations = this.generateRecommendations(summary, benchmarks, memoryAnalysis);

    return {
      summary,
      benchmarks,
      memoryAnalysis,
      recommendations
    };
  }

  private generateRecommendations(
    summary: PerformanceMetrics,
    benchmarks: PerformanceBenchmark[],
    memory: {
      current: MemorySnapshot | null;
      trend: 'stable' | 'growing' | 'declining';
      averageUsage: number;
      peakUsage: number;
    }
  ): string[] {
    const recommendations: string[] = [];

    // Performance recommendations
    const failedBenchmarks = benchmarks.filter((b) => !b.passed);
    if (failedBenchmarks.length > 0) {
      recommendations.push(
        `Performance optimization needed for: ${failedBenchmarks.map((b) => b.operation).join(', ')}`
      );
    }

    // Memory recommendations
    if (memory.trend === 'growing') {
      recommendations.push('Memory usage is trending upward - check for potential leaks');
    }

    if (memory.current && memory.current.heapUsed > 100 * 1024 * 1024) {
      recommendations.push('High memory usage detected - consider cache optimization');
    }

    // Cache recommendations
    if (summary.cacheHitRatio < 0.8) {
      recommendations.push('Cache hit ratio is low - review caching strategy');
    }

    // Error rate recommendations
    if (summary.errorRate > 0.05) {
      recommendations.push('High error rate detected - review error handling and validation');
    }

    if (recommendations.length === 0) {
      recommendations.push('All performance metrics are within acceptable ranges');
    }

    return recommendations;
  }

  // ============================================================================
  // Cleanup
  // ============================================================================

  public cleanup(): void {
    if (this.memoryTimer) {
      if (typeof window !== 'undefined') {
        window.clearInterval(this.memoryTimer);
      } else if (typeof clearInterval !== 'undefined') {
        clearInterval(this.memoryTimer);
      }
      this.memoryTimer = null;
    }

    this.metrics.clear();
    this.memorySnapshots.length = 0;
    this.baselineMetrics.clear();
  }
}

export default PerformanceMonitor;
