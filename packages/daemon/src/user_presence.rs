//! Asking the person at this device's screen to confirm a change no program may
//! make on its own: approving a remote tool, and setting or removing a
//! credential.
//!
//! On macOS the daemon asks the system itself (LocalAuthentication, policy
//! `deviceOwnerAuthentication`): a system panel reading "nodespaced is trying to
//! <reason>" that takes Touch ID, a paired Apple Watch or the login password. A
//! program running as the same user cannot answer it the way it can type into a
//! terminal or write a node. Everywhere else there is no such check, so the
//! change is refused ([`UserPresence::unavailable`]).
//!
//! The rules, each one tested:
//!
//! - **Asked in the daemon, every time.** A new evaluation per change, with
//!   Touch ID reuse off, so one confirmation never covers a second change.
//! - **No screen, no change.** Over SSH, at a locked screen, or with another
//!   user at the console the system refuses at once; that is "confirm at this
//!   Mac's screen", and nothing is written. Whether the system *could* ask is
//!   never trusted (it says yes over SSH): only an evaluation's own answer is.
//! - **One at a time.** While one confirmation is open, another is refused
//!   without being shown, and for [`COOLDOWN`] after one that was not confirmed
//!   (cancelled, failed or unanswered) no new one is shown, so a program cannot
//!   pile dialogs on the person until one is clicked through.
//! - **Bounded.** An unanswered dialog is closed after [`ANSWER_LIMIT`] and
//!   counts as not confirmed.
//! - **The reason is ours.** It is written here from fixed words, the tool's
//!   name, its code and its target; whatever a node holds is shown as ASCII,
//!   with `\u{..}` escapes for anything else, and cut to a bounded length.
//!
//! The dialog runs on a blocking thread (never the main thread, which the tray
//! owns, and never an async worker), inside an autorelease pool.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the person has to answer before the dialog is closed.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(60);
/// How long no new dialog is shown after one that was not confirmed.
pub const COOLDOWN: Duration = Duration::from_secs(30);
/// Longest part of a reason taken from a node (the target), in characters.
const MAX_REASON_DETAIL: usize = 120;

/// What one evaluation answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// The person at the screen confirmed.
    Confirmed,
    /// Shown, and cancelled, failed or not answered in time.
    NotConfirmed,
    /// Nobody can be asked here (no screen session, no login password, a
    /// platform with no such check): why, for the message.
    Unavailable(String),
}

/// Asks the person at the screen. Blocks until they answer, the time limit
/// passes or the system refuses; called on a blocking thread only.
pub trait PresenceCheck: Send + Sync {
    fn confirm(&self, reason: &str) -> Presence;
}

/// Why a change was not confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceRefusal {
    /// Shown, and cancelled, failed or not answered in time.
    NotConfirmed,
    /// Nobody can be asked here.
    Unavailable(String),
    /// Another confirmation is open.
    Busy,
    /// One was just not confirmed; no new one is shown for a while.
    CoolingDown,
}

impl PresenceRefusal {
    /// The refusal as a status: the message says what to do.
    pub fn status(&self) -> tonic::Status {
        match self {
            Self::NotConfirmed => tonic::Status::permission_denied(
                "not confirmed at this device: the system dialog was cancelled or not answered in time",
            ),
            Self::Unavailable(why) => tonic::Status::failed_precondition(format!(
                "{why}: run the command again at this Mac's own screen"
            )),
            Self::Busy => tonic::Status::unavailable(
                "another confirmation is open in a system dialog on this device: answer it first",
            ),
            Self::CoolingDown => tonic::Status::unavailable(format!(
                "a confirmation was just cancelled; try again in {} seconds",
                COOLDOWN.as_secs()
            )),
        }
    }
}

#[derive(Default)]
struct State {
    open: bool,
    quiet_until: Option<Instant>,
}

/// The daemon's one way to ask the person at the screen, shared by every
/// change that needs it (one confirmation at a time across all of them).
pub struct UserPresence {
    check: Arc<dyn PresenceCheck>,
    state: Arc<Mutex<State>>,
    cooldown: Duration,
}

/// Clears the open flag when the evaluation ends, on every path (a panic in
/// the check included), and starts the cooldown unless told not to.
struct Open {
    state: Arc<Mutex<State>>,
    cooldown: Duration,
    cool_down: bool,
}

impl Drop for Open {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.open = false;
        if self.cool_down {
            state.quiet_until = Some(Instant::now() + self.cooldown);
        }
    }
}

