//! Decide whether a break popup would cover a video or a meeting.
//!
//! `SHQueryUserNotificationState` catches exclusive fullscreen, presentation
//! mode, and the shell's busy state. Browser video fullscreen often stays in
//! `QUNS_ACCEPTS_NOTIFICATIONS`, so a foreground window that covers its
//! monitor is treated the same way. Maximized windows stop at the work area
//! and do not match `rcMonitor`.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowRect,
    IsIconic, IsWindowVisible,
};

const EDGE_TOLERANCE: i32 = 4;

pub fn blocks_break() -> bool {
    if notification_state_busy() {
        return true;
    }
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() || is_shell_or_ours(hwnd) {
        return false;
    }
    covers_its_monitor(hwnd)
}

fn notification_state_busy() -> bool {
    let Ok(state) = (unsafe { SHQueryUserNotificationState() }) else {
        return false;
    };
    state == QUNS_BUSY || state == QUNS_PRESENTATION_MODE || state == QUNS_RUNNING_D3D_FULL_SCREEN
}

fn is_shell_or_ours(hwnd: HWND) -> bool {
    let shell = unsafe { GetShellWindow() };
    let desktop = unsafe { GetDesktopWindow() };
    if hwnd == shell || hwnd == desktop {
        return true;
    }
    let name = class_name(hwnd);
    matches!(
        name.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "ReminderHost" | "ReminderOverlay"
    )
}

fn covers_its_monitor(hwnd: HWND) -> bool {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
        return false;
    }
    let mut window = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut window) }.is_err() {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return false;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return false;
    }
    let monitor_rect = info.rcMonitor;
    window.left <= monitor_rect.left + EDGE_TOLERANCE
        && window.top <= monitor_rect.top + EDGE_TOLERANCE
        && window.right >= monitor_rect.right - EDGE_TOLERANCE
        && window.bottom >= monitor_rect.bottom - EDGE_TOLERANCE
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}
