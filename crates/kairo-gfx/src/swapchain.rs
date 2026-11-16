//! The presentation swapchain.
//!
//! Every field here is chosen for latency, not throughput:
//!
//! * `FLIP_DISCARD` — the only modern presentation model; the blt model adds a full-frame
//!   copy and cannot use a frame-latency waitable object at all.
//! * `SetMaximumFrameLatency(1)` — at most one frame queued ahead of the CPU.
//! * `FRAME_LATENCY_WAITABLE_OBJECT` — gives the frame loop a handle to block on instead of
//!   guessing when the next frame can start.

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain1, IDXGISwapChain3,
};
use windows::core::{Interface, Result};

/// Three buffers: the minimum that keeps a frame in flight while the waitable object gates
/// the CPU. Two would serialise the pipeline against the compositor.
pub(crate) const BUFFER_COUNT: u32 = 3;

/// 8-bit sRGB-ish swapchain format; the compositor blends in this space like Windows itself.
pub(crate) const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;

const FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

pub(crate) struct Swapchain {
    device: ID3D12Device,
    sw: IDXGISwapChain3,
    heap: ID3D12DescriptorHeap,
    rtv_size: u32,
    buffers: Vec<ID3D12Resource>,
    waitable: HANDLE,
}

impl Swapchain {
    pub(crate) fn new(
        factory: &IDXGIFactory4,
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        hwnd: HWND,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: FORMAT,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: FLAGS.0 as u32,
        };

        let sw1: IDXGISwapChain1 =
            unsafe { factory.CreateSwapChainForHwnd(queue, hwnd, &desc, None, None)? };
        let sw: IDXGISwapChain3 = sw1.cast()?;

        unsafe { sw.SetMaximumFrameLatency(1)? };
        // Despite the `Set` in its name, this acquires (and returns) the waitable handle.
        let waitable = unsafe { sw.GetFrameLatencyWaitableObject() };

        let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: BUFFER_COUNT,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        };
        let heap: ID3D12DescriptorHeap = unsafe { device.CreateDescriptorHeap(&heap_desc)? };
        let rtv_size =
            unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) };

        let mut this = Self {
            device: device.clone(),
            sw,
            heap,
            rtv_size,
            buffers: Vec::with_capacity(BUFFER_COUNT as usize),
            waitable,
        };
        this.create_targets()?;
        Ok(this)
    }

    fn create_targets(&mut self) -> Result<()> {
        for index in 0..BUFFER_COUNT {
            let buffer: ID3D12Resource = unsafe { self.sw.GetBuffer(index)? };
            let handle = Self::handle_at(self.rtv_base(), self.rtv_size, index);
            unsafe { self.device.CreateRenderTargetView(&buffer, None, handle) };
            self.buffers.push(buffer);
        }
        Ok(())
    }

    fn rtv_base(&self) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() }
    }

    fn handle_at(
        base: D3D12_CPU_DESCRIPTOR_HANDLE,
        increment: u32,
        index: u32,
    ) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: base.ptr + (index * increment) as usize,
        }
    }

    /// Render target view for back buffer `index`.
    pub(crate) fn rtv(&self, index: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        Self::handle_at(self.rtv_base(), self.rtv_size, index as u32)
    }

    pub(crate) fn buffer(&self, index: usize) -> &ID3D12Resource {
        &self.buffers[index]
    }

    pub(crate) fn current_index(&self) -> u32 {
        unsafe { self.sw.GetCurrentBackBufferIndex() }
    }

    /// Handle that becomes signalled when the swapchain can accept another frame.
    pub(crate) fn waitable(&self) -> HANDLE {
        self.waitable
    }

    /// Present at vsync. `sync_interval = 1` with frame latency 1 is the lowest-latency
    /// configuration that still tears nothing.
    pub(crate) fn present(&self) -> Result<()> {
        unsafe { self.sw.Present(1, DXGI_PRESENT(0)) }.ok()
    }

    /// Resize the back buffers. Caller must ensure the GPU is idle.
    pub(crate) fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.buffers.clear();
        unsafe {
            self.sw
                .ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, FLAGS)?;
        }
        // The waitable object belongs to the swapchain, not to its buffers, so the handle
        // acquired at creation stays valid across resizes.
        self.create_targets()
    }
}
