//! D3D12 frame pipeline: one device, one direct queue, one flip-model swapchain.
//!
//! Scope is deliberately narrow for now — clear the target and present. The structure is the
//! part that matters: per-buffer command allocators gated by a fence, presentation at vsync
//! with frame latency 1, and a swapchain waitable object the caller's loop blocks on.

mod device;
mod swapchain;

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_NONE, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
    D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_VIEWPORT, ID3D12CommandAllocator, ID3D12CommandList,
    ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, PCWSTR, Result};

use device::DeviceInit;
use swapchain::BUFFER_COUNT;

/// The frame pipeline. Owns the device; one per process.
pub struct Gfx {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    swapchain: swapchain::Swapchain,
    allocators: Vec<ID3D12CommandAllocator>,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    /// Fence value the GPU must reach before `allocators[i]` may be reset again.
    fence_values: Vec<u64>,
    signal_value: u64,
    fence_event: HANDLE,
    frame: usize,
    width: u32,
    height: u32,
    adapter_name: String,
}

impl Gfx {
    /// Build the device and swapchain for `hwnd`, sized to `width` × `height` physical pixels.
    ///
    /// `enable_debug` turns on the D3D12 debug layer; it must be requested before the first
    /// device is created in the process, and it costs frames — never measure with it on.
    pub fn new(hwnd: HWND, width: u32, height: u32, enable_debug: bool) -> Result<Self> {
        let DeviceInit {
            device,
            queue,
            factory,
            adapter_name,
        } = device::create(enable_debug)?;

        let allocators = (0..BUFFER_COUNT)
            .map(|_| unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) })
            .collect::<Result<Vec<ID3D12CommandAllocator>>>()?;

        let list: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)?
        };

        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let fence_event = unsafe { CreateEventW(None, false, false, PCWSTR::null())? };

        let swapchain = swapchain::Swapchain::new(
            &factory,
            &device,
            &queue,
            hwnd,
            width.max(1),
            height.max(1),
        )?;
        let frame = swapchain.current_index() as usize;

        Ok(Self {
            device,
            queue,
            swapchain,
            allocators,
            list,
            fence,
            fence_values: vec![0; BUFFER_COUNT as usize],
            signal_value: 0,
            fence_event,
            frame,
            width,
            height,
            adapter_name,
        })
    }

    /// Name of the adapter in use, for the startup report.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Swapchain waitable handle. Block on this to pace frames.
    pub fn waitable(&self) -> HANDLE {
        self.swapchain.waitable()
    }

    /// Current back buffer size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reset and record the command list for one frame.
    fn record(&mut self, color: [f32; 4]) -> Result<()> {
        let index = self.frame;
        let handle = self.swapchain.rtv(index);
        let viewport = D3D12_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.width as f32,
            Height: self.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let scissor = RECT {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        };

        let buffer = self.swapchain.buffer(index);
        let to_render = transition(
            buffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        let to_present = transition(
            buffer,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        );

        unsafe {
            self.list.ResourceBarrier(&[to_render]);
            self.list.OMSetRenderTargets(1, Some(&handle), false, None);
            self.list.ClearRenderTargetView(handle, &color, None);
            self.list.RSSetViewports(&[viewport]);
            self.list.RSSetScissorRects(&[scissor]);
            self.list.ResourceBarrier(&[to_present]);
            self.list.Close()?;
        }
        Ok(())
    }

    /// Render and present one frame.
    ///
    /// Blocks until the swapchain can accept a frame, so callers should not add their own
    /// pacing. Returns `Ok(())` when the frame is queued for presentation.
    pub fn render(&mut self, color: [f32; 4]) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Ok(());
        }
        let index = self.frame;

        // The GPU may still be reading this buffer's allocator; wait for its fence first.
        if unsafe { self.fence.GetCompletedValue() } < self.fence_values[index] {
            unsafe {
                self.fence
                    .SetEventOnCompletion(self.fence_values[index], self.fence_event)?;
                WaitForSingleObject(self.fence_event, INFINITE);
            }
        }

        unsafe {
            self.allocators[index].Reset()?;
            self.list.Reset(&self.allocators[index], None)?;
        }

        self.record(color)?;

        unsafe {
            self.queue
                .ExecuteCommandLists(&[Some(self.list.cast::<ID3D12CommandList>()?)]);
            self.signal_value += 1;
            self.queue.Signal(&self.fence, self.signal_value)?;
        }
        self.fence_values[index] = self.signal_value;

        self.swapchain.present()?;
        self.frame = self.swapchain.current_index() as usize;
        Ok(())
    }

    /// Resize the back buffers to match a new client area.
    ///
    /// Zero-sized requests (minimised) are ignored: there is nothing to render and dangling
    /// zero-dimension render targets are how resize bugs start.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return Ok(());
        }
        self.wait_idle()?;
        self.swapchain.resize(width, height)?;
        self.width = width;
        self.height = height;
        self.frame = self.swapchain.current_index() as usize;
        Ok(())
    }

    /// Block until the GPU has drained every queued frame.
    pub fn wait_idle(&mut self) -> Result<()> {
        self.signal_value += 1;
        let value = self.signal_value;
        unsafe {
            self.queue.Signal(&self.fence, value)?;
            if self.fence.GetCompletedValue() < value {
                self.fence.SetEventOnCompletion(value, self.fence_event)?;
                WaitForSingleObject(self.fence_event, INFINITE);
            }
        }
        self.fence_values.fill(value);
        Ok(())
    }

    /// The underlying device, for subsystems (compositor, text atlas) that allocate resources.
    pub fn device(&self) -> &ID3D12Device {
        &self.device
    }
}

/// Build an explicit render-target transition barrier.
///
/// The COM struct holds a reference to the resource inside a union, which is why it needs
/// the manual `ManuallyDrop` wrapping rather than a plain initialiser.
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}
