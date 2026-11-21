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
//! - **The dialog is on the Mac's screen.** The daemon runs in the logged-in
//!   user's session, so a request that comes in over SSH still puts the panel
//!   on the Mac's own screen, where only someone at the Mac can answer it. At
//!   a locked screen, when the session has switched to another user, or when
//!   the daemon itself runs outside a logged-in session, the system refuses at
//!   once; that is "confirm at this Mac's screen", and nothing is written.
//!   Whether the system *could* ask is never trusted: only an evaluation's own
//!   answer is.
//! - **One at a time, and not in a hurry.** While one confirmation is open,
//!   another is refused without being shown. After one that was not confirmed
//!   (cancelled, failed or unanswered) no new one is shown for [`COOLDOWN`],
//!   doubling with each further one within [`PRESSURE_WINDOW`] up to
//!   [`MAX_COOLDOWN`], so a program cannot pile dialogs on the person until
//!   one is clicked through. After one that was confirmed none is shown for
//!   [`AFTER_CONFIRMED`], so a program cannot slip a request in right behind
//!   the person's own. A refusal for either reason says that something else
//!   may be asking, and is logged.
//! - **Bounded.** An unanswered dialog is closed after [`ANSWER_LIMIT`] and
//!   counts as not confirmed.
//! - **The reason is ours.** It is written here from fixed words, the tool's
//!   name, its code, the warnings about its program and its target, in an
//!   order of our own (the code, the name and the warnings first); whatever a
//!   node holds is shown as ASCII, with `\u{..}` escapes for anything else,
//!   and cut to a bounded length.
//!
//! The dialog runs on a blocking thread (never the main thread, which the tray
//! owns, and never an async worker), inside an autorelease pool.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the person has to answer before the dialog is closed.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(60);
/// How long no new dialog is shown after one that was not confirmed.
pub const COOLDOWN: Duration = Duration::from_secs(30);
/// The longest the cooldown grows to.
pub const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// How far back dialogs that were not confirmed count towards a longer
/// cooldown.
pub const PRESSURE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How long no new dialog is shown after one that was confirmed.
pub const AFTER_CONFIRMED: Duration = Duration::from_secs(5);
/// Longest part of a reason taken from a node (the target), in characters.
const MAX_REASON_DETAIL: usize = 120;
/// Most characters of the program path, or the method and host, the dialog
/// shows.
const MAX_DIALOG_TARGET: usize = 160;

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
    /// One was just not confirmed (or just confirmed); no new one is shown
    /// for a while.
    CoolingDown,
}

/// Said with a refusal the person may not have caused: something else on the
/// Mac may be asking.
const SOMETHING_ELSE_MAY_BE_ASKING: &str = "if you did not start another confirmation \
yourself, another program on this Mac asked: do not confirm a dialog you did not start";

impl PresenceRefusal {
    /// The refusal as a status: the message says what to do.
    pub fn status(&self) -> tonic::Status {
        match self {
            Self::NotConfirmed => tonic::Status::permission_denied(
                "not confirmed at this device: the system dialog was cancelled or not answered in time",
            ),
            Self::Unavailable(why) => tonic::Status::failed_precondition(format!(
                "{why}: confirm it at this Mac's own screen, with the screen unlocked"
            )),
            Self::Busy => tonic::Status::unavailable(format!(
                "another confirmation is open in a system dialog on this Mac; {SOMETHING_ELSE_MAY_BE_ASKING}"
            )),
            Self::CoolingDown => tonic::Status::unavailable(format!(
                "a confirmation was asked for just now, so none is shown for a while (up to {} minutes after several cancelled ones); {SOMETHING_ELSE_MAY_BE_ASKING}",
                MAX_COOLDOWN.as_secs() / 60
            )),
        }
    }
}

/// How long the gate keeps the next dialog back (shorter in tests).
#[derive(Clone, Copy)]
struct Timing {
    cooldown: Duration,
    max_cooldown: Duration,
    window: Duration,
    after_confirmed: Duration,
}

impl Timing {
    const DAEMON: Self = Self {
        cooldown: COOLDOWN,
        max_cooldown: MAX_COOLDOWN,
        window: PRESSURE_WINDOW,
        after_confirmed: AFTER_CONFIRMED,
    };
}

