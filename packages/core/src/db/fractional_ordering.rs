/// Calculates the fractional order for inserting a node between two siblings
pub struct FractionalOrderCalculator;

impl FractionalOrderCalculator {
    /// Smallest gap between adjacent sibling orders that is still safe to
    /// bisect. Below it, callers re-spread the siblings before inserting, so
    /// repeated same-position inserts never converge onto an existing key.
    pub const MIN_GAP: f64 = 0.0001;

    /// Calculate order value for inserting between prev and next.
    ///
    /// Returns a deterministic midpoint or endpoint. Under SQLite's serialized
    /// writes, concurrent inserts cannot interleave, so no jitter is needed.
    ///
    /// # Examples
    /// ```
    /// # use nodespace_core::db::fractional_ordering::FractionalOrderCalculator;
    /// assert_eq!(FractionalOrderCalculator::calculate_order(None, None), 1.0);
    /// assert_eq!(FractionalOrderCalculator::calculate_order(None, Some(2.0)), 1.0);
    /// assert_eq!(FractionalOrderCalculator::calculate_order(Some(3.0), None), 4.0);
    /// assert_eq!(FractionalOrderCalculator::calculate_order(Some(1.0), Some(3.0)), 2.0);
    /// ```
    pub fn calculate_order(prev_order: Option<f64>, next_order: Option<f64>) -> f64 {
        match (prev_order, next_order) {
            (None, None) => 1.0,
            (None, Some(next)) => next - 1.0,
            (Some(prev), None) => prev + 1.0,
            (Some(prev), Some(next)) => (prev + next) / 2.0,
        }
    }

    /// Check if rebalancing is needed (gap too small)
    pub fn needs_rebalancing(orders: &[f64]) -> bool {
        if orders.len() < 2 {
            return false;
        }

        for i in 1..orders.len() {
            let gap = orders[i] - orders[i - 1];
            if gap < Self::MIN_GAP {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_order_first_child() {
        assert_eq!(FractionalOrderCalculator::calculate_order(None, None), 1.0);
    }

    #[test]
    fn test_calculate_order_before_first() {
        assert_eq!(
            FractionalOrderCalculator::calculate_order(None, Some(2.0)),
            1.0
        );
    }

    #[test]
    fn test_calculate_order_after_last() {
        assert_eq!(
            FractionalOrderCalculator::calculate_order(Some(3.0), None),
            4.0
        );
    }

    #[test]
    fn test_calculate_order_between() {
        assert_eq!(
            FractionalOrderCalculator::calculate_order(Some(1.0), Some(3.0)),
            2.0
        );
    }

    #[test]
    fn test_calculate_order_deterministic() {
        // Same inputs always produce the same output
        let r1 = FractionalOrderCalculator::calculate_order(None, None);
        let r2 = FractionalOrderCalculator::calculate_order(None, None);
        assert_eq!(r1, r2);
        assert_eq!(r1, 1.0);
    }

    #[test]
    fn test_rapid_sequential_inserts_produce_distinct_orders() {
        // Simulate rapid sequential inserts at the same position.
        // Under serialized SQLite writes each insert reads the committed last_order
        // before the next insert runs, so orders are strictly increasing.
        let mut last = FractionalOrderCalculator::calculate_order(None, None);
        for _ in 0..10 {
            let next = FractionalOrderCalculator::calculate_order(Some(last), None);
            assert!(
                next > last,
                "orders must be strictly increasing: {} -> {}",
                last,
                next
            );
            last = next;
        }
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
