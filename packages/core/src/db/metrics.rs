//! Performance Metrics Collection for A/B Testing
//!
//! This module provides metrics collection and reporting capabilities for
//! comparing NodeStore backend implementations during the SurrealDB migration.
//!
//! # Features
//!
//! - **Operation Tracking**: Record execution times for database operations
//! - **Statistical Analysis**: Calculate percentiles, averages, and performance deltas
//! - **CSV Export**: Export raw metrics for external analysis
//! - **Report Generation**: Generate formatted performance summaries
//!
//! # Usage
//!
//! ```rust
//! use nodespace_core::db::metrics::MetricsCollector;
//! use std::time::Duration;
//!
//! let mut collector = MetricsCollector::new();
//!
//! collector.record("create_node", Duration::from_millis(10), Duration::from_millis(12));
//! collector.record("query_nodes", Duration::from_millis(50), Duration::from_millis(45));
//!
//! let report = collector.generate_report("Turso", "SurrealDB");
//! println!("{}", report);
//! ```

use anyhow::Result;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Metrics collector for A/B testing performance data
#[derive(Debug, Clone)]
pub struct MetricsCollector {
    metrics: Vec<OperationMetric>,
}

/// Single operation metric recording
#[derive(Debug, Clone)]
pub struct OperationMetric {
    /// Name of the operation (e.g., "create_node", "query_nodes")
    pub operation: String,
    /// Execution duration for backend A
    pub backend_a_duration: Duration,
    /// Execution duration for backend B
    pub backend_b_duration: Duration,
    /// Timestamp when the metric was recorded
    pub timestamp: SystemTime,
}