#[derive(Default)]
struct State {
    open: bool,
    quiet_until: Option<Instant>,
    /// When the recent dialogs that were not confirmed ended.
    unconfirmed: std::collections::VecDeque<Instant>,
}

impl State {
    /// Records how a shown dialog ended, and how long the next one waits.
    fn ended(&mut self, confirmed: bool, timing: Timing) {
        let now = Instant::now();
        if confirmed {
            self.quiet_until = Some(now + timing.after_confirmed);
            return;
        }
        while self
            .unconfirmed
            .front()
            .is_some_and(|at| now.duration_since(*at) > timing.window)
        {
            self.unconfirmed.pop_front();
        }
        self.unconfirmed.push_back(now);
        let doublings = (self.unconfirmed.len() - 1).min(16) as u32;
        let wait = timing
            .cooldown
            .saturating_mul(2u32.saturating_pow(doublings))
            .min(timing.max_cooldown);
        self.quiet_until = Some(now + wait);
    }
}

/// The daemon's one way to ask the person at the screen, shared by every
/// change that needs it (one confirmation at a time across all of them).
pub struct UserPresence {
    check: Arc<dyn PresenceCheck>,
    state: Arc<Mutex<State>>,
    timing: Timing,
}

/// Clears the open flag when the evaluation ends, on every path (a panic in
/// the check included), and keeps the next dialog back as it ended.
struct Open {
    state: Arc<Mutex<State>>,
    timing: Timing,
    /// How it ended: `None` when nothing was shown.
    ended: Option<bool>,
}

impl Drop for Open {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.open = false;
        if let Some(confirmed) = self.ended {
            state.ended(confirmed, self.timing);
        }
    }
}

impl UserPresence {
    pub fn new(check: Arc<dyn PresenceCheck>) -> Self {
        Self::with_timing(check, Timing::DAEMON)
    }

    /// A check with no quiet time after a dialog (only one open at a time),
    /// for tests that ask more than once in a row.
    #[cfg(test)]
    pub(crate) fn without_quiet_time(check: Arc<dyn PresenceCheck>) -> Self {
        Self::with_timing(
            check,
            Timing {
                cooldown: Duration::ZERO,
                max_cooldown: Duration::ZERO,
                window: Duration::ZERO,
                after_confirmed: Duration::ZERO,
            },
        )
    }

