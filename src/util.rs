//! Small Win32 helpers shared by the window procedures.

use windows::Win32::Foundation::{HINSTANCE, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::core::PCWSTR;

pub const WM_SKIP: u32 = WM_APP + 1;
pub const WM_TRAY: u32 = WM_APP + 2;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn x_lparam(lparam: LPARAM) -> i32 {
    (lparam.0 as i16) as i32
}

pub fn y_lparam(lparam: LPARAM) -> i32 {
    ((lparam.0 >> 16) as i16) as i32
}

pub fn instance() -> HINSTANCE {
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap_or_default();
    HINSTANCE(module.0)
}
