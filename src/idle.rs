//! Idle checkpoint via `GetLastInputInfo`, plus Raw Input only while idle.
//!
//! Working time does not subscribe to the keyboard or mouse. The scheduler
//! wakes once, at last-input plus the idle threshold. Raw Input is registered
//! with `RIDEV_INPUTSINK` only after that wake confirms the user is away, and
//! removed on the first event.

use std::mem::size_of;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT,
};

const USAGE_PAGE_GENERIC: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
const USAGE_KEYBOARD: u16 = 0x06;

pub fn idle_for() -> Duration {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if !ok.as_bool() {
        // A failed read must not look like "just active", or a break would
        // fire while we cannot tell whether anyone is there.
        return Duration::from_secs(60 * 60 * 24);
    }
    let now = unsafe { GetTickCount() };
    Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime)))
}

pub fn subscribe(hwnd: HWND) -> bool {
    let devices = [
        device(USAGE_MOUSE, RIDEV_INPUTSINK, hwnd),
        device(USAGE_KEYBOARD, RIDEV_INPUTSINK, hwnd),
    ];
    unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32).is_ok() }
}

pub fn unsubscribe() {
    let devices = [
        device(USAGE_MOUSE, RIDEV_REMOVE, HWND::default()),
        device(USAGE_KEYBOARD, RIDEV_REMOVE, HWND::default()),
    ];
    unsafe {
        let _ = RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// `WM_INPUT` keeps its payload until `GetRawInputData` reads it.
pub fn drain(lparam: LPARAM) {
    let handle = HRAWINPUT(lparam.0 as *mut _);
    let mut size = 0u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    unsafe {
        let _ = GetRawInputData(handle, RID_INPUT, None, &mut size, header);
        if size == 0 {
            return;
        }
        let mut buffer = vec![0u8; size as usize];
        let _ = GetRawInputData(
            handle,
            RID_INPUT,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            header,
        );
    }
}

fn device(usage: u16, flags: windows::Win32::UI::Input::RAWINPUTDEVICE_FLAGS, hwnd: HWND) -> RAWINPUTDEVICE {
    RAWINPUTDEVICE {
        usUsagePage: USAGE_PAGE_GENERIC,
        usUsage: usage,
        dwFlags: flags,
        hwndTarget: hwnd,
    }
}
