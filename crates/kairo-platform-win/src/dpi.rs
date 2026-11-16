//! DPI awareness.
//!
//! Awareness must be set before the first window exists, and setting it twice fails. Both
//! failure modes are benign here: an already-set context means the process (or an application
//! manifest) got there first, which is the state we want anyway.

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::Result;

/// Opt the process into per-monitor DPI awareness v2.
///
/// Returns `Ok(true)` when this call was the one that set the context, `Ok(false)` when the
/// process was already per-monitor-v2 (for example via manifest). An `Err` means the process
/// had previously opted into a *weaker* context, which cannot be upgraded at runtime.
///
/// Call this before creating any window or swapchain.
pub fn enable_per_monitor_v2() -> Result<bool> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .map(|()| true)
}
