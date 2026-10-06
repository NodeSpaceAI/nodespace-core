//! The hold on the app's shared gRPC channel until a start attempt has found
//! this app's own daemon answering on the socket.
//!
//! The frontend and extensions call the daemon as soon as the shell renders,
//! but the launcher only later asks which daemon holds the socket, and boots
//! out a daemon that runs another binary (ADR-084 §4.3). Until a start attempt
//! has asked the daemon answering and found it to be this app's, the shared
//! channel's connector waits on this hold instead of dialing, so nothing on
//! the channel reaches a daemon that has not been checked. The check dials a
//! connection of its own.
//!
//! The hold covers the calls made through the app's own gRPC client and
//! nothing else: the CLI, the skill and agent sessions dial the socket
//! themselves, and so does anything else on the machine.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

/// How long after the app builds its client a held call waits before it
/// fails.
///
/// Two minutes is a chosen bound, not a derived one. A start attempt normally
/// ends well inside it, but several of its steps have no timeout of their own
/// (the service manager, `codesign` and `lsof` subprocesses, the sidecar
/// copies), and the clock also counts the bundled-model copy that runs before
/// the attempt. The limit is what a held call sees when an attempt hangs in
/// one of those, or leaves the hold on.
pub(crate) const STARTUP_HOLD_LIMIT: Duration = Duration::from_secs(120);

/// Where a [`StartupHold`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupHoldState {
    /// No start attempt has found this app's daemon answering yet; held calls
    /// wait.
    Holding,
    /// Still holding past the limit; calls fail at once, until a release.
    Overdue,
    /// The channel dials the socket.
    Released,
}

/// A latch shared by a client and its channel's connector. It starts holding
/// and is released once, for good. Clones share the latch.
#[derive(Clone, Debug)]
pub(crate) struct StartupHold {
    released: Arc<watch::Sender<bool>>,
    /// When a call stops waiting for the release. `None` for a hold created
    /// released, which never waits.
    deadline: Option<Instant>,
}

impl StartupHold {
    /// A hold that is already released: the channel dials on its first call.
    pub(crate) fn released() -> Self {
        Self {
            released: Arc::new(watch::Sender::new(true)),
            deadline: None,
        }
    }

    /// A hold that waits for [`release`](Self::release), and fails calls once
    /// `limit` has passed without one.
    pub(crate) fn holding(limit: Duration) -> Self {
        Self {
            released: Arc::new(watch::Sender::new(false)),
            deadline: Some(Instant::now() + limit),
        }
    }

    /// Ends the hold for good, whether or not its limit has passed: waiting
    /// calls go ahead, and later ones dial at once.
    pub(crate) fn release(&self) {
        self.released.send_replace(true);
    }

    pub(crate) fn state(&self) -> StartupHoldState {
        if *self.released.borrow() {
            StartupHoldState::Released
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            StartupHoldState::Overdue
        } else {
            StartupHoldState::Holding
        }
    }

    /// Waits until the hold is released. Fails once the limit has passed
    /// without a release, at once when it already has.
    pub(crate) async fn wait(&self) -> std::io::Result<()> {
        let mut released = self.released.subscribe();
        let release = released.wait_for(|released| *released);
        // The sender lives in `self`, so the wait cannot see it dropped.
        let released = match self.deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, release)
                .await
                .is_ok_and(|release| release.is_ok()),
            None => release.await.is_ok(),
        };
        if released {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no start attempt has found this app's daemon answering on the socket",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StartupHold, StartupHoldState};
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn a_held_wait_ends_when_the_hold_is_released() {
        let hold = StartupHold::holding(Duration::from_secs(60));
        let waiting = tokio::spawn({
            let hold = hold.clone();
            async move { hold.wait().await }
        });
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(!waiting.is_finished(), "the wait must hold until release");
        assert_eq!(hold.state(), StartupHoldState::Holding);

        hold.release();

        waiting
            .await
            .expect("the wait does not panic")
            .expect("a released hold lets the wait through");
        assert_eq!(hold.state(), StartupHoldState::Released);
    }

    #[tokio::test(start_paused = true)]
    async fn a_held_wait_fails_only_once_the_limit_has_passed() {
        let hold = StartupHold::holding(Duration::from_secs(60));
        let waiting = tokio::spawn({
            let hold = hold.clone();
            async move { hold.wait().await }
        });

        tokio::time::sleep(Duration::from_secs(59)).await;
        assert!(
            !waiting.is_finished(),
            "nothing but the limit fails a wait while the hold is on"
        );
        assert_eq!(hold.state(), StartupHoldState::Holding);

        tokio::time::sleep(Duration::from_secs(1)).await;
        let waited = waiting.await.expect("the wait does not panic");
        assert_eq!(
            waited.map_err(|error| error.kind()),
            Err(std::io::ErrorKind::TimedOut)
        );
        assert_eq!(hold.state(), StartupHoldState::Overdue);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hold_past_its_limit_fails_waits_at_once_until_it_is_released() {
        let hold = StartupHold::holding(Duration::from_secs(60));
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(hold.state(), StartupHoldState::Overdue);

        let before = tokio::time::Instant::now();
        assert!(hold.wait().await.is_err());
        assert_eq!(
            tokio::time::Instant::now(),
            before,
            "a wait on an overdue hold fails without waiting"
        );
        assert_eq!(
            hold.state(),
            StartupHoldState::Overdue,
            "passing the limit does not end the hold"
        );

        hold.release();
        assert_eq!(hold.state(), StartupHoldState::Released);
        assert!(hold.wait().await.is_ok());
    }

    #[tokio::test]
    async fn a_released_hold_never_waits() {
        let hold = StartupHold::released();
        assert_eq!(hold.state(), StartupHoldState::Released);
        assert!(hold.wait().await.is_ok());
    }
}
