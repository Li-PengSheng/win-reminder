//! Unbiased deadlines, and interrupt time that includes sleep.
//!
//! `QueryUnbiasedInterruptTime` stops while the machine sleeps, so a nap does
//! not burn a work segment. Lock/sleep *absence* uses `QueryInterruptTime`,
//! which keeps moving through sleep, so a long nap still counts as leaving.

use std::time::Duration;

use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::WindowsProgramming::{QueryInterruptTime, QueryUnbiasedInterruptTime};

/// 100 ns ticks from `QueryUnbiasedInterruptTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mono(u64);

/// 100 ns ticks from `QueryInterruptTime` (includes sleep and hibernation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Interrupt(u64);

pub fn mono_now() -> Mono {
    let mut ticks = 0u64;
    let ok = unsafe { QueryUnbiasedInterruptTime(&mut ticks) };
    if ok.as_bool() {
        Mono(ticks)
    } else {
        Mono(0)
    }
}

pub fn interrupt_now() -> Interrupt {
    Interrupt(unsafe { QueryInterruptTime() })
}

impl Mono {
    #[cfg(test)]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    pub fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration_to_ticks(duration)))
    }

    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        ticks_to_duration(self.0.saturating_sub(earlier.0))
    }
}

impl Interrupt {
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        ticks_to_duration(self.0.saturating_sub(earlier.0))
    }
}

pub fn duration_to_ticks(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    if nanos >= u128::from(u64::MAX) {
        u64::MAX
    } else {
        (nanos / 100) as u64
    }
}

pub fn ticks_to_duration(ticks: u64) -> Duration {
    match ticks.checked_mul(100) {
        Some(nanos) => Duration::from_nanos(nanos),
        None => Duration::MAX,
    }
}

/// Local `HH:MM` after `delta` from the current local clock.
///
/// Only hours and minutes are needed for the tray tooltip, so this adds the
/// duration onto `GetLocalTime` directly and does not pull in timezone APIs.
pub fn local_hhmm_after(delta: Duration) -> String {
    let now = unsafe { GetLocalTime() };
    let base = u64::from(now.wHour) * 3600 + u64::from(now.wMinute) * 60 + u64::from(now.wSecond);
    let target = base.saturating_add(delta.as_secs());
    let minutes = (target / 60) % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_durations_round_trip_through_100ns_ticks() {
        let duration = Duration::from_secs(125);
        let ticks = duration_to_ticks(duration);
        assert_eq!(ticks, 125 * 10_000_000);
        assert_eq!(ticks_to_duration(ticks), duration);
    }

    #[test]
    fn mono_add_and_since_are_exact_for_whole_seconds() {
        let start = Mono::from_ticks(50);
        let later = start.saturating_add(Duration::from_secs(5));
        assert_eq!(
            later.saturating_duration_since(start),
            Duration::from_secs(5)
        );
        assert_eq!(start.saturating_duration_since(later), Duration::ZERO);
    }
}
