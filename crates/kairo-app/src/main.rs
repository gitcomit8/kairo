//! Kairo Phase 0 harness.
//!
//! Opens a window, presents through a flip-model swapchain at frame latency 1, and reports
//! the cold-start timeline and frame pacing. This binary exists to establish the measurement
//! baseline that every later performance claim is checked against; it is not yet a browser.
//!
//! It therefore keeps the console subsystem attached in release builds too: the report is
//! the deliverable. The shipping browser binary will switch to the Windows subsystem.

use std::time::{Duration, Instant};

use kairo_gfx::Gfx;
use kairo_metrics::{FrameStats, StartupTimeline};
use kairo_platform_win::{Window, display, dpi};
use windows::core::Result;

const TITLE: &str = "Kairo";
const INITIAL_WIDTH: u32 = 1280;
const INITIAL_HEIGHT: u32 = 800;

/// Near-black with a slight blue cast: what the compositor clears to before any content.
const BACKGROUND: [f32; 4] = [0.04, 0.05, 0.07, 1.0];

fn main() -> Result<()> {
    let mut timeline = StartupTimeline::new();
    timeline.mark("process entry");

    // Must precede window creation. A prior, stronger context (e.g. an app manifest) is a
    // success for our purposes, so the result is deliberately not fatal.
    let _ = dpi::enable_per_monitor_v2();

    let mut window = Window::create(TITLE, INITIAL_WIDTH, INITIAL_HEIGHT)?;
    window.set_dark_titlebar(true);
    timeline.mark("window created");

    // The debug layer costs frames, so it is opt-in even in debug builds.
    let debug_layer = std::env::var_os("KAIRO_GPU_DEBUG").is_some();
    let (width, height) = window.client_size();
    let mut gfx = Gfx::new(window.hwnd(), width, height, debug_layer)?;
    timeline.mark("device + swapchain");

    // Render the first frame while the window is still hidden, then reveal it: the first
    // visible pixel is the first presented frame, with no white flash in between.
    gfx.render(BACKGROUND)?;
    window.show();
    timeline.mark("first frame presented");

    println!("kairo {}", env!("CARGO_PKG_VERSION"));
    println!("adapter: {}", gfx.adapter_name());
    println!("swapchain: {width}x{height}, flip discard, frame latency 1");
    print!("{}", timeline.report());

    let refresh_hz = display::primary_refresh_hz();
    let mut stats = FrameStats::for_refresh_hz(refresh_hz);
    let started = Instant::now();
    let mut last_frame = started;
    let mut last_report = started;

    let exit_code = window.run_frame_loop(gfx.waitable(), |window| {
        let now = Instant::now();
        stats.push(now.duration_since(last_frame));
        last_frame = now;

        // Only touch the swapchain when there is a real client area to match.
        if !window.is_minimized() {
            let (client_w, client_h) = window.client_size();
            if let Err(err) = gfx.resize(client_w, client_h) {
                eprintln!("resize failed: {err}");
                return false;
            }
            if let Err(err) = gfx.render(pulse(started.elapsed())) {
                eprintln!("render failed: {err}");
                return false;
            }
        }

        if now.duration_since(last_report) >= Duration::from_secs(2) {
            last_report = now;
            println!("{}", stats.report());
        }
        true
    });

    println!("{}", stats.report());
    let _ = gfx.wait_idle();

    // Exit explicitly: the frame loop already returned the definitive code, and unwinding the
    // pipeline here would only add shutdown time to a measurement-oriented binary.
    std::process::exit(exit_code);
}

/// Slow colour drift so it is visually obvious that frames are actually being presented
/// (a static clear colour also proves nothing about whether the loop is running).
fn pulse(elapsed: Duration) -> [f32; 4] {
    let t = elapsed.as_secs_f32();
    [
        BACKGROUND[0] + 0.06 * (t * 0.7).sin(),
        BACKGROUND[1] + 0.06 * (t * 0.9 + 2.0).sin(),
        BACKGROUND[2] + 0.06 * (t * 1.1 + 4.0).sin(),
        1.0,
    ]
}
