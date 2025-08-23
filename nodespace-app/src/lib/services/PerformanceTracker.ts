/**
 * Performance Tracking for Component System
 *
 * Provides real-time monitoring and metrics collection for component hydration,
 * enabling performance optimization and debugging.
 */

export interface ComponentPerformanceMetrics {
  componentType: string;
  hydrationTime: number;
  mountTime: number;
  propsSize: number;
  success: boolean;
  error?: string;
  timestamp: number;
}

export interface PerformanceReport {
  totalComponents: number;
  averageHydrationTime: number;
  successRate: number;
  componentBreakdown: Record<
    string,
    {
      count: number;
      averageTime: number;
      failureRate: number;
    }
  >;
  performanceIssues: Array<{
    type: 'slow_hydration' | 'large_props' | 'frequent_failures';
    componentType: string;
    severity: 'low' | 'medium' | 'high';
    description: string;
  }>;
}

export class PerformanceTracker {
  private metrics: ComponentPerformanceMetrics[] = [];
  private readonly MAX_METRICS = 1000; // Keep last 1000 entries
  private readonly SLOW_HYDRATION_THRESHOLD = 50; // ms
  private readonly LARGE_PROPS_THRESHOLD = 1024; // bytes

  /**
   * Record component hydration performance
   */
  public recordHydration(
    componentType: string,
    hydrationTime: number,
    mountTime: number,
    propsSize: number,
    success: boolean,
    error?: string
  ): void {
    const metric: ComponentPerformanceMetrics = {
      componentType,
      hydrationTime,
      mountTime,
      propsSize,
      success,
      error,
      timestamp: Date.now()
    };

    this.metrics.push(metric);

    // Keep metrics array size manageable
    if (this.metrics.length > this.MAX_METRICS) {
      this.metrics.shift();
    }

    // Log performance issues in development
    if (typeof window !== 'undefined' && window.location?.hostname === 'localhost') {
      this.checkForPerformanceIssues(metric);
    }
  }

  /**
   * Generate comprehensive performance report
   */
  public generateReport(): PerformanceReport {
    if (this.metrics.length === 0) {
      return {
        totalComponents: 0,
        averageHydrationTime: 0,
        successRate: 0,
        componentBreakdown: {},
        performanceIssues: []
      };
    }

    const totalComponents = this.metrics.length;
    const successfulMetrics = this.metrics.filter((m) => m.success);
    const averageHydrationTime =
      successfulMetrics.reduce((sum, m) => sum + m.hydrationTime, 0) / successfulMetrics.length;
    const successRate = (successfulMetrics.length / totalComponents) * 100;

    // Component breakdown
    const componentBreakdown: Record<
      string,
      {
        count: number;
        averageTime: number;
        failureRate: number;
      }
    > = {};
    
    // Temporary storage for calculations
    const tempBreakdown: Record<
      string,
      {
        metrics: ComponentPerformanceMetrics[];
        count: number;
        failures: number;
      }
    > = {};
    
    for (const metric of this.metrics) {
      if (!tempBreakdown[metric.componentType]) {
        tempBreakdown[metric.componentType] = {
          metrics: [],
          count: 0,
          failures: 0
        };
      }

      tempBreakdown[metric.componentType].metrics.push(metric);
      tempBreakdown[metric.componentType].count++;
      if (!metric.success) {
        tempBreakdown[metric.componentType].failures++;
      }
    }

    // Calculate averages and failure rates
    for (const [componentType, data] of Object.entries(tempBreakdown)) {
      const successfulMetrics = data.metrics.filter((m: ComponentPerformanceMetrics) => m.success);
      componentBreakdown[componentType] = {
        count: data.count,
        averageTime:
          successfulMetrics.length > 0
            ? successfulMetrics.reduce(
                (sum: number, m: ComponentPerformanceMetrics) => sum + m.hydrationTime,
                0
              ) / successfulMetrics.length
            : 0,
        failureRate: (data.failures / data.count) * 100
      };
    }

    // Identify performance issues
    const performanceIssues = this.identifyPerformanceIssues(componentBreakdown);

    return {
      totalComponents,
      averageHydrationTime,
      successRate,
      componentBreakdown,
      performanceIssues
    };
  }

  /**
   * Get real-time statistics
   */
  public getRealtimeStats(): {
    componentsHydratedLastMinute: number;
    averageHydrationTimeLastMinute: number;
    currentSuccessRate: number;
  } {
    const oneMinuteAgo = Date.now() - 60000;
    const recentMetrics = this.metrics.filter((m) => m.timestamp > oneMinuteAgo);

    const successfulRecent = recentMetrics.filter((m) => m.success);
    const averageTime =
      successfulRecent.length > 0
        ? successfulRecent.reduce((sum, m) => sum + m.hydrationTime, 0) / successfulRecent.length
        : 0;

    return {
      componentsHydratedLastMinute: recentMetrics.length,
      averageHydrationTimeLastMinute: averageTime,
      currentSuccessRate:
        recentMetrics.length > 0 ? (successfulRecent.length / recentMetrics.length) * 100 : 0
    };
  }

  /**
   * Clear all metrics (useful for testing)
   */
  public clearMetrics(): void {
    this.metrics = [];
  }

  /**
   * Check for immediate performance issues
   */
  private checkForPerformanceIssues(metric: ComponentPerformanceMetrics): void {
    if (metric.hydrationTime > this.SLOW_HYDRATION_THRESHOLD) {
      console.warn(
        `🐌 Slow component hydration detected: ${metric.componentType} took ${metric.hydrationTime}ms`
      );
    }

    if (metric.propsSize > this.LARGE_PROPS_THRESHOLD) {
      console.warn(
        `📦 Large props detected: ${metric.componentType} has ${metric.propsSize} bytes of props`
      );
    }

    if (!metric.success && metric.error) {
      console.error(`❌ Component hydration failed: ${metric.componentType} - ${metric.error}`);
    }
  }

  /**
   * Identify systematic performance issues
   */
  private identifyPerformanceIssues(
    componentBreakdown: Record<
      string,
      {
        count: number;
        averageTime: number;
        failureRate: number;
      }
    >
  ): PerformanceReport['performanceIssues'] {
    const issues: PerformanceReport['performanceIssues'] = [];

    for (const [componentType, stats] of Object.entries(componentBreakdown)) {
      // Check for slow components
      if (stats.averageTime > this.SLOW_HYDRATION_THRESHOLD) {
        issues.push({
          type: 'slow_hydration',
          componentType,
          severity: stats.averageTime > 100 ? 'high' : 'medium',
          description: `Average hydration time of ${stats.averageTime.toFixed(1)}ms exceeds threshold`
        });
      }

      // Check for high failure rates
      if (stats.failureRate > 10) {
        issues.push({
          type: 'frequent_failures',
          componentType,
          severity: stats.failureRate > 25 ? 'high' : 'medium',
          description: `Failure rate of ${stats.failureRate.toFixed(1)}% is concerning`
        });
      }
    }

    return issues;
  }

  /**
   * Export metrics for external analysis
   */
  public exportMetrics(): ComponentPerformanceMetrics[] {
    return [...this.metrics]; // Return copy to prevent mutation
  }
}

/**
 * Singleton instance for application-wide performance tracking
 */
export const performanceTracker = new PerformanceTracker();

export default PerformanceTracker;