/// Aggregated statistics for a set of operations
#[derive(Debug, Clone)]
pub struct MetricsStats {
    /// Operation name
    pub operation: String,
    /// Number of samples
    pub count: usize,
    /// Average duration for backend A
    pub avg_a: Duration,
    /// Average duration for backend B
    pub avg_b: Duration,
    /// p50 (median) for backend A
    pub p50_a: Duration,
    /// p50 (median) for backend B
    pub p50_b: Duration,
    /// p95 percentile for backend A
    pub p95_a: Duration,
    /// p95 percentile for backend B
    pub p95_b: Duration,
    /// p99 percentile for backend A
    pub p99_a: Duration,
    /// p99 percentile for backend B
    pub p99_b: Duration,
    /// Average performance delta percentage
    pub avg_delta_percent: f64,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            metrics: Vec::new(),
        }
    }

    /// Record an operation metric
    ///
    /// # Arguments
    ///
    /// * `operation` - Name of the operation
    /// * `backend_a_duration` - Execution time for backend A
    /// * `backend_b_duration` - Execution time for backend B
    pub fn record(
        &mut self,
        operation: &str,
        backend_a_duration: Duration,
        backend_b_duration: Duration,
    ) {
        self.metrics.push(OperationMetric {
            operation: operation.to_string(),
            backend_a_duration,
            backend_b_duration,
            timestamp: SystemTime::now(),
        });
    }

    /// Get all recorded metrics
    pub fn metrics(&self) -> &[OperationMetric] {
        &self.metrics
    }

    /// Calculate statistics for a specific operation
    ///
    /// Returns None if no metrics exist for the operation
    pub fn stats_for(&self, operation: &str) -> Option<MetricsStats> {
        let operation_metrics: Vec<_> = self
            .metrics
            .iter()
            .filter(|m| m.operation == operation)
            .collect();

        if operation_metrics.is_empty() {
            return None;
        }

        let count = operation_metrics.len();

        // Calculate averages
        let avg_a = Duration::from_nanos(
            operation_metrics
                .iter()
                .map(|m| m.backend_a_duration.as_nanos())
                .sum::<u128>() as u64
                / count as u64,
        );

        let avg_b = Duration::from_nanos(
            operation_metrics
                .iter()
                .map(|m| m.backend_b_duration.as_nanos())
                .sum::<u128>() as u64
                / count as u64,
        );

        // Calculate percentiles
        let mut a_durations: Vec<_> = operation_metrics
            .iter()
            .map(|m| m.backend_a_duration)
            .collect();
        let mut b_durations: Vec<_> = operation_metrics
            .iter()
            .map(|m| m.backend_b_duration)
            .collect();

        a_durations.sort();
        b_durations.sort();

        let p50_a = percentile(&a_durations, 50.0);
        let p50_b = percentile(&b_durations, 50.0);
        let p95_a = percentile(&a_durations, 95.0);
        let p95_b = percentile(&b_durations, 95.0);
        let p99_a = percentile(&a_durations, 99.0);
        let p99_b = percentile(&b_durations, 99.0);

        // Calculate average delta
        let avg_delta_percent = if avg_a.as_nanos() > 0 {
            ((avg_b.as_nanos() as f64 - avg_a.as_nanos() as f64) / avg_a.as_nanos() as f64) * 100.0
        } else {
            0.0
        };

        Some(MetricsStats {
            operation: operation.to_string(),
            count,
            avg_a,
            avg_b,
            p50_a,
            p50_b,
            p95_a,
            p95_b,
            p99_a,
            p99_b,
            avg_delta_percent,
        })
    }

    /// Generate a formatted performance report
    ///
    /// # Arguments
    ///
    /// * `backend_a_name` - Display name for backend A (e.g., "Turso")
    /// * `backend_b_name` - Display name for backend B (e.g., "SurrealDB")
    pub fn generate_report(&self, backend_a_name: &str, backend_b_name: &str) -> String {
        let mut report = String::new();
        report.push_str("=== A/B Testing Performance Report ===\n");
        report.push_str(&format!(
            "Backend A: {} | Backend B: {}\n",
            backend_a_name, backend_b_name
        ));
        report.push_str(&format!(
            "Total operations recorded: {}\n\n",
            self.metrics.len()
        ));

        // Get unique operations
        let mut operations: Vec<String> =
            self.metrics.iter().map(|m| m.operation.clone()).collect();
        operations.sort();
        operations.dedup();

        for operation in operations {
            if let Some(stats) = self.stats_for(&operation) {
                report.push_str(&format!("Operation: {}\n", stats.operation));
                report.push_str(&format!("  Samples: {}\n", stats.count));
                report.push_str(&format!(
                    "  Average: {:.2}ms ({}) vs {:.2}ms ({}) | Δ {:.2}%\n",
                    stats.avg_a.as_secs_f64() * 1000.0,
                    backend_a_name,
                    stats.avg_b.as_secs_f64() * 1000.0,
                    backend_b_name,
                    stats.avg_delta_percent
                ));
                report.push_str(&format!(
                    "  p50: {:.2}ms vs {:.2}ms\n",
                    stats.p50_a.as_secs_f64() * 1000.0,
                    stats.p50_b.as_secs_f64() * 1000.0
                ));
                report.push_str(&format!(
                    "  p95: {:.2}ms vs {:.2}ms\n",
                    stats.p95_a.as_secs_f64() * 1000.0,
                    stats.p95_b.as_secs_f64() * 1000.0
                ));
                report.push_str(&format!(
                    "  p99: {:.2}ms vs {:.2}ms\n\n",
                    stats.p99_a.as_secs_f64() * 1000.0,
                    stats.p99_b.as_secs_f64() * 1000.0
                ));
            }
        }

        // Calculate overall statistics
        if !self.metrics.is_empty() {
            let total_a: Duration = self.metrics.iter().map(|m| m.backend_a_duration).sum();
            let total_b: Duration = self.metrics.iter().map(|m| m.backend_b_duration).sum();
            let avg_delta = if total_a.as_nanos() > 0 {
                ((total_b.as_nanos() as f64 - total_a.as_nanos() as f64)
                    / total_a.as_nanos() as f64)
                    * 100.0
            } else {
                0.0
            };

            report.push_str("=== Overall Summary ===\n");
            report.push_str(&format!(
                "Total time: {:.2}ms ({}) vs {:.2}ms ({}) | Δ {:.2}%\n",
                total_a.as_secs_f64() * 1000.0,
                backend_a_name,
                total_b.as_secs_f64() * 1000.0,
                backend_b_name,
                avg_delta
            ));
        }

        report
    }

    /// Export metrics to CSV file
    ///
    /// # Arguments
    ///
    /// * `path` - Output path for CSV file
    /// * `backend_a_name` - Display name for backend A
    /// * `backend_b_name` - Display name for backend B
    pub fn export_csv(
        &self,
        path: &Path,
        backend_a_name: &str,
        backend_b_name: &str,
    ) -> Result<()> {
        use std::fs::File;
        use std::io::Write;

        let mut file = File::create(path)?;

        // Write header
        writeln!(
            file,
            "timestamp,operation,{}_duration_ns,{}_duration_ns,delta_percent",
            backend_a_name, backend_b_name
        )?;

        // Write metrics
        for metric in &self.metrics {
            let timestamp = metric
                .timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let delta = if metric.backend_a_duration.as_nanos() > 0 {
                ((metric.backend_b_duration.as_nanos() as f64
                    - metric.backend_a_duration.as_nanos() as f64)
                    / metric.backend_a_duration.as_nanos() as f64)
                    * 100.0
            } else {
                0.0
            };

            writeln!(
                file,
                "{},{},{},{},{:.2}",
                timestamp,
                metric.operation,
                metric.backend_a_duration.as_nanos(),
                metric.backend_b_duration.as_nanos(),
                delta
            )?;
        }

        Ok(())
    }

    /// Clear all recorded metrics
    pub fn clear(&mut self) {
        self.metrics.clear();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate percentile from sorted durations
fn percentile(sorted_durations: &[Duration], percentile: f64) -> Duration {
    if sorted_durations.is_empty() {
        return Duration::from_nanos(0);
    }

    let index = ((percentile / 100.0) * (sorted_durations.len() as f64 - 1.0)).round() as usize;
    sorted_durations[index.min(sorted_durations.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_metrics_collector_record() {
        let mut collector = MetricsCollector::new();

        collector.record(
            "test_op",
            Duration::from_millis(10),
            Duration::from_millis(12),
        );
        collector.record(
            "test_op",
            Duration::from_millis(11),
            Duration::from_millis(13),
        );

        assert_eq!(collector.metrics().len(), 2);
        assert_eq!(collector.metrics()[0].operation, "test_op");
    }

    #[test]
    fn test_metrics_stats_calculation() {
        let mut collector = MetricsCollector::new();

        // Record 5 samples
        collector.record(
            "create",
            Duration::from_millis(10),
            Duration::from_millis(12),
        );
        collector.record(
            "create",
            Duration::from_millis(11),
            Duration::from_millis(13),
        );
        collector.record(
            "create",
            Duration::from_millis(9),
            Duration::from_millis(11),
        );
        collector.record(
            "create",
            Duration::from_millis(10),
            Duration::from_millis(12),
        );
        collector.record(
            "create",
            Duration::from_millis(10),
            Duration::from_millis(12),
        );

        let stats = collector.stats_for("create").unwrap();
        assert_eq!(stats.count, 5);
        assert!(stats.avg_a.as_millis() >= 9 && stats.avg_a.as_millis() <= 11);
        assert!(stats.avg_b.as_millis() >= 11 && stats.avg_b.as_millis() <= 13);
    }

    #[test]
    fn test_percentile_calculation() {
        let durations = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
            Duration::from_millis(50),
        ];

        assert_eq!(percentile(&durations, 0.0), Duration::from_millis(10));
        assert_eq!(percentile(&durations, 50.0), Duration::from_millis(30));
        assert_eq!(percentile(&durations, 100.0), Duration::from_millis(50));
    }

    #[test]
    fn test_generate_report() {
        let mut collector = MetricsCollector::new();

        collector.record(
            "create",
            Duration::from_millis(10),
            Duration::from_millis(12),
        );
        collector.record(
            "query",
            Duration::from_millis(50),
            Duration::from_millis(45),
        );

        let report = collector.generate_report("Turso", "SurrealDB");

        assert!(report.contains("Turso"));
        assert!(report.contains("SurrealDB"));
        assert!(report.contains("create"));
        assert!(report.contains("query"));
    }

    #[test]
    fn test_clear_metrics() {
        let mut collector = MetricsCollector::new();

        collector.record("test", Duration::from_millis(10), Duration::from_millis(12));
        assert_eq!(collector.metrics().len(), 1);

        collector.clear();
        assert_eq!(collector.metrics().len(), 0);
    }
}