impl UserPresence {
    pub fn new(check: Arc<dyn PresenceCheck>) -> Self {
        Self::with_cooldown(check, COOLDOWN)
    }

    fn with_cooldown(check: Arc<dyn PresenceCheck>, cooldown: Duration) -> Self {
        Self {
            check,
            state: Arc::default(),
            cooldown,
        }
    }

    /// The check this platform has: LocalAuthentication on macOS, none
    /// elsewhere.
    pub fn for_this_platform() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::new(Arc::new(local_authentication::LocalAuthentication::new()))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::unavailable()
        }
    }

    /// No way to ask: every change that needs a confirmation is refused.
    pub fn unavailable() -> Self {
        Self::new(Arc::new(Unavailable))
    }

    /// Asks the person at the screen to confirm `reason`. `Ok` only when they
    /// did.
    pub async fn confirm(&self, reason: String) -> Result<(), PresenceRefusal> {
        let mut open = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.open {
                return Err(PresenceRefusal::Busy);
            }
            if state
                .quiet_until
                .is_some_and(|until| Instant::now() < until)
            {
                return Err(PresenceRefusal::CoolingDown);
            }
            state.open = true;
            Open {
                state: Arc::clone(&self.state),
                cooldown: self.cooldown,
                cool_down: true,
            }
        };
        let check = Arc::clone(&self.check);
        // The guard goes with the evaluation, so a caller that stops waiting
        // (a client that hung up) leaves the dialog counted as open until it
        // has really ended.
        let answer = tokio::task::spawn_blocking(move || {
            let answer = check.confirm(&reason);
            // Only a dialog that was shown and not confirmed keeps the next
            // one back; one that could not be shown showed nothing.
            open.cool_down = answer == Presence::NotConfirmed;
            drop(open);
            answer
        })
        .await
        .unwrap_or(Presence::NotConfirmed);
        match answer {
            Presence::Confirmed => Ok(()),
            Presence::NotConfirmed => Err(PresenceRefusal::NotConfirmed),
            Presence::Unavailable(why) => Err(PresenceRefusal::Unavailable(why)),
        }
    }
}

/// A platform with no way to ask.
struct Unavailable;

impl PresenceCheck for Unavailable {
    fn confirm(&self, _reason: &str) -> Presence {
        Presence::Unavailable(
            "this device has no way to confirm a change at its screen".to_string(),
        )
    }
}

/// `text` as ASCII: every character outside printable ASCII shown as a
/// `\u{..}` escape (a control, a bidi or an invisible character, a look-alike
/// letter), cut to `max` characters with `...` when longer.
fn ascii_safe(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let shown = if c == ' ' || c.is_ascii_graphic() {
            c.to_string()
        } else {
            format!("\\u{{{:X}}}", c as u32)
        };
        if out.chars().count() + shown.chars().count() > max {
            out.push_str("...");
            break;
        }
        out.push_str(&shown);
    }
    out
}

/// The dialog's reason for approving a tool. The system shows it after
/// "nodespaced is trying to".
pub fn approval_reason(name: &str, code: &str, target: &str) -> String {
    format!(
        "approve the remote tool \u{201C}{}\u{201D} (code {}): {}",
        ascii_safe(name, 64),
        ascii_safe(code, 9),
        ascii_safe(target, MAX_REASON_DETAIL)
    )
}

/// The dialog's reason for setting or removing a credential.
pub fn credential_reason(name: &str) -> String {
    format!(
        "change the credential \u{201C}{}\u{201D}",
        ascii_safe(name, 64)
    )
}

#[cfg(target_os = "macos")]
mod local_authentication {
    //! The macOS check: one `LAContext` per evaluation.

    use std::sync::mpsc;
    use std::time::Duration;

    use super::{Presence, PresenceCheck, ANSWER_LIMIT};

    /// `LAErrorSystemCancel`: the system ended it (the screen locked, the
    /// session went to the background, another user switched in).
    const SYSTEM_CANCEL: isize = -4;
    /// `LAErrorPasscodeNotSet`: the account has no login password.
    const PASSCODE_NOT_SET: isize = -5;
    /// `LAErrorNotInteractive`: no panel can be shown (no screen session: a
    /// remote login, a daemon outside the user's session).
    const NOT_INTERACTIVE: isize = -1004;

    /// One evaluation of the device-owner policy, as [`evaluate`] drives it.
    pub(super) trait Evaluation {
        /// Starts it; the answer is sent once on `reply`, from any thread: `Ok`
        /// or the `LAError` code.
        fn start(&self, reason: &str, reply: mpsc::SyncSender<Result<(), isize>>);
        /// Ends one still open (its answer then comes as `LAErrorAppCancel`).
        fn invalidate(&self);
    }

