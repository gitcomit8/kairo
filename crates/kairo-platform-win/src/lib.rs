//! Windows platform layer: windowing, DPI, and the frame-aligned message pump.
//!
//! This crate intentionally does **not** abstract the event loop behind a portable
//! interface. Kairo's performance thesis depends on owning the exact relationship between
//! Win32 input messages, the DXGI frame-latency waitable object, and the present call, and
//! every portable windowing abstraction we could adopt sits on that path.

pub mod display;
pub mod dpi;
pub mod window;

pub use window::Window;
