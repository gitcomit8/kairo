//! A Win32 window and the frame-aligned message pump.
//!
//! The pump is the point of this module. A conventional render loop either blocks in
//! `GetMessageW` (input-only wakeups, frames rendered on a timer) or spins on a timer
//! (input latency equal to the timer granularity). Kairo waits on the DXGI frame-latency
//! waitable object *and* the message queue at once via `MsgWaitForMultipleObjectsEx`, so a
//! frame begins the instant the swapchain can accept one, and input never waits behind a
//! sleep.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    GetLastError, HANDLE, HWND, LPARAM, LRESULT, RECT, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, IDC_ARROW, IsIconic, LoadCursorW, MSG,
    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT, RegisterClassW, SW_SHOW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_QUIT,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{Error, HRESULT, HSTRING, PCWSTR, Result, w};

const CLASS_NAME: PCWSTR = w!("KairoWindow");

/// Window handle wrapper.
///
/// The window is created hidden so the first presented frame is also the first visible one;
/// call [`Window::show`] once a frame has been rendered. That removes the white flash which
/// otherwise reveals a browser's real startup time to the user.
pub struct Window {
    hwnd: HWND,
}

// The HWND is only ever touched from the thread that created it; Win32 windows are bound to
// their owning thread, and this type does not implement Send/Sync across that boundary.
impl Window {
    /// Create the main window. `width`/`height` are the initial window size in physical
    /// pixels; use [`Window::client_size`] for the area the swapchain must match.
    pub fn create(title: &str, width: u32, height: u32) -> Result<Self> {
        static REGISTERED: AtomicBool = AtomicBool::new(false);

        let instance = unsafe { GetModuleHandleW(None)? };

        if !REGISTERED.swap(true, Ordering::SeqCst) {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if unsafe { RegisterClassW(&wc) } == 0 {
                return Err(last_error());
            }
        }

        let title = HSTRING::from(title);
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                &title,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }?;

        Ok(Self { hwnd })
    }

    /// The raw window handle, for DXGI swapchain creation.
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Client area in physical pixels. `(0, 0)` while minimised.
    pub fn client_size(&self) -> (u32, u32) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rect) }.is_err() {
            return (0, 0);
        }
        (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    }

    /// Whether the window is minimised, in which case rendering should be suspended.
    pub fn is_minimized(&self) -> bool {
        unsafe { IsIconic(self.hwnd) }.as_bool()
    }

    /// Make the window visible. Call after the first frame is presented.
    pub fn show(&self) {
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOW) };
    }

    /// Use the dark system title bar (Windows 10 1809+). Ignored on failure.
    pub fn set_dark_titlebar(&self, dark: bool) {
        let value: u32 = u32::from(dark);
        unsafe {
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &value as *const u32 as *const core::ffi::c_void,
                size_of::<u32>() as u32,
            );
        }
    }

    /// Run the frame loop until the window closes.
    ///
    /// `latency_waitable` is the handle from `IDXGISwapChain2::SetFrameLatencyWaitableObject`.
    /// `on_frame` is called once per presentable frame and returns `false` to exit.
    /// Returns the process exit code.
    pub fn run_frame_loop(
        &mut self,
        latency_waitable: HANDLE,
        mut on_frame: impl FnMut(&Window) -> bool,
    ) -> i32 {
        loop {
            // Drain pending messages first: never present a frame while input is queued.
            loop {
                let mut msg = MSG::default();
                if !unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    break;
                }
                if msg.message == WM_QUIT {
                    return msg.wParam.0 as i32;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            // Block until the swapchain can take another frame, or input arrives.
            let handles = [latency_waitable];
            let wait = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&handles),
                    INFINITE,
                    QS_ALLINPUT,
                    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(MWMO_INPUTAVAILABLE.0),
                )
            };

            if wait == WAIT_OBJECT_0 {
                if !on_frame(self) {
                    return 0;
                }
            } else if wait == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                // Input pending; drain it before rendering the next frame.
                continue;
            } else if wait == WAIT_FAILED {
                return -1;
            }
        }
    }
}

/// Convert the thread's last Win32 error into an `Error`.
///
/// This is `HRESULT_FROM_WIN32` from `winerror.h`: system codes below 0x8000_0000 are
/// already HRESULT-shaped and pass through, everything else is offset into the FACILITY_WIN32
/// range.
fn last_error() -> Error {
    let code = unsafe { GetLastError() }.0;
    let hr = if (code as i32) < 0 {
        code as i32
    } else {
        (0x8007_0000u32 | code) as i32
    };
    Error::from(HRESULT(hr))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // Exit through the normal destroy path so the loop can report a real exit code.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // We present with D3D; GDI must never paint the background.
        WM_ERASEBKGND => LRESULT(1),
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