    /// What an `LAError` code means here. Only the codes that say nobody could
    /// be asked are "unavailable"; every other failure is "not confirmed".
    pub(super) fn outcome_of(code: isize) -> Presence {
        match code {
            NOT_INTERACTIVE => Presence::Unavailable(
                "no dialog can be shown here (no screen session: a remote login, or the daemon is outside the logged-in session)"
                    .to_string(),
            ),
            SYSTEM_CANCEL => Presence::Unavailable(
                "the system ended the dialog (the screen locked, or another user is at the console)"
                    .to_string(),
            ),
            PASSCODE_NOT_SET => Presence::Unavailable(
                "this account has no login password, so the system cannot confirm it is you"
                    .to_string(),
            ),
            _ => Presence::NotConfirmed,
        }
    }

    /// Runs one evaluation: waits up to `limit` for the answer, then closes the
    /// dialog and counts it as not confirmed.
    pub(super) fn evaluate(evaluation: &dyn Evaluation, reason: &str, limit: Duration) -> Presence {
        let (reply, answer) = mpsc::sync_channel(1);
        evaluation.start(reason, reply);
        match answer.recv_timeout(limit) {
            Ok(Ok(())) => Presence::Confirmed,
            Ok(Err(code)) => outcome_of(code),
            Err(_) => {
                evaluation.invalidate();
                Presence::NotConfirmed
            }
        }
    }

    /// The real check. Every call builds a new context.
    pub(super) struct LocalAuthentication {
        limit: Duration,
    }

    impl LocalAuthentication {
        pub(super) fn new() -> Self {
            Self {
                limit: ANSWER_LIMIT,
            }
        }
    }

    impl PresenceCheck for LocalAuthentication {
        fn confirm(&self, reason: &str) -> Presence {
            objc2::rc::autoreleasepool(|_| evaluate(&Context::new(), reason, self.limit))
        }
    }

    /// One `LAContext`, used for one evaluation and dropped after it.
    struct Context(objc2::rc::Retained<objc2_local_authentication::LAContext>);

    impl Context {
        fn new() -> Self {
            // SAFETY: `new` makes a fresh context; setting the Touch ID reuse
            // window on it has no other effect.
            let context = unsafe { objc2_local_authentication::LAContext::new() };
            // No reuse of an earlier Touch ID match: every change is its own.
            unsafe { context.setTouchIDAuthenticationAllowableReuseDuration(0.0) };
            Self(context)
        }
    }

    impl Evaluation for Context {
        fn start(&self, reason: &str, reply: mpsc::SyncSender<Result<(), isize>>) {
            use objc2::runtime::Bool;
            use objc2_foundation::{NSError, NSString};
            let block = block2::RcBlock::new(move |ok: Bool, error: *mut NSError| {
                let answer = if ok.as_bool() {
                    Ok(())
                } else if error.is_null() {
                    Err(-1)
                } else {
                    // SAFETY: a non-null error the reply hands over is a live
                    // NSError for the length of the call.
                    Err(unsafe { (*error).code() })
                };
                // The waiter may have given up (the time limit): then nobody
                // reads it.
                let _ = reply.try_send(answer);
            });
            let reason = NSString::from_str(reason);
            // SAFETY: the reply block only sends on a channel, so it may run on
            // any thread, as the system calls it.
            unsafe {
                self.0.evaluatePolicy_localizedReason_reply(
                    objc2_local_authentication::LAPolicy::DeviceOwnerAuthentication,
                    &reason,
                    &block,
                );
            }
        }

