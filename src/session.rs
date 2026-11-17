//! Sleep and session-lock notifications.
//!
//! The host window is a real (hidden) top-level window. `HWND_MESSAGE`
//! windows do not receive `WM_POWERBROADCAST`.

use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMECRITICAL, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Lock,
    Unlock,
}

pub fn register(hwnd: HWND) -> bool {
    unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_ok() }
}

pub fn unregister(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = WTSUnRegisterSessionNotification(hwnd);
    }
}

pub fn power_event(wparam: WPARAM) -> Option<PowerEvent> {
    match wparam.0 as u32 {
        PBT_APMSUSPEND => Some(PowerEvent::Suspend),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
            Some(PowerEvent::Resume)
        }
        _ => None,
    }
}

pub fn session_event(wparam: WPARAM) -> Option<SessionEvent> {
    match wparam.0 as u32 {
        WTS_SESSION_LOCK => Some(SessionEvent::Lock),
        WTS_SESSION_UNLOCK => Some(SessionEvent::Unlock),
        _ => None,
    }
}
