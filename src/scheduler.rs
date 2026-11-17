//! One coalescable window timer.
//!
//! `SetCoalescableTimer` is periodic, so every `WM_TIMER` kills it before the
//! next deadline is computed. The process stays asleep for minutes at a time
//! during a work segment. The one-second cadence exists only while a break
//! overlay is on screen.

use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, SetCoalescableTimer, SetTimer, TIMERV_NO_COALESCING,
};

const TIMER_ID: usize = 1;

pub struct Scheduler {
    hwnd: HWND,
    armed: bool,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            hwnd: HWND::default(),
            armed: false,
        }
    }

    pub fn bind(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
    }

    pub fn arm(&mut self, delay: Duration, precise: bool) {
        if self.hwnd.is_invalid() {
            return;
        }
        self.kill();
        let elapsed = delay_ms(delay);
        let tolerance = if precise {
            TIMERV_NO_COALESCING
        } else if delay >= Duration::from_secs(60) {
            5_000
        } else {
            1_000
        };
        let id = unsafe { SetCoalescableTimer(Some(self.hwnd), TIMER_ID, elapsed, None, tolerance) };
        if id == 0 {
            let fallback = unsafe { SetTimer(Some(self.hwnd), TIMER_ID, elapsed, None) };
            self.armed = fallback != 0;
        } else {
            self.armed = true;
        }
    }

    pub fn kill(&mut self) {
        if self.hwnd.is_invalid() {
            self.armed = false;
            return;
        }
        if self.armed {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            }
        }
        self.armed = false;
    }
}

fn delay_ms(delay: Duration) -> u32 {
    let millis = delay.as_millis();
    if millis < 1 {
        1
    } else if millis > 0x7FFF_FFFF {
        0x7FFF_FFFF
    } else {
        millis as u32
    }
}
