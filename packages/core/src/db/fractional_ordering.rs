/// Calculates the fractional order for inserting a node between two siblings
pub struct FractionalOrderCalculator;

impl FractionalOrderCalculator {
    /// Calculate order value for inserting between prev and next
    ///
    /// # Examples
    /// ```
    /// // Insert at beginning (before all)
    /// calculate_order(None, Some(1.0)) => 0.5
    ///
    /// // Insert at end (after all)
    /// calculate_order(Some(3.0), None) => 4.0
    ///
    /// // Insert between two nodes
    /// calculate_order(Some(1.0), Some(2.0)) => 1.5
    /// ```
    pub fn calculate_order(prev_order: Option<f64>, next_order: Option<f64>) -> f64 {
        match (prev_order, next_order) {
            (None, None) => 1.0,                             // First child
            (None, Some(next)) => next - 1.0,                // Before first
            (Some(prev), None) => prev + 1.0,                // After last
            (Some(prev), Some(next)) => (prev + next) / 2.0, // Between siblings
        }
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
    fn test_needs_rebalancing() {
        assert!(!FractionalOrderCalculator::needs_rebalancing(&[
            1.0, 2.0, 3.0
        ]));
        assert!(FractionalOrderCalculator::needs_rebalancing(&[
            1.0, 1.00001, 1.00002
        ]));
    }
}
