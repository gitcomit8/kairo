# Kairo — Architecture &amp; Build Plan

A Windows-first, Rust-implemented web browser whose differentiator is **measured speed**:
cold-start time, input-to-photon latency, tab-switch time, memory per tab, and energy.

Status: planning. No code yet.

---

## 1. Reality check first

Three premises need correcting before any architecture decision, because they change what
"extremely fast" can mean.

**1.1 Safari is not fast because Apple owns the OS.**
Safari's wins come mostly from WebKit/JSC tuning, a smaller process footprint, and
compositing straight into CoreAnimation. Chrome beats Safari on many JS benchmarks on the
same Mac. Ownership gives Apple two real but *narrow* advantages: a GPU/compositor API it
controls end to end, and lower-level access to memory-pressure and energy signals.

The good news: Windows exposes equivalents for nearly all of that — DirectComposition, DXGI
flip-model swapchains with frame-latency waitable objects, DirectWrite, ETW, memory-priority
and EcoQoS process APIs, MMCSS. A Windows-only browser can legitimately claw back that
"OS-integrated" margin. That is the honest version of your thesis, and it's worth building.

**1.2 Speed is dominated by the engine, not the shell.**
Every URL you type ends up in a JS engine, a layout engine, and a rasterizer. A small team
will not out-JIT V8 or out-layout Blink. If Kairo's claim is "highest Speedometer score,"
the project is not winnable.

What *is* winnable, and what users actually feel:

| Metric | Chrome/Edge today | What it's limited by | Can a small team beat it? |
| --- | --- | --- | --- |
| Cold start → first paint | ~1–2 s | shell init, profile load, extensions | **Yes, big margin** |
| Tab switch | 50–300 ms | re-raster / process wake | **Yes** |
| Input-to-photon (scroll, typing) | ~1–2 frames @60Hz | compositor path, IPC | **Yes, marginal but real** |
| Memory per tab | 100–300 MB | engine heap, process model | **Yes** |
| Energy / battery | — | background work, timers | **Yes** |
| JS throughput (Speedometer 3, JetStream) | — | the engine | **Only by matching a top engine** |

So the goal is: **"the fastest-feeling browser on Windows"** — define it as
*time-to-interactive on the top 10k sites, plus p99 input latency, plus RSS per tab* — and
treat JS benchmark parity as the ceiling, not the target.

**1.3 "Written in Rust" and "site-compatible" are in tension.**
That tension is the single decision that shapes everything else (§2).

---

## 2. The engine decision (decide this before writing code)

There are only three real options.

### A. WebView2 (Microsoft's Chromium, shipped with Windows)
- Crates: `webview2-com` (Microsoft-maintained). Supports the **composition controller**
  mode, which lets you host the web content inside your own DirectComposition visual tree.
- **For:** fastest path to a working browser by months. Site compatibility ≈ Edge. DRM,
  extensions, media all work. Zero engine maintenance.
- **Against:** you inherit Edge's engine version and process model, cannot tune the
  scheduler or heap, and can never beat Edge on engine speed. You are building an Edge
  shell — which is exactly what Opera/Brave-style products are. "Fastest browser" claims
  become very hard to defend.

### B. Embarcadero-free Chromium via CEF (or a forked Chromium)
- **For:** full control of process model, flags, features, memory policy, V8 code cache and
  snapshots. This is the only way to be "the fastest *Chromium* on Windows."
- **Against:** ~150 MB+ binary, a C++ build pipeline you must maintain, months of
  integration, and you still don't own the engine.

### C. Servo (`servo` crate) — the only genuinely Rust browser
- Servo has been on crates.io since **0.1.0 (Apr 2026)** and is at **0.7.x (Nov 2026)**, with
  a real embedding API: `ServoBuilder`, `WebView`, `WebViewDelegate`, `RenderingContext`,
  multi-process support, WebGL in `default_web_features`.
- **For:** you own the process model, compositor, network stack, and every Windows-level
  tuning knob end to end. This is the only path where the OS-integration thesis is fully
  realizable, and the only one where "built in Rust" is more than marketing.
- **Against:** compatibility. Expect meaningful breakage on real sites — missing Web APIs,
  weak media, no Widevine (no Netflix/Spotify), no extension ecosystem, no IME polish. The
  Verso webview experiment (Servo + Tauri) was **archived in Oct 2025** because a small team
  couldn't keep pace; Servo's own advice was that `servoshell` was the better UX. Read that
  as a warning about scope, not a verdict on the engine.

