# Kairo

A Windows-only web browser written in Rust, built around measured speed: cold start,
input-to-photon latency, tab switching, memory per tab, and energy.

**Status: Phase 0.** There is no browser yet — there is a window that presents through a
D3D12 flip-model swapchain at frame latency 1, plus the startup and frame-pacing
instrumentation that every later performance claim will be checked against.

## Requirements

- Windows 10 1809+ or Windows 11, x64
- Rust stable (pinned in `rust-toolchain.toml`) with the MSVC toolchain
- A GPU with Direct3D 12 support

## Build and run

```sh
cargo run -p kairo-app --release
```

The window is intentionally hidden until the first frame is presented, so the first pixel
you see is a real frame. `Esc` or closing the window exits; an exit summary prints the
cold-start timeline and frame statistics.

Set `KAIRO_GPU_DEBUG=1` to enable the D3D12 debug layer. Do not measure performance with it
enabled — validation costs frames.

## Layout

| Crate | Responsibility |
| --- | --- |
| `kairo-app` | Executable, startup sequence, the Phase 0 report |
| `kairo-platform-win` | Win32 window, DPI, frame-aligned message pump |
| `kairo-gfx` | D3D12 device, flip-model swapchain, frame submission |
| `kairo-metrics` | Startup timeline and frame-pacing statistics (platform-independent, tested) |

## Tests

```sh
cargo test -p kairo-metrics
cargo check --workspace --target x86_64-pc-windows-msvc
```

## Documentation

- [`docs/PLAN.md`](docs/PLAN.md) — architecture, engine strategy, Windows integration, phase plan
