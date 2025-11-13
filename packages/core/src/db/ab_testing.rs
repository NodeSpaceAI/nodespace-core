//! A/B Testing Framework for Backend Comparison
//!
//! This module provides infrastructure for parallel testing of multiple NodeStore
//! implementations (Turso vs SurrealDB), enabling automated performance monitoring
//! and validation during the SurrealDB migration (Epic #461, Phase 3).
//!
//! # Architecture
//!
//! - **Parallel Execution**: Run identical operations on both backends simultaneously
//! - **Result Validation**: Ensure backends produce identical results
//! - **Performance Monitoring**: Track and compare execution times
//! - **Metrics Collection**: Aggregate data for analysis and reporting
//!
//! # Usage Example
//!
//! ```rust,no_run
//! use nodespace_core::db::{ABTestRunner, TursoStore, SurrealStore, DatabaseService};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let turso = Arc::new(TursoStore::new(Arc::new(DatabaseService::new_in_memory().await?)));
//!     let surreal = Arc::new(SurrealStore::new(":memory:".to_string()).await?);
//!
//!     let runner = ABTestRunner::new(turso, surreal);
//!
//!     let result = runner.run_parallel_test("create_node", |store| async move {
//!         store.create_node(test_node()).await
//!     }).await?;
//!
//!     println!("Performance delta: {:.2}%", result.delta_percent);
//!
//!     Ok(())
//! }
//! ```

use crate::db::metrics::MetricsCollector;
use crate::db::NodeStore;
use anyhow::{Context, Result};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// A/B test runner for comparing two NodeStore implementations
///
/// Executes identical operations on both backends, validates results match,
/// and collects performance metrics for comparison.
pub struct ABTestRunner {
    backend_a: Arc<dyn NodeStore>,
    backend_b: Arc<dyn NodeStore>,
    metrics_collector: Arc<Mutex<MetricsCollector>>,
    backend_a_name: String,
    backend_b_name: String,
}

/// Result of an A/B test comparing two backend implementations
#[derive(Debug)]
pub struct ABTestResult<T> {
    /// Duration for backend A
    pub backend_a_duration: std::time::Duration,
    /// Duration for backend B
    pub backend_b_duration: std::time::Duration,
    /// Performance delta as percentage (positive means B is slower)
    pub delta_percent: f64,
    /// The result value (validated to be identical across backends)
    pub result: T,
}

impl ABTestRunner {
    /// Create a new A/B test runner with two backend implementations
    ///
    /// # Arguments
    ///
    /// * `backend_a` - First backend (typically Turso/baseline)
    /// * `backend_b` - Second backend (typically SurrealDB/new)
    pub fn new(backend_a: Arc<dyn NodeStore>, backend_b: Arc<dyn NodeStore>) -> Self {
        Self::with_names(backend_a, backend_b, "Backend A", "Backend B")
    }

    /// Create a new A/B test runner with named backends
    ///
    /// # Arguments
    ///
    /// * `backend_a` - First backend
    /// * `backend_b` - Second backend
    /// * `name_a` - Display name for backend A (e.g., "Turso")
    /// * `name_b` - Display name for backend B (e.g., "SurrealDB")
    pub fn with_names(
        backend_a: Arc<dyn NodeStore>,
        backend_b: Arc<dyn NodeStore>,
        name_a: impl Into<String>,
        name_b: impl Into<String>,
    ) -> Self {
        Self {
            backend_a,
            backend_b,
            metrics_collector: Arc::new(Mutex::new(MetricsCollector::new())),
            backend_a_name: name_a.into(),
            backend_b_name: name_b.into(),
        }
    }

