/// Calculates the fractional order for inserting a node between two siblings
pub struct FractionalOrderCalculator;

impl FractionalOrderCalculator {
    /// Calculate order value for inserting between prev and next
    ///
    /// Includes a tiny random offset to prevent exact order collisions when multiple
    /// insertions happen concurrently with eventual consistency (e.g., SurrealDB with RocksDB).
    ///
    /// # Examples
    /// ```text
    /// // Insert at beginning (before all)
    /// calculate_order(None, Some(1.0)) => ~0.5 (with tiny jitter)
    ///
    /// // Insert at end (after all)
    /// calculate_order(Some(3.0), None) => ~4.0 (with tiny jitter)
    ///
    /// // Insert between two nodes
    /// calculate_order(Some(1.0), Some(2.0)) => ~1.5 (with tiny jitter)
    /// ```
    pub fn calculate_order(prev_order: Option<f64>, next_order: Option<f64>) -> f64 {
        // Use generate_jitter() to prevent code duplication (DRY principle)
        let jitter = Self::generate_jitter();

        let base = match (prev_order, next_order) {
            (None, None) => 1.0,                             // First child
            (None, Some(next)) => next - 1.0,                // Before first
            (Some(prev), None) => prev + 1.0,                // After last
            (Some(prev), Some(next)) => (prev + next) / 2.0, // Between siblings
        };

        base + jitter
    }

    /// Check if rebalancing is needed (gap too small)
    pub fn needs_rebalancing(orders: &[f64]) -> bool {
        if orders.len() < 2 {
            return false;
        }

        for i in 1..orders.len() {
            let gap = orders[i] - orders[i - 1];
            if gap < 0.0001 {
                // Precision threshold
                return true;
            }
        }
        false
    }

    /// Rebalance orders to have even spacing
    ///
    /// # Example
    /// Input:  [1.0, 1.0001, 1.0002, 1.0003]
    /// Output: [1.0, 2.0, 3.0, 4.0]
    pub fn rebalance(count: usize) -> Vec<f64> {
        (1..=count).map(|i| i as f64).collect()
    }

    /// Generate a jitter value for order uniqueness
    ///
    /// Issue #865: Extracted for use in atomic SurrealDB queries.
    /// Returns a value in range [0.0, 0.001) that can be passed to SurrealDB
    /// and added to order values within the query itself.
    ///
    /// Uses the same entropy sources as calculate_order:
    /// - Nanoseconds since epoch (rapid change)
    /// - Process-unique counter (guaranteed uniqueness within process)
    pub fn generate_jitter() -> f64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let counter_val = COUNTER.fetch_add(1, Ordering::Relaxed);
        let time_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // Combine both sources for uniqueness
        let combined = time_nanos.wrapping_add(counter_val);
        (combined % 1_000_000) as f64 / 1_000_000_000.0 // 0.0 to 0.001
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to check approximate equality (jitter adds up to 0.001)
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn test_calculate_order_first_child() {
        let result = FractionalOrderCalculator::calculate_order(None, None);
        assert!(approx_eq(result, 1.0), "Expected ~1.0, got {}", result);
    }

    #[test]
    fn test_calculate_order_before_first() {
        let result = FractionalOrderCalculator::calculate_order(None, Some(2.0));
        assert!(approx_eq(result, 1.0), "Expected ~1.0, got {}", result);
    }

    #[test]
    fn test_calculate_order_after_last() {
        let result = FractionalOrderCalculator::calculate_order(Some(3.0), None);
        assert!(approx_eq(result, 4.0), "Expected ~4.0, got {}", result);
    }

    #[test]
    fn test_calculate_order_between() {
        let result = FractionalOrderCalculator::calculate_order(Some(1.0), Some(3.0));
        assert!(approx_eq(result, 2.0), "Expected ~2.0, got {}", result);
    }

    #[test]
    fn test_calculate_order_uniqueness() {
        // Two consecutive calls should produce different values due to jitter
        let result1 = FractionalOrderCalculator::calculate_order(None, None);
        let result2 = FractionalOrderCalculator::calculate_order(None, None);
        // They should be approximately equal (both ~1.0)
        assert!(approx_eq(result1, 1.0), "Expected ~1.0, got {}", result1);
        assert!(approx_eq(result2, 1.0), "Expected ~1.0, got {}", result2);
        // But they should NOT be exactly equal (jitter ensures uniqueness)
        assert_ne!(
            result1, result2,
            "Two calls should produce different values: {} vs {}",
            result1, result2
        );
    }

    #[test]
    fn test_needs_rebalancing() {
        assert!(!FractionalOrderCalculator::needs_rebalancing(&[
            1.0, 2.0, 3.0
        ]));
        assert!(FractionalOrderCalculator::needs_rebalancing(&[
            1.0, 1.00001, 1.00002
        ]));
    }
}