    fn with_timing(check: Arc<dyn PresenceCheck>, timing: Timing) -> Self {
        Self {
            check,
            state: Arc::default(),
            timing,
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
        Self::new(Arc::new(Unavailable(
            "this device has no way to confirm a change at its screen",
        )))
    }

    /// The check of a daemon that never installed one: every change that
    /// needs a confirmation is refused, and the message says what is missing.
    pub fn not_installed() -> Self {
        Self::new(Arc::new(Unavailable(
            "this daemon did not install the check that asks the person at the screen \
             (nodespace_daemon::install_approval_services at startup)",
        )))
    }

    /// Asks the person at the screen to confirm `reason`. `Ok` only when they
    /// did.
    pub async fn confirm(&self, reason: String) -> Result<(), PresenceRefusal> {
        let mut open = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.open {
                tracing::warn!(
                    "a confirmation was asked for while another was open; refused unshown"
                );
                return Err(PresenceRefusal::Busy);
            }
            if state
                .quiet_until
                .is_some_and(|until| Instant::now() < until)
            {
                tracing::warn!(
                    "a confirmation was asked for during the quiet time after the last one; refused unshown"
                );
                return Err(PresenceRefusal::CoolingDown);
            }
            state.open = true;
            Open {
                state: Arc::clone(&self.state),
                timing: self.timing,
                // Counted as not confirmed unless the check says otherwise
                // (a panic in it included).
                ended: Some(false),
            }
        };
        let check = Arc::clone(&self.check);
        // The guard goes with the evaluation, so a caller that stops waiting
        // (a client that hung up) leaves the dialog counted as open until it
        // has really ended.
        let answer = tokio::task::spawn_blocking(move || {
            let answer = check.confirm(&reason);
            open.ended = match answer {
                Presence::Confirmed => Some(true),
                Presence::NotConfirmed => Some(false),
                // Nothing was shown: nothing to keep back.
                Presence::Unavailable(_) => None,
            };
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

/// No way to ask, and why.
struct Unavailable(&'static str);

impl PresenceCheck for Unavailable {
    fn confirm(&self, _reason: &str) -> Presence {
        Presence::Unavailable(self.0.to_string())
    }
}

/// Whether `chars[at]` is a dash with four hex digits on each side: the shape
/// of a code group, which node text must not be able to draw.
fn joins_code_groups(chars: &[char], at: usize) -> bool {
    let hex = |range: std::ops::Range<usize>| {
        range.end <= chars.len() && chars[range].iter().all(char::is_ascii_hexdigit)
    };
    chars[at] == '-' && at >= 4 && hex(at - 4..at) && hex(at + 1..at + 5)
}

/// Whether `chars[at]` is a `:` that reads as part of an address: right after
/// a URL scheme (`https://`) or before a port number.
fn address_colon(chars: &[char], at: usize) -> bool {
    if chars[at] != ':' {
        return false;
    }
    if chars.get(at + 1).is_some_and(char::is_ascii_digit) {
        return true;
    }
    if chars.get(at + 1..at + 3) != Some(&['/', '/'][..]) {
        return false;
    }
    // A scheme: `[a-z][a-z0-9+.-]*`, from the start of a word.
    let start = chars[..at]
        .iter()
        .rposition(|c| {
            !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
        })
        .map_or(0, |i| i + 1);
    start < at && chars[start].is_ascii_lowercase()
}

/// Text a node holds, as the dialog shows it: ASCII only, every other
/// character (a control, a bidi or an invisible character, a look-alike
/// letter) as a `\u{..}` escape, and so are `(`, `)`, `;`, a `:` that is not
/// part of an address (`https://`, a port), and a dash between two
/// code-shaped groups, so the text cannot draw a code, a "(code ...):" or a
/// part of the dialog of its own. Cut to `max` characters with `...` when
/// longer.
fn node_text(text: &str, max: usize) -> String {
    shown_text(text, max, false)
}

/// A credential reference as the dialog shows it: like [`node_text`], but the
/// `:` of `keychain:<name>` and `env:<NAME>` is kept (a reference is checked
/// against that grammar when it is written, so it holds nothing else), and cut
/// at [`MAX_DIALOG_CREDENTIAL`].
fn reference_text(text: &str) -> String {
    shown_text(text, MAX_DIALOG_CREDENTIAL, true)
}

/// Each character of `text` as the dialog shows it (see [`node_text`]): the
/// character itself, or its `\u{..}` escape.
fn shown_chars(text: &str, keep_colons: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(at, &c)| {
            let plain = (c == ' ' || c.is_ascii_graphic())
                && !matches!(c, '(' | ')' | ';')
                && (c != ':' || keep_colons || address_colon(&chars, at))
                && !joins_code_groups(&chars, at);
            if plain {
                c.to_string()
            } else {
                format!("\\u{{{:X}}}", c as u32)
            }
        })
        .collect()
}

fn shown_text(text: &str, max: usize, keep_colons: bool) -> String {
    let mut out = String::new();
    let mut count = 0;
    for shown in shown_chars(text, keep_colons) {
        let n = shown.chars().count();
        if count + n > max {
            out.push_str("...");
            break;
        }
        out.push_str(&shown);
        count += n;
    }
    out
}

/// Of a target longer than [`MAX_DIALOG_TARGET`], the characters kept from
/// its start; the rest of the limit is kept from its end.
const DIALOG_TARGET_START: usize = 48;

/// The program path, or the method and host, as the dialog shows it: like
/// [`node_text`], whole up to [`MAX_DIALOG_TARGET`] characters. A longer one
/// keeps its start and its end (the program's folder and file name, or the
/// end of the host and its port) around an ellipsis, `\u{2026}`, which node
/// text cannot draw (it is shown as an escape), so a long path cannot push
/// the credentials out of a panel that shortens the reason.
fn target_text(text: &str) -> String {
    /// The shown characters `chars` yields while they fit in `budget`.
    fn fitting<'a>(chars: impl Iterator<Item = &'a String>, budget: usize) -> Vec<&'a str> {
        let mut kept = Vec::new();
        let mut used = 0;
        for c in chars {
            let width = c.chars().count();
            if used + width > budget {
                break;
            }
            used += width;
            kept.push(c.as_str());
        }
        kept
    }
    let shown = shown_chars(text, false);
    if shown.iter().map(|c| c.chars().count()).sum::<usize>() <= MAX_DIALOG_TARGET {
        return shown.concat();
    }
    let start = fitting(shown.iter(), DIALOG_TARGET_START);
    let mut end = fitting(
        shown.iter().rev(),
        MAX_DIALOG_TARGET - DIALOG_TARGET_START - 1,
    );
    end.reverse();
    format!("{}\u{2026}{}", start.concat(), end.concat())
}

/// Where a credential the dialog names comes from, in the order it names
/// them: keychain items first, then environment variables, then anything
/// else, each by its reference. An entry is `<what> from <reference>` (as the
/// approval preview lists it).
fn credential_order(entry: &str) -> (u8, &str) {
    match entry.rsplit_once(" from ") {
        Some((_, reference)) if reference.starts_with("keychain:") => (0, reference),
        Some((_, reference)) if reference.starts_with("env:") => (1, reference),
        _ => (2, entry),
    }
}

/// The code as the dialog shows it: the daemon made it, so it keeps its
/// dashes; anything but hex digits and dashes is dropped.
fn code_text(code: &str) -> String {
    code.chars()
        .filter(|c| *c == '-' || c.is_ascii_hexdigit())
        .take(14)
        .collect()
}

/// What the dialog for approving a tool says.
pub struct ApprovalDialog<'a> {
    /// The code the command line printed.
    pub code: &'a str,
    pub name: &'a str,
    /// The credential references the tool uses (references only).
    pub credentials: &'a [String],
    /// `runs`, `sends` or `connects to`.
    pub verb: &'a str,
    /// The program's full path, or the method and the scheme, host and port:
    /// whole up to [`MAX_DIALOG_TARGET`] characters, else its start and end.
    pub head: &'a str,
    /// The arguments, or the rest of the address.
    pub rest: &'a str,
    /// The approval pins the program file's content.
    pub content_pinned: bool,
    /// Programs of this user can replace the program file (or the
    /// interpreter it names).
    pub replaceable: bool,
    /// The program a `#!/usr/bin/env` script starts from the `PATH` (not
    /// pinned); empty when none.
    pub starts_from_path: &'a str,
}

/// Most credential references the dialog names one by one; the rest are
/// counted.
const MAX_DIALOG_CREDENTIALS: usize = 4;
/// Most characters of one credential reference the dialog shows.
const MAX_DIALOG_CREDENTIAL: usize = 64;

/// The dialog's reason for approving a tool. The system shows it after
/// "nodespaced is trying to". In this order: the code, the tool's name, and
/// the warnings about the program's pin (a file programs of this user can
/// replace, a program started from the `PATH`); then what it runs (the full
/// program path) or where it sends a call (the method and the host), whole up
/// to [`MAX_DIALOG_TARGET`] characters and else its start and its end, with
/// what is said about its pin; then the credentials it uses (at most
/// [`MAX_DIALOG_CREDENTIALS`] named, keychain references first, in an order
/// of the daemon's, each cut at [`MAX_DIALOG_CREDENTIAL`], the others
/// counted); then the arguments, or the rest of the address, cut. The real
/// code, the warnings and what is approved therefore come before anything
/// long, and a panel that shortens a long reason cuts only the parts after
/// them. Everything a node holds is shown as ASCII, with the brackets, colons
/// and dashes it could fake a code with escaped.
pub fn approval_reason(dialog: &ApprovalDialog<'_>) -> String {
    let credentials = if dialog.credentials.is_empty() {
        "with no credentials".to_string()
    } else {
        let mut ordered: Vec<&String> = dialog.credentials.iter().collect();
        ordered.sort_by(|a, b| {
            credential_order(a)
                .cmp(&credential_order(b))
                .then_with(|| a.cmp(b))
        });
        let mut named: Vec<String> = ordered
            .into_iter()
            .take(MAX_DIALOG_CREDENTIALS)
            .map(|c| reference_text(c))
            .collect();
        let more = dialog
            .credentials
            .len()
            .saturating_sub(MAX_DIALOG_CREDENTIALS);
        if more > 0 {
            named.push(format!("+{more} more"));
        }
        format!("with the credentials {}", named.join(", "))
    };
    let mut warnings = String::new();
    if dialog.replaceable {
        warnings.push_str(" (WARNING: programs of this user can replace its program file)");
    }
    if !dialog.starts_from_path.is_empty() {
        warnings.push_str(&format!(
            " (WARNING: it starts {} from the PATH, which is not pinned)",
            node_text(dialog.starts_from_path, 64)
        ));
    }
    let pinned = if dialog.content_pinned {
        " (that file's content is pinned)"
    } else {
        ""
    };
    let rest = if dialog.rest.is_empty() {
        String::new()
    } else {
        format!(", then {}", node_text(dialog.rest, MAX_REASON_DETAIL))
    };
    format!(
        "approve code {} for the remote tool \u{201C}{}\u{201D}{}, which {} {}{}, {}{}",
        code_text(dialog.code),
        node_text(dialog.name, 64),
        warnings,
        dialog.verb,
        target_text(dialog.head),
        pinned,
        credentials,
        rest
    )
}

/// The dialog's reason for setting or removing a credential.
pub fn credential_reason(name: &str) -> String {
    format!(
        "change the credential \u{201C}{}\u{201D}",
        node_text(name, 64)
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
    /// `LAErrorNotInteractive`: no panel can be shown: the daemon runs outside
    /// a logged-in screen session (a request over SSH to a daemon in the
    /// session still shows the panel on the Mac's screen).
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
                "no dialog can be shown: this daemon runs outside a logged-in screen session"
                    .to_string(),
            ),
            SYSTEM_CANCEL => Presence::Unavailable(
                "the system ended the dialog (the screen locked, or the session switched to another user)"
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
        /// Waits until the test sends an answer on the paired sender, and is a
        /// no after ten seconds without one (so a test that went wrong fails
        /// instead of hanging on the dialog).
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
                Answer::Wait(rx) => match rx
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(10))
                {
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
        // No quiet time after a confirmation here: only the open one counts.
        let presence = Arc::new(UserPresence::with_timing(fake.clone(), quick(30_000, 0)));
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

    fn quick(cooldown_ms: u64, after_confirmed_ms: u64) -> Timing {
        Timing {
            cooldown: Duration::from_millis(cooldown_ms),
            max_cooldown: Duration::from_millis(cooldown_ms * 4),
            window: Duration::from_secs(60),
            after_confirmed: Duration::from_millis(after_confirmed_ms),
        }
    }

    /// After a dialog that was not confirmed, none is shown for the cooldown;
    /// one that could not be shown keeps nothing back.
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
        let short = UserPresence::with_timing(fake.clone(), quick(20, 0));
        let _ = short.confirm("a".into()).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            short.confirm("b".into()).await,
            Err(PresenceRefusal::NotConfirmed)
        );
        assert_eq!(fake.asked(), ["a", "b"]);
        let fake = FakePresence::new(Answer::Unavailable);
        let presence = UserPresence::new(fake.clone());
        let _ = presence.confirm("a".into()).await;
        let _ = presence.confirm("b".into()).await;
        assert_eq!(fake.asked(), ["a", "b"]);
    }

