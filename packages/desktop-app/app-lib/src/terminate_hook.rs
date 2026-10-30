//! Routes a quit that AppKit delivers as `-[NSApplication terminate:]` (Dock
//! icon Quit, Cmd+Tab then Q, AppleScript quit, logout/restart/shutdown)
//! through the app's own quit path.
//!
//! The windowing layer's application delegate implements only
//! `applicationWillTerminate:`, so such a quit goes straight to exit and the
//! webview never gets to flush its pending edits. This adds
//! `applicationShouldTerminate:` to the delegate's class: it answers
//! `NSTerminateLater` and starts the same close path the Quit menu item uses
//! (window close -> frontend flush -> destroy -> exit). That path ends the
//! process itself, which also satisfies the deferred terminate. With no window
//! open there is nothing to flush or close, so it answers `NSTerminateNow` and
//! lets AppKit run the normal exit.

use objc2::ffi;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use std::sync::{Arc, Mutex};

/// `NSApplicationTerminateReply::NSTerminateNow`: AppKit goes on to terminate.
const NS_TERMINATE_NOW: usize = 1;
/// `NSApplicationTerminateReply::NSTerminateLater`: the app will finish
/// quitting on its own.
const NS_TERMINATE_LATER: usize = 2;

/// Starts the app's quit path. Returns whether a quit is now under way, which
/// is false when there was nothing to start it on (no window open).
type QuitHook = Arc<dyn Fn() -> bool + Send + Sync>;

static QUIT_HOOK: Mutex<Option<QuitHook>> = Mutex::new(None);

/// The reply to a terminate request. `NSTerminateLater` is only safe when the
/// quit path was actually started, because that path is what ends the process;
/// in every other case AppKit is told to terminate itself.
fn terminate_reply() -> usize {
    // The lock is released before the hook runs, so a hook may itself install
    // or read the slot.
    let hook = QUIT_HOOK.lock().ok().and_then(|slot| slot.clone());
    match hook {
        Some(hook) if hook() => NS_TERMINATE_LATER,
        _ => NS_TERMINATE_NOW,
    }
}

extern "C-unwind" fn should_terminate(
    _this: &AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> usize {
    terminate_reply()
}

/// Stores `hook`, then calls `add_method`; the previous hook is put back when
/// the method was not added. The hook is stored first so the method can never
/// be live without one.
fn store_hook_then_add(hook: QuitHook, add_method: impl FnOnce() -> bool) -> bool {
    let previous = QUIT_HOOK
        .lock()
        .ok()
        .and_then(|mut slot| slot.replace(hook));
    let added = add_method();
    if !added {
        if let Ok(mut slot) = QUIT_HOOK.lock() {
            *slot = previous;
        }
    }
    added
}

/// Adds `applicationShouldTerminate:` to `class` and stores `hook` as what it
/// runs. Returns false when the class already implements the selector, so an
/// existing handler is never replaced.
pub(crate) fn install_on_class(class: &AnyClass, hook: QuitHook) -> bool {
    let types = c"Q@:@";
    store_hook_then_add(hook, || {
        // SAFETY: the implementation matches the method's `Q(id, SEL, id)`
        // signature, and the class is a live, registered class.
        unsafe {
            let imp: Imp = std::mem::transmute::<
                extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize,
                Imp,
            >(should_terminate);
            ffi::class_addMethod(
                class as *const AnyClass as *mut _,
                sel!(applicationShouldTerminate:),
                imp,
                types.as_ptr(),
            )
            .as_bool()
        }
    })
}

/// Installs the handler on the running application's delegate. Must run on the
/// main thread, after the delegate is set (the first run-loop event).
pub(crate) fn install(hook: QuitHook) {
    let Some(app_class) = AnyClass::get(c"NSApplication") else {
        return;
    };
    // SAFETY: `sharedApplication` and `delegate` are plain getters, called on
    // the main thread.
    let installed = unsafe {
        let app: *mut AnyObject = msg_send![app_class, sharedApplication];
        if app.is_null() {
            return;
        }
        let delegate: *mut AnyObject = msg_send![app, delegate];
        if delegate.is_null() {
            return;
        }
        let added = install_on_class((*delegate).class(), hook);
        if added {
            // AppKit decides which delegate methods exist when the delegate is
            // assigned; assigning it again makes it see the new one.
            let _: () = msg_send![app, setDelegate: delegate];
        }
        added
    };
    if !installed {
        tracing::warn!(
            "applicationShouldTerminate: not installed; Dock and logout quits skip the flush"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::runtime::ClassBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The hook slot is process-wide, so tests that touch it take this lock.
    static SLOT_TESTS: Mutex<()> = Mutex::new(());

    fn set_hook(hook: Option<QuitHook>) {
        *QUIT_HOOK.lock().unwrap() = hook;
    }

    #[test]
    fn a_terminate_request_runs_the_hook_and_defers_termination() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let builder = ClassBuilder::new(c"TerminateHookTestDelegate", objc2::class!(NSObject))
            .expect("class");
        let class = builder.register();
        set_hook(None);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        assert!(install_on_class(
            class,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            })
        ));

        let object: *mut AnyObject = unsafe { msg_send![class, new] };
        let reply: usize = unsafe {
            msg_send![object, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };
        assert_eq!(reply, NS_TERMINATE_LATER);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A class that already answers the selector is left alone, and the
        // hook that was in place stays in place.
        assert!(!install_on_class(class, Arc::new(|| false)));
        let reply: usize = unsafe {
            msg_send![object, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };
        assert_eq!(reply, NS_TERMINATE_LATER);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn without_a_hook_termination_is_not_deferred() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(None);
        assert_eq!(terminate_reply(), NS_TERMINATE_NOW);
    }

    #[test]
    fn a_poisoned_slot_does_not_defer_termination() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(Some(Arc::new(|| true)));
        let _ = std::thread::spawn(|| {
            let _slot = QUIT_HOOK.lock().unwrap();
            panic!("poison the slot");
        })
        .join();
        assert_eq!(terminate_reply(), NS_TERMINATE_NOW);
        QUIT_HOOK.clear_poison();
        set_hook(None);
    }

    #[test]
    fn a_hook_that_started_no_quit_does_not_defer_termination() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(Some(Arc::new(|| false)));
        assert_eq!(terminate_reply(), NS_TERMINATE_NOW);
        set_hook(None);
    }

    #[test]
    fn the_slot_is_not_locked_while_the_hook_runs() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(Some(Arc::new(|| QUIT_HOOK.try_lock().is_ok())));
        assert_eq!(terminate_reply(), NS_TERMINATE_LATER);
        set_hook(None);
    }

    #[test]
    fn the_hook_is_stored_before_the_method_is_added() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(None);
        let stored_when_added =
            store_hook_then_add(Arc::new(|| true), || QUIT_HOOK.lock().unwrap().is_some());
        assert!(stored_when_added);
        set_hook(None);
    }

    #[test]
    fn a_failed_add_restores_the_previous_hook() {
        let _guard = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        set_hook(None);
        assert!(!store_hook_then_add(Arc::new(|| true), || false));
        assert!(QUIT_HOOK.lock().unwrap().is_none());
    }
}