    /// Run a test operation on both backends in parallel
    ///
    /// # Type Parameters
    ///
    /// * `F` - Async function that takes a NodeStore and returns a Result<T>
    /// * `T` - Result type that must implement PartialEq and Debug for validation
    ///
    /// # Arguments
    ///
    /// * `test_name` - Name of the operation being tested (for metrics)
    /// * `test_fn` - Async function to execute on both backends
    ///
    /// # Returns
    ///
    /// ABTestResult containing execution times, performance delta, and validated result
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Either backend operation fails
    /// - Results from backends don't match
    pub async fn run_parallel_test<F, Fut, T>(
        &self,
        test_name: &str,
        test_fn: F,
    ) -> Result<ABTestResult<T>>
    where
        F: Fn(Arc<dyn NodeStore>) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T>> + Send,
        T: PartialEq + std::fmt::Debug + Send,
    {
        // Execute on backend A
        let start_a = Instant::now();
        let result_a = test_fn(self.backend_a.clone()).await.with_context(|| {
            format!(
                "Backend A ({}) failed for {}",
                self.backend_a_name, test_name
            )
        })?;
        let duration_a = start_a.elapsed();

        // Execute on backend B
        let start_b = Instant::now();
        let result_b = test_fn(self.backend_b.clone()).await.with_context(|| {
            format!(
                "Backend B ({}) failed for {}",
                self.backend_b_name, test_name
            )
        })?;
        let duration_b = start_b.elapsed();

        // Validate results match
        if result_a != result_b {
            anyhow::bail!(
                "Backend results diverged for '{}'\n{}: {:?}\n{}: {:?}",
                test_name,
                self.backend_a_name,
                result_a,
                self.backend_b_name,
                result_b
            );
        }

        // Calculate performance delta
        let delta_percent = calculate_delta(duration_a, duration_b);

        // Record metrics
        let mut metrics = self.metrics_collector.lock().await;
        metrics.record(test_name, duration_a, duration_b);
        drop(metrics); // Release lock

        Ok(ABTestResult {
            backend_a_duration: duration_a,
            backend_b_duration: duration_b,
            delta_percent,
            result: result_a,
        })
    }

    /// Get a snapshot of the metrics collector
    ///
    /// Returns a cloned MetricsCollector with all recorded operations
    pub async fn metrics(&self) -> MetricsCollector {
        self.metrics_collector.lock().await.clone()
    }

    /// Generate a performance report from collected metrics
    ///
    /// Returns a formatted string with summary statistics
    pub async fn generate_report(&self) -> String {
        let metrics = self.metrics_collector.lock().await;
        metrics.generate_report(&self.backend_a_name, &self.backend_b_name)
    }

    /// Export metrics to CSV file
    ///
    /// # Arguments
    ///
    /// * `path` - Output path for CSV file
    pub async fn export_csv(&self, path: &std::path::Path) -> Result<()> {
        let metrics = self.metrics_collector.lock().await;
        metrics.export_csv(path, &self.backend_a_name, &self.backend_b_name)
    }

    /// Get backend names for display
    pub fn backend_names(&self) -> (&str, &str) {
        (&self.backend_a_name, &self.backend_b_name)
    }
}

/// Calculate performance delta percentage
///
/// Positive value means duration_b is slower than duration_a
/// Negative value means duration_b is faster than duration_a
///
/// # Formula
///
/// `delta = ((duration_b - duration_a) / duration_a) * 100`
fn calculate_delta(duration_a: std::time::Duration, duration_b: std::time::Duration) -> f64 {
    if duration_a.as_nanos() == 0 {
        return 0.0;
    }

    let a_nanos = duration_a.as_nanos() as f64;
    let b_nanos = duration_b.as_nanos() as f64;

    ((b_nanos - a_nanos) / a_nanos) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_delta() {
        use std::time::Duration;

        // B is 50% slower than A
        let a = Duration::from_millis(100);
        let b = Duration::from_millis(150);
        assert_eq!(calculate_delta(a, b), 50.0);

        // B is 50% faster than A
        let a = Duration::from_millis(100);
        let b = Duration::from_millis(50);
        assert_eq!(calculate_delta(a, b), -50.0);

        // Identical performance
        let a = Duration::from_millis(100);
        let b = Duration::from_millis(100);
        assert_eq!(calculate_delta(a, b), 0.0);

        // Zero duration A (edge case)
        let a = Duration::from_nanos(0);
        let b = Duration::from_millis(100);
        assert_eq!(calculate_delta(a, b), 0.0);
    }
}
