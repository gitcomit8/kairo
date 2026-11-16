//! Display enumeration.

use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};

/// Refresh rate of the primary display in Hz.
///
/// Used only to size the frame-pacing window and to decide what counts as a dropped frame.
/// Hybrid and variable-refresh setups report their current mode here, which is why the
/// pacing statistic also records raw frame intervals rather than trusting this value alone.
pub fn primary_refresh_hz() -> u32 {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode) };
    if ok.as_bool() && mode.dmDisplayFrequency > 1 {
        mode.dmDisplayFrequency
    } else {
        60
    }
}
