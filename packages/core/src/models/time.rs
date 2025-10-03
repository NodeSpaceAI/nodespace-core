//! Time Provider Abstraction
//!
//! Provides a trait-based abstraction for time operations to enable
//! deterministic testing without thread sleeps.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::time::{TimeProvider, SystemTimeProvider};
//! use chrono::Utc;
//!
//! let provider = SystemTimeProvider;
//! let now = provider.now();
//! assert!(now <= Utc::now());
//! ```

use chrono::{DateTime, Utc};

/// Trait for providing current time
///
/// This abstraction enables:
/// - Deterministic testing (use `MockTimeProvider`)
/// - Time-based testing without thread sleeps
/// - Easier testing of time-dependent logic
pub trait TimeProvider: Send + Sync {
    /// Get the current UTC time
    fn now(&self) -> DateTime<Utc>;
}

/// System time provider using actual system clock
///
/// This is the default implementation for production use.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTimeProvider;

impl TimeProvider for SystemTimeProvider {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Mock time provider for testing
///
/// Allows setting a specific time for deterministic tests.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::time::{TimeProvider, MockTimeProvider};
/// use chrono::{Utc, Duration};
///
/// let mut provider = MockTimeProvider::new();
/// let time1 = provider.now();
///
/// // Advance time by 1 hour
/// provider.advance(Duration::hours(1));
/// let time2 = provider.now();
///
/// assert_eq!(time2 - time1, Duration::hours(1));
/// ```
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct MockTimeProvider {
    current_time: DateTime<Utc>,
}

#[cfg(test)]
impl MockTimeProvider {
    /// Create a new mock time provider starting at the current time
    pub fn new() -> Self {
        Self {
            current_time: Utc::now(),
        }
    }

    /// Create a mock time provider with a specific starting time
    pub fn with_time(time: DateTime<Utc>) -> Self {
        Self { current_time: time }
    }

    /// Set the current time to a specific value
    pub fn set_time(&mut self, time: DateTime<Utc>) {
        self.current_time = time;
    }

    /// Advance time by the given duration
    pub fn advance(&mut self, duration: chrono::Duration) {
        self.current_time += duration;
    }
}

#[cfg(test)]
impl TimeProvider for MockTimeProvider {
    fn now(&self) -> DateTime<Utc> {
        self.current_time
    }
}

#[cfg(test)]
impl Default for MockTimeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_system_time_provider() {
        let provider = SystemTimeProvider;
        let now1 = provider.now();
        let now2 = Utc::now();

        // Should be very close (within 1 second)
        assert!((now2 - now1).num_milliseconds().abs() < 1000);
    }

    #[test]
    fn test_mock_time_provider_new() {
        let provider = MockTimeProvider::new();
        let now = Utc::now();

        // Should be very close to current time
        assert!((now - provider.now()).num_milliseconds().abs() < 1000);
    }

    #[test]
    fn test_mock_time_provider_with_time() {
        let specific_time = Utc::now() - Duration::days(7);
        let provider = MockTimeProvider::with_time(specific_time);

        assert_eq!(provider.now(), specific_time);
    }

    #[test]
    fn test_mock_time_provider_set_time() {
        let mut provider = MockTimeProvider::new();
        let new_time = Utc::now() + Duration::hours(3);

        provider.set_time(new_time);

        assert_eq!(provider.now(), new_time);
    }

    #[test]
    fn test_mock_time_provider_advance() {
        let mut provider = MockTimeProvider::new();
        let start_time = provider.now();

        provider.advance(Duration::hours(2));

        assert_eq!(provider.now() - start_time, Duration::hours(2));
    }

    #[test]
    fn test_mock_time_provider_deterministic() {
        let base_time = Utc::now();
        let mut provider1 = MockTimeProvider::with_time(base_time);
        let mut provider2 = MockTimeProvider::with_time(base_time);

        // Both should return same time
        assert_eq!(provider1.now(), provider2.now());

        // Advance both by same amount
        provider1.advance(Duration::minutes(30));
        provider2.advance(Duration::minutes(30));

        // Should still be equal
        assert_eq!(provider1.now(), provider2.now());
    }
}