### Recommendation: hybrid, engine behind a trait

```
kairo-engine-api    trait EngineHost { create_view, navigate, resize, input, present, events }
   ├── kairo-engine-webview2   (compat / fallback / v0)
   └── kairo-engine-servo      (primary, Rust-native, Windows-tuned)
```

Build the shell and the **compositor** first with WebView2 attached, so you have a usable
browser in ~10 weeks. Then swap Servo in as the default engine and keep WebView2 as a
per-site fallback (a compat-triage heuristic: if a page throws, times out, or fails a smoke
test, reload it in the compat engine).

This buys a shippable product early and keeps the differentiator alive. It also means the
shell — where nearly all the speed wins live — is engine-agnostic and worth building well.

**Spike first:** before Phase 3, embed Servo 0.7 into a Win32 window and render and scroll
representative pages in it. This validates the engine choice before the rest of the shell is
built on the assumption that it works.

---

## 3. Do NOT use GPUI for the content surface

You asked about GPUI. Verdict:

- **For the browser chrome (tabs, omnibox, bookmarks, settings): plausible.**
- **For web content or the compositor: no.** GPUI owns windowing, layout, and present; you
  need a single compositor that mixes your UI quads with *engine-supplied textures* at
  frame-latency-1. GPUI doesn't hand you that contract.

And the risk profile as of late 2026 is unattractive for a multi-year product:

- Still pre-1.0 (`gpui 0.2.2` on crates.io), explicitly "breaking changes between versions."
- Windows backend is **DirectX 11** via raw Win32 — not where you want to be for flip-model
  low-latency presentation.
- Zed **paused GPUI development for non-Zed use cases** in 2026; the community fork
  `gpui-ce` has had little merged activity. You would be betting a browser on a framework
  whose maintainer has deprioritized external users.

Alternatives for the chrome layer, in order of pragmatism:
1. **Raw Win32 + your own D3D12 UI renderer** (what Chrome/Views does). Most control, most work.
2. **Slint** — already has a *working* Servo integration on Windows with hardware rendering
   (see §5.4). Fast iteration, and a reference implementation to copy from.
3. **`taffy` + `parley`/`cosmic-text` + your D3D12 quad renderer** — middle path: real layout
   and shaping, you own pixels.

If you prototype with GPUI, put it behind a `kairo-ui` trait so it is replaceable. Do not let
it into the compositor.

---

## 4. Windows deep-integration plan

This is the section that justifies "Windows-only." Each item is a concrete API surface.

### 4.1 Windowing &amp; composition
- Win32 directly via `windows` crate (not winit) — you need `WM_GETMINMAXINFO` (snap layouts),
  `WM_NCCALCSIZE` (custom frame), `WM_POINTER` (pen/touch), `WM_DWMCOMPOSITIONCHANGED`.
- **DirectComposition** (`IDCompositionDevice3` + visuals) for the window tree; per-monitor DPI v2.
- **DXGI flip-model swapchain** with `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` and
  `SetMaximumFrameLatency(1)`. This is the whole low-latency story: never `Present(1)` a blt-model chain.
- DWM system backdrop (Mica/acrylic) for chrome; Windows 11 rounded corners + dark mode via
  `DwmSetWindowAttribute`.
- Multi-monitor + mixed refresh rate: composite per-monitor, don't force a global vsync.

### 4.2 Graphics
- One **D3D12** device in the browser process (D3D11 fallback, WARP last resort), Agility SDK
  for current features.
- Content from renderer processes arrives as **shared NT-handle textures**
  (`IDXGIResource1::CreateSharedHandle` + keyed mutex, or `D3D11on12`).
- Damage tracking + dirty-rect presents; never re-present an unchanged frame.
- Own shader path for UI quads, rounded rects, blur, and glyph atlases.

### 4.3 Text &amp; input
- **DirectWrite** (`IDWriteFactory5`/`DWriteCore`) for layout/shaping, variable fonts, and
  system font fallback — the browser should get Windows' text rendering for free.
- Glyph atlas cached per (font, size, DPI, subpixel mode); rasterize off the UI thread.
- Scroll must run on the **compositor thread** with an offset owned there (like Chrome's
  compositor-thread scrolling) to avoid a round trip on the first frame of every scroll.
