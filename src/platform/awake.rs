//! Keeping the computer (and its screens) awake during a show. A show run
//! only from a Stream Deck or the phone remote gives the computer no
//! keyboard or mouse, so without this a laptop could sleep mid-take and the
//! second screen could go dark.

/// Hold (or let go of) the "don't sleep" request. Cheap to call again with
/// the same value. Call it from one thread (the window's).
pub fn keep_awake(on: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ON: AtomicBool = AtomicBool::new(false);
    if ON.swap(on, Ordering::Relaxed) != on {
        imp::set(on);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::CFString;
    use std::sync::Mutex;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(kind: *const CFString, level: u32, name: *const CFString, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    /// kIOPMAssertionLevelOn.
    const ON: u32 = 255;
    static HELD: Mutex<Option<u32>> = Mutex::new(None);

    pub fn set(on: bool) {
        let Ok(mut held) = HELD.lock() else { return };
        match (on, *held) {
            (true, None) => {
                // Keeping the display awake keeps the system awake too.
                let kind = CFString::from_static_str("PreventUserIdleDisplaySleep");
                let name = CFString::from_static_str("Cutaway is recording or live");
                let mut id = 0u32;
                if unsafe { IOPMAssertionCreateWithName(&*kind, ON, &*name, &mut id) } == 0 {
                    *held = Some(id);
                } else {
                    crate::logging::note("couldn't ask macOS to stay awake during the show");
                }
            }
            (false, Some(id)) => {
                unsafe { IOPMAssertionRelease(id) };
                *held = None;
            }
            _ => {}
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};

    pub fn set(on: bool) {
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        if unsafe { SetThreadExecutionState(flags) }.0 == 0 {
            crate::logging::note("couldn't ask Windows to stay awake during the show");
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn set(_on: bool) {}
}
