//! Whether the user's screen is locked.
//!
//! On a locked macOS session WebKit throttles or suspends webviews, so pages
//! answer late or not at all while the app process and its connector socket
//! stay alive. Errors and health reports name the lock instead of implying
//! the app is dead.

use std::cell::Cell;

/// Stable reason and error code for a command blocked by a locked screen.
pub const SCREEN_LOCKED: &str = "screen_locked";

/// Advice appended to errors observed while the screen is locked.
pub const SCREEN_LOCKED_NOTE: &str = "screen_locked: the screen is locked, so macOS throttles or suspends the app's webview; the app is alive but its page answers late or not at all. Unlock the screen and retry";

thread_local! {
    static OVERRIDE: Cell<Option<Option<bool>>> = const { Cell::new(None) };
}

/// Test hook (this thread only): force [`screen_locked`]'s answer, or `None`
/// to read the real session again.
#[doc(hidden)]
pub fn force_screen_locked(value: Option<Option<bool>>) {
    OVERRIDE.with(|cell| cell.set(value));
}

/// `Some(true)` when the console session's screen is locked, `Some(false)`
/// when unlocked, `None` when unknown (no GUI session, or not macOS).
pub fn screen_locked() -> Option<bool> {
    if let Some(forced) = OVERRIDE.with(Cell::get) {
        return forced;
    }
    platform::screen_locked()
}

/// `SCREEN_LOCKED_NOTE` when the screen is known to be locked.
pub fn locked_note() -> Option<&'static str> {
    (screen_locked() == Some(true)).then_some(SCREEN_LOCKED_NOTE)
}

/// Run a read-only capture only while the desktop is unlocked. Check before
/// polling it, during suspended native/page work, and before returning pixels.
/// Do not use this for writes: cancellation cannot establish their outcome.
pub async fn while_unlocked<T>(
    operation: impl std::future::Future<Output = T>,
) -> Result<T, &'static str> {
    if let Some(note) = locked_note() {
        return Err(note);
    }
    let locked = async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Some(note) = locked_note() {
                return note;
            }
        }
    };
    tokio::select! {
        biased;
        note = locked => Err(note),
        result = operation => match locked_note() {
            Some(note) => Err(note),
            None => Ok(result),
        },
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void};

    type CFTypeRef = *const c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: CFTypeRef,
            s: *const c_char,
            encoding: u32,
        ) -> CFTypeRef;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFGetTypeID(value: CFTypeRef) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: CFTypeRef) -> u8;
        fn CFRelease(value: CFTypeRef);
    }
    const UTF8: u32 = 0x0800_0100;

    pub fn screen_locked() -> Option<bool> {
        // SAFETY: Core Foundation create/copy results are released once;
        // `CFDictionaryGetValue` follows the get rule (not owned).
        unsafe {
            let session = CGSessionCopyCurrentDictionary();
            if session.is_null() {
                return None;
            }
            let key = CFStringCreateWithCString(
                std::ptr::null(),
                c"CGSSessionScreenIsLocked".as_ptr(),
                UTF8,
            );
            let locked = if key.is_null() {
                None
            } else {
                let value = CFDictionaryGetValue(session, key);
                CFRelease(key);
                // The key is absent while the session is unlocked.
                Some(
                    !value.is_null()
                        && CFGetTypeID(value) == CFBooleanGetTypeID()
                        && CFBooleanGetValue(value) != 0,
                )
            };
            CFRelease(session);
            locked
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn screen_locked() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_drives_the_note() {
        force_screen_locked(Some(Some(true)));
        assert_eq!(locked_note(), Some(SCREEN_LOCKED_NOTE));
        force_screen_locked(Some(Some(false)));
        assert_eq!(locked_note(), None);
        force_screen_locked(Some(None));
        assert_eq!(locked_note(), None);
        force_screen_locked(None);
    }

    #[tokio::test]
    async fn capture_is_not_polled_when_locked_and_cannot_return_late_pixels() {
        let polled = Cell::new(false);
        force_screen_locked(Some(Some(true)));
        let refused = while_unlocked(async {
            polled.set(true);
            "pixels"
        })
        .await;
        force_screen_locked(None);
        assert_eq!(refused, Err(SCREEN_LOCKED_NOTE));
        assert!(!polled.get());

        force_screen_locked(Some(Some(false)));
        let late = while_unlocked(async {
            force_screen_locked(Some(Some(true)));
            "solid frame"
        })
        .await;
        force_screen_locked(None);
        assert_eq!(late, Err(SCREEN_LOCKED_NOTE));

        force_screen_locked(Some(Some(false)));
        let unlocked = while_unlocked(async { "real pixels" }).await;
        force_screen_locked(None);
        assert_eq!(unlocked, Ok("real pixels"));
    }

    #[test]
    fn the_real_session_can_be_read_without_crashing() {
        // Unlocked in CI and dev sessions; `None` without a GUI session.
        let _ = screen_locked();
    }
}