- IME via `ITfThreadMgr` / `WM_IME_*` — this is a real project, budget for it.
- Accessibility: UI Automation provider per content view (`UiaHostProviderFromHwnd` +
  raw element provider). Non-negotiable if you want enterprise/education use.

### 4.4 Process model &amp; sandbox
```
browser.exe   (UI thread, compositor, network, session)
  └─ renderer.exe × N   (one per site instance, AppContainer, low IL, job object)
  └─ gpu.exe            (optional; driver-crash isolation)
  └─ utility.exe        (storage, audio w/ MMCSS, media)
```
- **Zygote-style pre-warm:** at login (Task Scheduler) or on app start, spawn 1–2 paused
  renderer processes with the DLLs/ICU/JS engine already mapped; `NtCreateProcessEx`-style
  fork-on-Windows via `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` is limited, so prefer "pre-load,
  then adopt." This is the single biggest cold-start lever.
- Sandbox: restricted token (`CreateRestrictedToken`), low integrity, AppContainer SID,
  `JobObjectBasicUIRestrictions` + memory/CPU limits.
- Kill and respawn renderers on hang; tab crash isolation with a reload affordance.

### 4.5 Memory &amp; power (the user-visible wins)
- `SetProcessInformation(ProcessMemoryPriority)` per renderer; background tabs get
  `MemoryPriorityBackground`.
- EcoQoS: `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` on hidden tabs; stop `requestAnimationFrame`
  and timer coalescing aggressively.
- **Tab discard**: serialize a tab to a memory-mapped file, `DiscardVirtualMemory` the live
  heap, restore with `PrefetchVirtualMemory` on switch. Target p50 tab switch &lt; 16 ms.
- Per-tab memory budget with a visible counter; users trust a browser that shows its numbers.

### 4.6 Threads &amp; scheduling
- `AvSetMmThreadCharacteristics("Audio")` for the media/audio path, `"Games"` for compositor.
- CPU sets (`SetThreadSelectedCpuSets`) to keep the compositor off cores running JS.
- Thread priority raise on input (`WM_MOUSEMOVE`/key events) to shorten the critical path.
- **IOCP** (`GetQueuedCompletionStatusEx`) with `SetFileCompletionNotificationModes(FILE_SKIP_COMPLETION_PORT_ON_SUCCESS)`
  for file I/O; `SIO_LOOPBACK_FAST_PATH` for local servers.

