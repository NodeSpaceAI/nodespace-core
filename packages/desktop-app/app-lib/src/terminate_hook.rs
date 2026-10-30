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
//! process itself, which also satisfies the deferred terminate.

use objc2::ffi;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use std::sync::Mutex;

/// `NSApplicationTerminateReply::NSTerminateLater`: the app will finish
/// quitting on its own.
const NS_TERMINATE_LATER: usize = 2;

type QuitHook = Box<dyn Fn() + Send + Sync>;

static QUIT_HOOK: Mutex<Option<QuitHook>> = Mutex::new(None);

extern "C-unwind" fn should_terminate(
    _this: &AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> usize {
    if let Ok(hook) = QUIT_HOOK.lock() {
        if let Some(hook) = hook.as_ref() {
            hook();
        }
    }
    NS_TERMINATE_LATER
}

/// Adds `applicationShouldTerminate:` to `class` and stores `hook` as what it
/// runs. Returns false when the class already implements the selector, so an
/// existing handler is never replaced.
pub(crate) fn install_on_class(class: &AnyClass, hook: QuitHook) -> bool {
    let types = c"Q@:@";
    // SAFETY: the implementation matches the method's `Q(id, SEL, id)`
    // signature, and the class is a live, registered class.
    let added = unsafe {
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
    };
    if added {
        if let Ok(mut slot) = QUIT_HOOK.lock() {
            *slot = Some(hook);
        }
    }
    added
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
    use std::sync::Arc;

    // The hook slot is process-wide, so the tests that use it run in one body.
    #[test]
    fn a_terminate_request_runs_the_hook_and_defers_termination() {
        let builder = ClassBuilder::new(c"TerminateHookTestDelegate", objc2::class!(NSObject))
            .expect("class");
        let class = builder.register();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        assert!(install_on_class(
            class,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        ));

        let object: *mut AnyObject = unsafe { msg_send![class, new] };
        let reply: usize = unsafe {
            msg_send![object, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };

        assert_eq!(reply, NS_TERMINATE_LATER);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A class that already answers the selector is left alone.
        assert!(!install_on_class(class, Box::new(|| {})));
        let _: usize = unsafe {
            msg_send![object, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };
        assert_eq!(calls.load(Ordering::SeqCst), 2, "original hook still wired");
    }
}