    /// Each further dialog that is not confirmed keeps the next one back
    /// twice as long, up to the cap: a program cannot keep the person
    /// clicking.
    #[tokio::test]
    async fn repeated_cancelled_dialogs_keep_the_next_one_back_longer_each_time() {
        let fake = FakePresence::new(Answer::No);
        let presence = UserPresence::with_timing(fake.clone(), quick(40, 0));
        let _ = presence.confirm("a".into()).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _ = presence.confirm("b".into()).await;
        // The second cooldown is 80 ms: still on after 60.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            presence.confirm("c".into()).await,
            Err(PresenceRefusal::CoolingDown)
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _ = presence.confirm("d".into()).await;
        assert_eq!(fake.asked(), ["a", "b", "d"]);
        // Capped at four times the first.
        let mut state = State::default();
        for _ in 0..10 {
            state.ended(false, quick(40, 0));
        }
        let wait = state.quiet_until.unwrap() - Instant::now();
        assert!(wait <= Duration::from_millis(160), "{wait:?}");
    }

    /// Right after a confirmed dialog no new one is shown for a moment, so a
    /// program cannot slip its request in behind the person's own.
    #[tokio::test]
    async fn a_confirmed_dialog_keeps_the_next_one_back_for_a_moment() {
        let fake = FakePresence::new(Answer::Yes);
        let presence = UserPresence::new(fake.clone());
        assert_eq!(presence.confirm("mine".into()).await, Ok(()));
        assert_eq!(
            presence.confirm("slipped in".into()).await,
            Err(PresenceRefusal::CoolingDown)
        );
        assert_eq!(fake.asked(), ["mine"]);
        let fake = FakePresence::new(Answer::Yes);
        let short = UserPresence::with_timing(fake.clone(), quick(20, 20));
        let _ = short.confirm("a".into()).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(short.confirm("b".into()).await, Ok(()));
    }