### 4.7 OS integration (the "feels native" layer)
Default-browser registration (`RegisteredApplications` + ProgID), jump lists
(`ICustomDestinationList`), taskbar thumbnails/toolbars (`ITaskbarList3`), toast notifications,
native share sheet (`IHostDataTransferManagerInterop`), Windows Hello WebAuthn (`webauthn.dll`),
Credential Manager, proxy from WinINET settings, `SMTC` for media keys, per-app volume,
`RegisterApplicationRestart`, and MSIX or WiX installer + delta auto-update (own updater, not
MSIX's, for control over rollout).

### 4.8 Measurement (build this before the browser)
- ETW provider for Kairo events; `QPC` timestamps at input-received, frame-start, present-submit,
  DWM-composed.
- `DwmGetCompositionTimingInfo` and **PresentMon** interop one-ETW-session-away.
- A harness that runs the *same* measurement against Chrome and Edge on the same machine, on the
  same pages, at the same refresh rate. Every performance claim must come out of this harness or
  it doesn't ship in the release notes.

---

## 5. Crate layout

```
kairo/
  crates/
    kairo-app             # WinMain, single instance, crash handler, updater, first-run
    kairo-platform-win    # Win32 window, DWM, DirectComposition, taskbar, toasts, DPI, IME, UIA
    kairo-gfx             # D3D12 device, swapchain, shared-texture import, quad/glyph renderer
    kairo-compositor      # visual tree, damage tracking, scroll offsets, frame scheduler
    kairo-ui              # chrome UI (tabs, omnibox, bookmarks, settings) — behind a trait
    kairo-engine-api      # EngineHost trait, IPC message types, input/event structs
    kairo-engine-servo    # Servo embedding (RenderingContext -> shared texture)
    kairo-engine-webview2 # WebView2 composition controller
    kairo-proc            # zygote pool, job objects, sandbox tokens, crash capture, supervision
    kairo-net             # HTTP/1.1,2,3 (hyper + quinn), DNS w/ prefetch, connection pool, preconnect
    kairo-cache           # disk cache: mmap'd index + data files, LRU, integrity, quota
    kairo-storage         # cookies, storage partitioning, history, bookmarks (SQLite via rusqlite)
    kairo-ipc             # shared-memory rings + named pipes, serialization (rkyv / bincode)
    kairo-metrics         # ETW, histograms, present timing, benchmark harness driver
    kairo-compat          # per-site fallback heuristics & blocklists
```

**Frame flow:** input (Win32) → compositor thread updates scroll/visuals immediately →
frame scheduler waits on swapchain latency object → present. Renderer processes push damage +
new textures over IPC. The UI thread is *not* on this path — that's the whole point.

**Engine trait (rough):**
```rust
trait EngineHost {
    fn create_view(&mut self, id: ViewId, rect: Rect) -> Result<ViewHandle>;
    fn navigate(&mut self, view: ViewId, url: Url) -> Result<()>;
    fn resize(&mut self, view: ViewId, rect: Rect) -> Result<()>;
    fn input(&mut self, view: ViewId, ev: InputEvent) -> Result<()>;
    fn frame_texture(&mut self, view: ViewId) -> Option<SharedTexture>;
    fn poll_events(&mut self, out: &mut Vec<EngineEvent>);
}
```

**Servo → D3D12 gotcha (verify in the spike):** Servo renders through Surfman on **ANGLE
(D3D11)** on Windows, so its texture can't simply be handed to a D3D12/wgpu device.
Options: (a) `EGL_ANGLE_d3d_share_handle_client_buffer` + `IDXGIResource1::CreateSharedHandle`
to a shared D3D11 texture, then `D3D11on12` import; (b) render Servo into an offscreen GL
framebuffer and share its backing D3D11 texture the same way. Slint shipped a working
ANGLE→D3D12 bridge for Servo in May 2026 — read that code before writing your own.
Public `servo-wgpu-interop-adapter` docs say GPU import on Windows is blocked and their
demos use CPU readback; **CPU readback is not acceptable** (kills the whole performance
thesis), so this bridge is a Phase-3 critical path item.

---

## 6. Phases, with exit criteria

**Phase 0 — Measurement &amp; scaffold (2–3 weeks)**
Win32 window + D3D12 flip swapchain + ETW harness + PresentMon runs. Baseline Chrome/Edge
numbers recorded on the dev machine.
*Exit:* you can state Kairo's and Edge's cold-start and input-to-photon numbers, reproducibly.

**Phase 1 — Shell v0 on WebView2 (4–6 weeks)**
Tabs, omnibox, history, downloads, session restore, settings, engine trait with one impl.
*Exit:* ten people use it as a daily driver; cold start measured under target.

**Phase 2 — Own compositor (6–8 weeks)**
DirectComposition tree, own UI renderer, scroll on compositor thread, damage tracking,
taskbar/jump lists, default-browser registration, DPI, dark mode, toasts.
*Exit:* Chrome-parity input latency on a 120 Hz display; no UI-thread work in the scroll path.

**Phase 3 — Servo engine (8–12 weeks, gated on the spike)**
Servo embed, ANGLE→D3D12 zero-copy bridge, input/IME/accessibility plumbing, scroll.
*Exit:* 100 representative sites (news, docs, web apps, shopping) load and scroll at 60 fps
with no CPU readback in the frame path.

**Phase 4 — Performance hardening (ongoing)**
Zygote pre-warm, background throttling + EcoQoS, tab discard, disk cache, preconnect,
PGO/LTO (`lto="fat"`, `codegen-units=1`, `panic="abort"`, `llvm-pgo`/propane).
*Exit:* Kairo beats Edge on ≥3 of 6 headline metrics in the harness.

**Phase 5 — Compatibility triage**
Automated crawl of top-10k sites against both engines; per-site fallback rules.
*Exit:* unrecoverable-breakage rate below target on the top 1000.

**Phase 6 — Security, install, update**
Site isolation, full AppContainer sandbox, installer, signed delta updates, telemetry opt-in,
and an external security review.

Phases 0–2 are independently valuable: they produce a shippable product regardless of what
Phase 3 decides.

---

## 7. Risks, ranked

1. **Engine compatibility (Servo path).** Mitigation: hybrid engine trait plus per-site
   fallback, validated at the Phase-3 spike.
2. **Servo's Windows GPU path.** Zero-copy ANGLE→D3D12 sharing is the critical path. Prototype
   it in week 1 of Phase 3; keep a Servo-direct-D3D12 patch as plan B (upstream it if it works).
3. **Scope creep into being a platform.** Extensions, sync, PDF, password manager, devtools —
   each is a quarter. Ship five features excellent, not twenty mediocre.
4. **Legal/licensing.** Chromium/CEF is BSD-3 (fine). Codec patents (H.264/AAC) are a real cost
   if you bundle decoders; Windows Media Foundation avoids some of this. Widevine requires a
   signed agreement — plan for "no DRM" in v1.
5. **"Fastest" claim discipline.** Without the harness in Phase 0, this becomes marketing and
   the project loses its reason to exist.

---

## 8. Do these five things this week

1. **Pick the engine path** (§2) and write down the one metric you'll win on.
2. `cargo new`, workspace scaffold, `windows` + `windows-core` crates, Win32 window that clears
   a D3D12 swapchain with frame-latency 1.
3. Stand up the **measurement harness** and record Edge/Chrome baselines *before* writing
   features. Non-negotiable ordering.
4. Read Slint's Servo Windows integration and the `servo` crate's `WebViewDelegate` docs.
5. **Two-week spike:** `servo 0.7` rendering one real page into your swapchain. This is the
   cheapest possible test of the entire thesis.

---

## 9. Final stack (decided)

| Layer | Choice | Why this, not the obvious alternative |
| --- | --- | --- |
| Language | Rust, current stable, pinned by `rust-toolchain.toml` | Nightly only inside `kairo-simd` if `portable_simd` earns its keep |
| Engine | **Servo 0.7.x** primary, **WebView2** per-site fallback | Only Rust engine you can tune end to end; WebView2 covers compat gaps |
| Windowing | **Raw Win32** (`windows` crate) | Reject **winit**: it hides `WM_NCCALCSIZE`, `WM_POINTER`, `WM_GETMINMAXINFO`, and DPI-context-before-create, and puts an abstraction in the input hot path |
| Present | **DXGI flip-model swapchain**, `FRAME_LATENCY_WAITABLE_OBJECT`, latency 1 | Reject blt-model and `Present(1)`; both add a frame of latency permanently |
| Graphics | **D3D12** (Agility SDK) + thin safe wrapper in `kairo-gfx`; D3D11on12 for engine texture import | Reject **wgpu** on the present path: too much abstraction between you and the latency object / present stats. wgpu only if a Servo backend forces it |
| Compositor | Own: visual tree, damage rects, compositor-thread scroll, own quad/rounded-rect/blur shaders | This is where the input-latency win lives; no framework gives it to you |
| Chrome UI | **own retained tree**: `taffy` (layout) + `parley`/`swash` (shaping, raster) + own D3D12 quads; DirectWrite for font enumeration/fallback | Reject **GPUI** (pre-1.0, DX11, upstream paused) and skip Slint for the shipping UI — it's a reference implementation to read, not a dependency to carry |
| Text | DirectWrite for enumeration + shaping fallback; own glyph atlas per (font, size, DPI, subpixel mode) | Never rasterize glyphs on the UI thread |
| IO | **IOCP** (`GetQueuedCompletionStatusEx`) + `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`; `polling` crate's IOCP backend where it helps | Reject a **tokio**-everywhere design: an extra scheduler between you and completions. Tokio allowed only in non-hot tooling |
| Network | `hyper` (h1/h2) + `quinn` (h3) + `rustls` with `aws-lc-rs` | Own connection pool, preconnect, cross-tab prioritization — you cannot tune these inside a webview |
| Cache | Custom mmap'd disk cache (mmap index + data files, LRU, quota, integrity) | The web cache is a top-3 cold-load lever; keep it yours |
| Storage | **SQLite** (`rusqlite`, bundled) for history/bookmarks/session; partitioned cookies in `kairo-storage` | Reject `redb`/`sled` here: no WAL-grade crash story to bet user history on |
| IPC | Shared-memory rings + named pipes, `rkyv` zero-copy | Never serialize frames; pass NT-handle textures |
| Sandbox | AppContainer + restricted token + job objects per renderer | Site isolation from day one, not retrofitted |
| Profiling | **Tracy** in dev, **ETW** + PresentMon for shipped numbers | Every perf claim comes from the harness |
| Build | `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `opt-level = 3`, PGO via propane/`llvm-pgo`, `target-cpu=x86-64-v3` with runtime dispatch to a v1 path | Static CRT off (share the OS copy) to cut install size and cold-start I/O |
| Packaging | MSI (WiX) for direct download, MSIX for Store, own signed delta updater | Own the rollout; don't hand update cadence to the Store |
| Compatibility | WPT subset in CI + top-10k crawl + per-site fallback rules | The only way the hybrid engine stays honest |

**Rejected outright:** winit, GPUI, Tauri/wry, Electron-style bundling, tokio on the frame
path, wgpu on the present path, and any UI framework that owns `Present`.

**The one caveat that matters:** Servo's JS engine is SpiderMonkey, and the crash-prone
parts of the web platform (media, DRM, extensions, IME edge cases) are exactly where a
one-engine Servo browser loses users. The hybrid trait is not optional architecture — it is
the difference between shipping and not shipping.

---

<!--
  INTERNAL — OWNER NOTES. Do not publish.

  Everything above this line is written for a public repository. Everything below is
  owner-facing: kill criteria, positioning, cost, headcount. This section stays at the
  very bottom of the file so it is easy to strip when the surrounding docs are promoted.
  New owner-directed notes go here, not inline in the body or in code comments. Code
  comments describe the machine; this section describes the business.
-->

## Internal — owner notes

> **Not for publication.** Plans and honest numbers, not claims.

### Kill criteria for the Servo path

The Phase-3 spike has one job: decide cheaply. Treat it as failed if, on the dev machine,
Servo cannot render **and scroll 20 representative sites at 60 fps** with a **zero-copy GPU
path** (no CPU readback) inside two weeks. On failure, do not negotiate with the deadline:
switch the primary engine to WebView2/CEF and keep the shell. Phases 0–2 survive either
decision, which is precisely why they come first.

### The uncomfortable strategic point

If the product needs extensions (uBlock), DRM (Netflix, Spotify), and zero site breakage,
then the WebView2/CEF path *is* the product and "written in Rust" is only true of the shell
— the same position Opera, Brave, and Arc occupy. The Rust-native engine is the only
durable moat, and it caps the addressable market. Beachhead where the gaps do not matter:
documentation reading, developer tooling, enterprise/education, kiosk, and low-spec hardware
— the last being a genuine, defensible win nobody else is optimising for.

### What we will not claim publicly

No "5× faster." No "beats Chrome at JavaScript." The stack cannot deliver those and a public
retraction would cost more than the temporary headline gained. Defensible line, backed by
the harness on the same machine, same pages, same display:

> fastest time-to-interactive and lowest p99 input latency on Windows, plus lower memory per tab.

Every number in a release note or README must be reproducible from a checked-in harness run.
Do not cherry-pick benchmark configs. Recording with `KAIRO_GPU_DEBUG=1` set invalidates the
measurement (debug layer costs frames) and must never appear next to a published figure.

### Cost centres to plan for

- **Media codecs.** H.264/AAC patent pools are a real per-unit cost if we ship decoders.
  Media Foundation avoids bundling but ties us to the OS pipeline.
- **Widevine.** Requires a signed agreement; absent until then, "no DRM video in v1" is a
  product decision, not a bug.
- **Chromium/CEF licensing.** BSD-3 is fine for the code; it does not cover codec patents.
- Budget a legal review before v1 ships, not after.

### Headcount and timeline

Solo: ~18–24 months to something trustworthy with credentials. Team of 4–6: ~9–12 months.
Phases 0–2 (roughly 3–4 months) are the safe 80% and ship regardless of Phase 3. Anything
that pulls in extensions, sync, PDF, or a password manager adds a quarter each and is out of
scope until the six headline metrics are won.

### Known Phase 0 gaps (so they are not mistaken for regressions later)

- No PresentMon/ETW session yet; timing is in-process QPC only, so input-to-photon is not
  measured end to end. That is the next harness step, not a browser feature.
- Refresh rate comes from the primary display only; hybrid and VRR setups will need DXGI
  output enumeration.
- `ALLOW_TEARING` is deliberately off: it needs explicit opt-in and a tearing-capability
  check, and it only helps when the user has vsync disabled.
- No IME, no UI Automation, no tearing path, no frame-budget scheduler — all Phase 2+.