        fn invalidate(&self) {
            // SAFETY: invalidating ends an open evaluation; on one already
            // ended it does nothing.
            unsafe { self.0.invalidate() };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Mutex;

        /// An evaluation that answers `answer` at once, or never.
        struct Scripted {
            answer: Option<Result<(), isize>>,
            reason: Mutex<Option<String>>,
            invalidated: AtomicBool,
        }

        impl Scripted {
            fn answering(answer: Option<Result<(), isize>>) -> Self {
                Self {
                    answer,
                    reason: Mutex::new(None),
                    invalidated: AtomicBool::new(false),
                }
            }
        }

        impl Evaluation for Scripted {
            fn start(&self, reason: &str, reply: mpsc::SyncSender<Result<(), isize>>) {
                *self.reason.lock().unwrap() = Some(reason.to_string());
                if let Some(answer) = self.answer {
                    reply.send(answer).unwrap();
                }
            }
            fn invalidate(&self) {
                self.invalidated.store(true, Ordering::SeqCst);
            }
        }

        #[test]
        fn only_a_yes_confirms_and_only_the_no_screen_codes_are_unavailable() {
            let run = |answer| evaluate(&Scripted::answering(Some(answer)), "r", ANSWER_LIMIT);
            assert_eq!(run(Ok(())), Presence::Confirmed);
            // Cancelled, failed, fell back, cancelled by us, a bad context:
            // shown, and not confirmed.
            for code in [-1, -2, -3, -9, -10, -6, -8, 0, 7] {
                assert_eq!(run(Err(code)), Presence::NotConfirmed, "{code}");
            }
            for code in [NOT_INTERACTIVE, SYSTEM_CANCEL, PASSCODE_NOT_SET] {
                assert!(matches!(run(Err(code)), Presence::Unavailable(_)), "{code}");
            }
        }

        #[test]
        fn the_reason_is_handed_to_the_system_as_given() {
            let scripted = Scripted::answering(Some(Ok(())));
            evaluate(&scripted, "approve the remote tool", ANSWER_LIMIT);
            assert_eq!(
                scripted.reason.lock().unwrap().as_deref(),
                Some("approve the remote tool")
            );
            assert!(!scripted.invalidated.load(Ordering::SeqCst));
        }

        /// No answer in time: the dialog is closed, and it is a no.
        #[test]
        fn an_unanswered_dialog_is_closed_and_is_not_confirmed() {
            let silent = Scripted::answering(None);
            assert_eq!(
                evaluate(&silent, "r", Duration::from_millis(50)),
                Presence::NotConfirmed
            );
            assert!(silent.invalidated.load(Ordering::SeqCst));
        }

        /// Shows a real system dialog (Touch ID or the password). Never run by
        /// the test suites: it is ignored, and even a run of the ignored tests
        /// (`rust:benchmark` runs them) skips it unless
        /// `NODESPACE_SHOW_REAL_DIALOG=1` is set. Run it by hand at the Mac's
        /// screen. Over SSH it answers at once with "unavailable".
        #[test]
        #[ignore = "shows a real system dialog; run by hand at the screen"]
        fn a_real_dialog_at_the_screen() {
            if std::env::var_os("NODESPACE_SHOW_REAL_DIALOG").as_deref()
                != Some(std::ffi::OsStr::new("1"))
            {
                println!("skipped: set NODESPACE_SHOW_REAL_DIALOG=1 to show the dialog");
                return;
            }
            let answer = LocalAuthentication::new()
                .confirm("check that NodeSpace can ask you (a test: nothing is changed)");
            println!("answer: {answer:?}");
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A check for tests: answers as told and records every reason.

    use super::{Presence, PresenceCheck};
    use std::sync::{mpsc, Arc, Mutex};

    pub(crate) enum Answer {
        Yes,
        No,
        Unavailable,
        /// Waits until the test sends an answer on the paired sender.
        Wait(Mutex<mpsc::Receiver<bool>>),
    }

    pub(crate) struct FakePresence {
        answer: Answer,
        pub(crate) reasons: Mutex<Vec<String>>,
        /// Run once while the dialog is open, before it answers (something
        /// else writing meanwhile).
        during: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    impl FakePresence {
        pub(crate) fn new(answer: Answer) -> Arc<Self> {
            Arc::new(Self {
                answer,
                reasons: Mutex::new(Vec::new()),
                during: Mutex::new(None),
            })
        }

        /// A check that waits for the test's answer.
        pub(crate) fn waiting() -> (Arc<Self>, mpsc::Sender<bool>) {
            let (tx, rx) = mpsc::channel();
            (Self::new(Answer::Wait(Mutex::new(rx))), tx)
        }

        pub(crate) fn while_open(self: &Arc<Self>, run: impl FnOnce() + Send + 'static) {
            *self.during.lock().unwrap() = Some(Box::new(run));
        }

        pub(crate) fn asked(&self) -> Vec<String> {
            self.reasons.lock().unwrap().clone()
        }
    }

    impl PresenceCheck for FakePresence {
        fn confirm(&self, reason: &str) -> Presence {
            self.reasons.lock().unwrap().push(reason.to_string());
            if let Some(run) = self.during.lock().unwrap().take() {
                run();
            }
            match &self.answer {
                Answer::Yes => Presence::Confirmed,
                Answer::No => Presence::NotConfirmed,
                Answer::Unavailable => Presence::Unavailable("no screen here".to_string()),
                Answer::Wait(rx) => match rx.lock().unwrap().recv() {
                    Ok(true) => Presence::Confirmed,
                    _ => Presence::NotConfirmed,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Answer, FakePresence};
    use super::*;

    #[tokio::test]
    async fn only_a_confirmation_lets_a_change_through() {
        let yes = UserPresence::new(FakePresence::new(Answer::Yes));
        assert_eq!(yes.confirm("r".into()).await, Ok(()));
        let no = UserPresence::new(FakePresence::new(Answer::No));
        assert_eq!(
            no.confirm("r".into()).await,
            Err(PresenceRefusal::NotConfirmed)
        );
        let nobody = UserPresence::new(FakePresence::new(Answer::Unavailable));
        assert!(matches!(
            nobody.confirm("r".into()).await,
            Err(PresenceRefusal::Unavailable(_))
        ));
        assert!(matches!(
            UserPresence::unavailable().confirm("r".into()).await,
            Err(PresenceRefusal::Unavailable(_))
        ));
    }

    /// While one dialog is open a second is refused without being shown.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_confirmation_while_one_is_open_is_refused_unshown() {
        let (fake, answer) = FakePresence::waiting();
        let presence = Arc::new(UserPresence::new(fake.clone()));
        let first = {
            let presence = Arc::clone(&presence);
            tokio::spawn(async move { presence.confirm("first".into()).await })
        };
        while fake.asked().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            presence.confirm("second".into()).await,
            Err(PresenceRefusal::Busy)
        );
        answer.send(true).unwrap();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(fake.asked(), ["first"]);
        // Once it has ended, the next one is shown.
        answer.send(true).unwrap();
        assert_eq!(presence.confirm("third".into()).await, Ok(()));
        assert_eq!(fake.asked(), ["first", "third"]);
    }

    /// After a dialog that was not confirmed, none is shown for a while; one
    /// that was confirmed, or could not be shown, keeps nothing back.
    #[tokio::test]
    async fn a_cancelled_dialog_keeps_the_next_one_back_for_the_cooldown() {
        let fake = FakePresence::new(Answer::No);
        let presence = UserPresence::new(fake.clone());
        assert_eq!(
            presence.confirm("first".into()).await,
            Err(PresenceRefusal::NotConfirmed)
        );
        assert_eq!(
            presence.confirm("second".into()).await,
            Err(PresenceRefusal::CoolingDown)
        );
        assert_eq!(fake.asked(), ["first"]);
        // The cooldown ends.
        let fake = FakePresence::new(Answer::No);
        let short = UserPresence::with_cooldown(fake.clone(), Duration::from_millis(20));
        let _ = short.confirm("a".into()).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            short.confirm("b".into()).await,
            Err(PresenceRefusal::NotConfirmed)
        );
        assert_eq!(fake.asked(), ["a", "b"]);
        for answer in [Answer::Yes, Answer::Unavailable] {
            let fake = FakePresence::new(answer);
            let presence = UserPresence::new(fake.clone());
            let _ = presence.confirm("a".into()).await;
            let _ = presence.confirm("b".into()).await;
            assert_eq!(fake.asked(), ["a", "b"]);
        }
    }

    #[test]
    fn the_reason_shows_a_node_text_as_ascii_and_keeps_the_code() {
        let reason = approval_reason(
            "lookup",
            "1A2B-3C4D",
            "Runs: /usr/bin/gi\u{1D42D} \u{202E}fdp.exe\n[\"x\"]",
        );
        assert!(
            reason.starts_with("approve the remote tool \u{201C}lookup\u{201D} (code 1A2B-3C4D): ")
        );
        let detail = reason.split_once("): ").unwrap().1;
        assert!(detail.is_ascii(), "{detail}");
        assert!(
            detail.contains("gi\\u{1D42D}")
                && detail.contains("\\u{202E}")
                && detail.contains("\\u{A}"),
            "{detail}"
        );
        // A long target is cut; the name and the code stay whole.
        let long = approval_reason("lookup", "1A2B-3C4D", &"x".repeat(500));
        assert!(long.contains("(code 1A2B-3C4D)"));
        assert!(long.ends_with("..."), "{long}");
        assert!(long.len() < 220, "{}", long.len());
        assert_eq!(
            credential_reason("tok\u{200B}"),
            "change the credential \u{201C}tok\\u{200B}\u{201D}"
        );
    }
}