    /// A refusal the person may not have caused says that something else on
    /// the Mac may be asking.
    #[test]
    fn a_busy_or_quiet_refusal_says_another_program_may_be_asking() {
        for refusal in [PresenceRefusal::Busy, PresenceRefusal::CoolingDown] {
            let status = refusal.status();
            assert_eq!(status.code(), tonic::Code::Unavailable);
            assert!(
                status
                    .message()
                    .contains("another program on this Mac asked"),
                "{}",
                status.message()
            );
        }
        assert!(PresenceRefusal::Unavailable("x".into())
            .status()
            .message()
            .contains("at this Mac's own screen"));
    }

    #[tokio::test]
    async fn a_daemon_that_installed_no_check_says_so() {
        let refused = UserPresence::not_installed().confirm("r".into()).await;
        assert!(
            matches!(&refused, Err(PresenceRefusal::Unavailable(why)) if why.contains("install_approval_services")),
            "{refused:?}"
        );
    }

    fn dialog<'a>(
        code: &'a str,
        credentials: &'a [String],
        verb: &'a str,
        head: &'a str,
        rest: &'a str,
    ) -> ApprovalDialog<'a> {
        ApprovalDialog {
            code,
            name: "lookup",
            credentials,
            verb,
            head,
            rest,
            content_pinned: false,
            replaceable: false,
            starts_from_path: "",
        }
    }

    /// In this order: the code, the name, then what the tool runs or where it
    /// sends a call, whole; then the credentials, bounded; the arguments come
    /// last, and are cut. Node text is ASCII.
    #[test]
    fn the_code_comes_first_and_only_the_arguments_are_cut() {
        let creds = ["TOKEN from keychain:tok".to_string()];
        let reason = approval_reason(&dialog(
            "1A2B-3C4D-5E6F",
            &creds,
            "runs",
            "/usr/bin/gi\u{1D42D}",
            "[\"\u{202E}fdp.exe\nx\"]",
        ));
        assert!(
            reason.starts_with(
                "approve code 1A2B-3C4D-5E6F for the remote tool \u{201C}lookup\u{201D}, which runs /usr/bin/gi\\u{1D42D}, with the credentials TOKEN from keychain:tok, then "
            ),
            "{reason}"
        );
        assert!(
            reason.replace(['\u{201C}', '\u{201D}'], "").is_ascii(),
            "{reason}"
        );
        assert!(
            reason.contains("\\u{202E}") && reason.contains("\\u{A}"),
            "{reason}"
        );
        // A long argument list is cut; a long program path and the
        // credentials are not.
        let long_path = format!("/opt/{}/bin/tool", "d".repeat(140));
        let long = approval_reason(&ApprovalDialog {
            content_pinned: true,
            ..dialog(
                "1A2B-3C4D-5E6F",
                &creds,
                "runs",
                &long_path,
                &"x".repeat(500),
            )
        });
        assert!(long.contains(&long_path), "{long}");
        assert!(long.contains("TOKEN from keychain:tok"));
        assert!(
            long.contains("(that file's content is pinned), with the credentials"),
            "{long}"
        );
        assert!(long.ends_with("..."), "{long}");
        assert_eq!(
            credential_reason("tok\u{200B}"),
            "change the credential \u{201C}tok\\u{200B}\u{201D}"
        );
    }

    /// Node text cannot draw a code of its own: a tool argument that holds a
    /// "(code ...): runs ..." of its choosing shows with its brackets, colons
    /// and code-shaped dashes escaped, after the real code; an address keeps
    /// its `://` and its port.
    #[test]
    fn a_code_written_into_the_node_cannot_pass_for_the_real_one() {
        let reason = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &[],
            "runs",
            "/usr/bin/git",
            r#"["(code 1A2B-3C4D-5E6F): runs /usr/bin/git; ok"]"#,
        ));
        assert!(
            reason.starts_with("approve code AAAA-BBBB-CCCC "),
            "{reason}"
        );
        let rest = reason.split_once(", then ").unwrap().1;
        assert!(!rest.contains("(code"), "{rest}");
        assert!(!rest.contains("):"), "{rest}");
        assert!(!rest.contains("; ok"), "{rest}");
        assert!(!rest.contains("1A2B-3C4D"), "{rest}");
        assert!(
            rest.contains("\\u{28}code 1A2B\\u{2D}3C4D\\u{2D}5E6F\\u{29}\\u{3A}"),
            "{rest}"
        );
        let web = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &[],
            "sends",
            "GET https://api.example.com:8443",
            "/items/{q}",
        ));
        assert!(
            web.contains("which sends GET https://api.example.com:8443, with no credentials, then /items/{q}"),
            "{web}"
        );
        // A colon that is not an address's is escaped; an ordinary dash stays.
        let plain = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &[],
            "runs",
            "/usr/bin/git-lfs",
            "a:b",
        ));
        assert!(
            plain.ends_with("which runs /usr/bin/git-lfs, with no credentials, then a\\u{3A}b"),
            "{plain}"
        );
    }

    /// A program file programs of this user can replace is flagged in the
    /// dialog itself, right after the code and the name, before the program
    /// path.
    #[test]
    fn a_replaceable_program_file_is_flagged_in_the_dialog() {
        let reason = approval_reason(&ApprovalDialog {
            content_pinned: true,
            replaceable: true,
            ..dialog("AAAA-BBBB-CCCC", &[], "runs", "/opt/homebrew/bin/git", "")
        });
        assert_eq!(
            reason,
            "approve code AAAA-BBBB-CCCC for the remote tool \u{201C}lookup\u{201D} (WARNING: programs of this user can replace its program file), which runs /opt/homebrew/bin/git (that file's content is pinned), with no credentials"
        );
        // A script that starts a program from the PATH says that instead; it
        // is not a replaceable file.
        let script = approval_reason(&ApprovalDialog {
            content_pinned: true,
            starts_from_path: "python3",
            ..dialog("AAAA-BBBB-CCCC", &[], "runs", "/usr/local/libexec/tool", "")
        });
        assert_eq!(
            script,
            "approve code AAAA-BBBB-CCCC for the remote tool \u{201C}lookup\u{201D} (WARNING: it starts python3 from the PATH, which is not pinned), which runs /usr/local/libexec/tool (that file's content is pinned), with no credentials"
        );
    }

    /// A very long program path cannot push the warnings or the credentials
    /// out of a panel that shortens the reason: the warnings come before it,
    /// and it keeps its start and its end (the program's folder and file
    /// name) around an ellipsis no node text can draw.
    #[test]
    fn a_very_long_program_path_keeps_its_start_and_end_after_the_warnings() {
        let creds = ["TOKEN from keychain:tok".to_string()];
        let long_path = format!("/Users/me/{}/bin/real-tool", "x".repeat(400));
        let reason = approval_reason(&ApprovalDialog {
            content_pinned: true,
            replaceable: true,
            starts_from_path: "python3",
            ..dialog("AAAA-BBBB-CCCC", &creds, "runs", &long_path, "")
        });
        let (before, after) = reason
            .split_once(", which runs ")
            .unwrap_or_else(|| panic!("{reason}"));
        assert_eq!(
            before,
            "approve code AAAA-BBBB-CCCC for the remote tool \u{201C}lookup\u{201D} (WARNING: programs of this user can replace its program file) (WARNING: it starts python3 from the PATH, which is not pinned)"
        );
        let (shown, rest) = after
            .split_once(" (that file's content is pinned), ")
            .unwrap_or_else(|| panic!("{reason}"));
        assert!(shown.starts_with("/Users/me/xxx"), "{shown}");
        assert!(shown.ends_with("xxx/bin/real-tool"), "{shown}");
        assert_eq!(shown.matches('\u{2026}').count(), 1, "{shown}");
        assert!(shown.chars().count() <= MAX_DIALOG_TARGET, "{shown}");
        assert_eq!(rest, "with the credentials TOKEN from keychain:tok");
        // A path at the limit is whole; node text cannot draw the ellipsis.
        let at_limit = format!("/opt/{}", "d".repeat(MAX_DIALOG_TARGET - 5));
        let whole = approval_reason(&dialog("AAAA-BBBB-CCCC", &[], "runs", &at_limit, ""));
        assert!(whole.contains(&at_limit), "{whole}");
        let drawn = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &[],
            "runs",
            "/opt/a\u{2026}b",
            "",
        ));
        assert!(drawn.contains("/opt/a\\u{2026}b"), "{drawn}");
    }

    /// The credentials the dialog names are in an order the daemon chooses,
    /// not the node: keychain references first, sorted, then environment
    /// variables, sorted; the rest are counted.
    #[test]
    fn the_credentials_named_are_keychain_references_first_in_order() {
        let creds: Vec<String> = [
            "Z from env:Z_VAR",
            "A from env:A_VAR",
            "T4 from keychain:d",
            "T1 from keychain:a",
            "bearer from keychain:c",
            "header X-Key from keychain:b",
        ]
        .map(String::from)
        .to_vec();
        let reason = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &creds,
            "sends",
            "GET https://api.example.com",
            "",
        ));
        assert!(
            reason.ends_with(
                ", with the credentials T1 from keychain:a, header X-Key from keychain:b, bearer from keychain:c, T4 from keychain:d, +2 more"
            ),
            "{reason}"
        );
        let few: Vec<String> = ["Z from env:Z_VAR", "A from env:A_VAR", "T from keychain:t"]
            .map(String::from)
            .to_vec();
        let reason = approval_reason(&dialog("AAAA-BBBB-CCCC", &few, "runs", "/usr/bin/git", ""));
        assert!(
            reason.ends_with(
                ", with the credentials T from keychain:t, A from env:A_VAR, Z from env:Z_VAR"
            ),
            "{reason}"
        );
    }

    /// However many credentials a tool names, and however long, the program
    /// or host comes first, whole, and only a few are named, each cut.
    #[test]
    fn many_long_credentials_come_after_the_program_and_are_bounded() {
        let creds: Vec<String> = (0..7)
            .map(|i| format!("C{i} from keychain:{}", "k".repeat(200)))
            .collect();
        let reason = approval_reason(&dialog(
            "AAAA-BBBB-CCCC",
            &creds,
            "runs",
            "/usr/bin/git",
            "",
        ));
        assert!(
            reason.starts_with(
                "approve code AAAA-BBBB-CCCC for the remote tool \u{201C}lookup\u{201D}, which runs /usr/bin/git, with the credentials C0 from keychain:"
            ),
            "{reason}"
        );
        assert!(reason.contains("C3 from keychain:"), "{reason}");
        assert!(!reason.contains("C4 from"), "{reason}");
        assert!(reason.ends_with(", +3 more"), "{reason}");
        assert!(
            !reason.contains(&"k".repeat(MAX_DIALOG_CREDENTIAL)),
            "{reason}"
        );
    }
}
