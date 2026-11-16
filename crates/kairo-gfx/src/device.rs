//! Device and adapter selection.

use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12CreateDevice,
    D3D12GetDebugInterface, ID3D12CommandQueue, ID3D12Debug, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    IDXGIAdapter1, IDXGIFactory4, IDXGIFactory6,
};
use windows::core::{Error, Interface, Result};

/// The objects needed to build a swapchain and a command stream.
pub(crate) struct DeviceInit {
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    pub factory: IDXGIFactory4,
    pub adapter_name: String,
}

/// Create the D3D12 device, its direct command queue, and the DXGI factory.
pub(crate) fn create(enable_debug: bool) -> Result<DeviceInit> {
    if enable_debug {
        // Debug layer must be enabled before device creation.
        let mut debug: Option<ID3D12Debug> = None;
        if unsafe { D3D12GetDebugInterface(&mut debug) }.is_ok()
            && let Some(debug) = debug
        {
            unsafe { debug.EnableDebugLayer() };
        }
    }

    let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1()? };
    let adapter = pick_adapter(&factory)?;

    let desc = unsafe { adapter.GetDesc1()? };
    let adapter_name = String::from_utf16_lossy(&desc.Description)
        .trim_end_matches('\0')
        .to_string();

    // Feature level 11_0 is the floor: everything Kairo renders is quads and textures, so
    // asking for 12_0 would exclude GPUs for no capability we need.
    let mut created: Option<ID3D12Device> = None;
    unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut created)? };
    let device = created.ok_or_else(|| Error::from(E_FAIL))?;

    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        ..Default::default()
    };
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc)? };

    Ok(DeviceInit {
        device,
        queue,
        factory,
        adapter_name,
    })
}

/// Prefer the high-performance adapter (hybrid laptops list the iGPU first), skipping the
/// software adapter so we do not silently fall back to WARP.
fn pick_adapter(factory: &IDXGIFactory4) -> Result<IDXGIAdapter1> {
    if let Ok(factory6) = factory.cast::<IDXGIFactory6>() {
        for index in 0..8u32 {
            let adapter: IDXGIAdapter1 = match unsafe {
                factory6.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
            } {
                Ok(adapter) => adapter,
                Err(_) => break,
            };
            if let Ok(desc) = unsafe { adapter.GetDesc1() }
                && (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) == 0
            {
                return Ok(adapter);
            }
        }
    }
    unsafe { factory.EnumAdapters1(0) }
}
