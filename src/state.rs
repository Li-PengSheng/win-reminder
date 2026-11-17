//! Pure sitting-reminder state machine.
//!
//! The host feeds it the unbiased clock, how long the user has been idle, and
//! whether a break popup is allowed. It returns the single next wake-up.
//! Nothing here calls Win32, so the transitions can be tested on their own.

use std::time::Duration;

use crate::clock::Mono;
use crate::config::Config;

const PAUSE_FOR: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Working,
    Micro,
    Long,
    Idle,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Working { deadline: Mono },
    Micro { deadline: Mono },
    Long { deadline: Mono },
    Idle,
    Paused { resume_at: Mono, saved: Saved },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Saved {
    Working { remaining: Duration },
    Micro { remaining: Duration },
    Long { remaining: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freeze {
    phase: Phase,
    at: Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    Unchanged,
    Hide,
    ShowMicro,
    ShowLong,
    /// Repaint the countdown. The overlay windows stay up.
    Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm {
    pub delay: Duration,
    /// One-second countdown ticks must not be coalesced.
    pub precise: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub overlay: OverlayAction,
    pub raw_input: bool,
    pub arm: Option<Arm>,
    pub tooltip: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Working { remaining: Duration, until_break: Duration },
    Micro { remaining: Duration },
    Long { remaining: Duration },
    Idle,
    Paused { remaining: Duration },
}

impl View {
    pub fn countdown(&self) -> Option<Duration> {
        match self {
            View::Micro { remaining } | View::Long { remaining } => Some(*remaining),
            _ => None,
        }
    }

    /// Delta from local now to the clock time shown in the tray tooltip.
    pub fn clock_delta(&self) -> Duration {
        match self {
            View::Working { until_break, .. } => *until_break,
            View::Paused { remaining } => *remaining,
            View::Micro { remaining } | View::Long { remaining } => *remaining,
            View::Idle => Duration::ZERO,
        }
    }
}

pub struct Engine {
    cfg: Config,
    phase: Phase,
    segments_done: u32,
    /// After launch or a short unlock, ignore a stale `GetLastInputInfo`.
    idle_grace_until: Option<Mono>,
}

impl Engine {
    pub fn start(cfg: Config, now: Mono) -> (Self, Step) {
        let mut engine = Self {
            phase: Phase::Working {
                deadline: now.saturating_add(cfg.work),
            },
            segments_done: 0,
            idle_grace_until: Some(now.saturating_add(cfg.idle)),
            cfg,
        };
        let step = engine.plan_working(now, Duration::ZERO, true);
        (engine, step)
    }

    pub fn kind(&self) -> Kind {
        match self.phase {
            Phase::Working { .. } => Kind::Working,
            Phase::Micro { .. } => Kind::Micro,
            Phase::Long { .. } => Kind::Long,
            Phase::Idle => Kind::Idle,
            Phase::Paused { .. } => Kind::Paused,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    /// True when this wake might open a break popup, so the host should run
    /// the fullscreen check. Idle checkpoints and countdown ticks skip it.
    pub fn popup_possible(&self, now: Mono) -> bool {
        match self.phase {
            Phase::Working { deadline } => now >= deadline,
            Phase::Paused {
                resume_at,
                saved: Saved::Working { remaining },
            } => now >= resume_at && remaining.is_zero(),
            _ => false,
        }
    }

    pub fn view(&self, now: Mono) -> View {
        match self.phase {
            Phase::Working { deadline } => {
                let until_break = deadline.saturating_duration_since(now);
                View::Working {
                    remaining: until_break,
                    until_break,
                }
            }
            Phase::Micro { deadline } => View::Micro {
                remaining: deadline.saturating_duration_since(now),
            },
            Phase::Long { deadline } => View::Long {
                remaining: deadline.saturating_duration_since(now),
            },
            Phase::Idle => View::Idle,
            Phase::Paused { resume_at, .. } => View::Paused {
                remaining: resume_at.saturating_duration_since(now),
            },
        }
    }

    pub fn on_wake(&mut self, now: Mono, idle_for: Duration, blocks_popup: bool) -> Step {
        if matches!(self.phase, Phase::Idle) && idle_for < Duration::from_secs(2) {
            return self.on_user_returned(now);
        }
        match self.phase {
            Phase::Working { deadline } => {
                let in_grace = self.grace_active(now);
                if !in_grace && idle_for >= self.cfg.idle {
                    return self.enter_idle();
                }
                if now >= deadline {
                    if blocks_popup {
                        self.phase = Phase::Working {
                            deadline: now.saturating_add(self.cfg.defer),
                        };
                        return self.plan_working(now, idle_for, true);
                    }
                    return self.begin_break(now);
                }
                self.plan_working(now, idle_for, false)
            }
            Phase::Micro { deadline } => self.break_tick(now, deadline, idle_for),
            Phase::Long { deadline } => self.break_tick(now, deadline, idle_for),
            Phase::Paused { resume_at, saved } => {
                if now >= resume_at {
                    self.phase = restore_saved(now, saved);
                    return self.after_restore(now, idle_for, blocks_popup);
                }
                Step {
                    overlay: OverlayAction::Hide,
                    raw_input: false,
                    arm: Some(arm_after(resume_at.saturating_duration_since(now))),
                    tooltip: false,
                }
            }
            Phase::Idle => Step {
                overlay: OverlayAction::Hide,
                raw_input: true,
                arm: None,
                tooltip: false,
            },
        }
    }

    pub fn on_user_returned(&mut self, now: Mono) -> Step {
        if !matches!(self.phase, Phase::Idle) {
            return Step {
                overlay: OverlayAction::Unchanged,
                raw_input: false,
                arm: None,
                tooltip: false,
            };
        }
        self.phase = Phase::Working {
            deadline: now.saturating_add(self.cfg.work),
        };
        self.idle_grace_until = None;
        let mut step = self.plan_working(now, Duration::ZERO, true);
        step.overlay = OverlayAction::Hide;
        step.raw_input = false;
        step
    }

    pub fn freeze(&self, at: Mono) -> Freeze {
        Freeze {
            phase: self.phase,
            at,
        }
    }

    pub fn resume(
        &mut self,
        now: Mono,
        away: Duration,
        frozen: Freeze,
        idle_for: Duration,
        blocks_popup: bool,
    ) -> Step {
        if away >= self.cfg.idle {
            self.segments_done = segments_after_absence(self.segments_done, frozen.phase);
            self.phase = Phase::Working {
                deadline: now.saturating_add(self.cfg.work),
            };
            self.idle_grace_until = Some(now.saturating_add(self.cfg.idle));
            let mut step = self.plan_working(now, idle_for, true);
            step.overlay = OverlayAction::Hide;
            step.raw_input = false;
            return step;
        }
        self.phase = shift_phase(frozen, now);
        if matches!(self.phase, Phase::Working { .. }) {
            self.idle_grace_until = Some(now.saturating_add(self.cfg.idle));
        }
        let mut step = self.on_wake(now, idle_for, blocks_popup);
        force_visible(self.phase, &mut step);
        step.tooltip = true;
        step
    }

    pub fn rest_now(&mut self, now: Mono) -> Step {
        match self.phase {
            Phase::Micro { .. } => {
                self.phase = Phase::Micro {
                    deadline: now.saturating_add(self.cfg.micro_break),
                };
                armed_break(self.cfg.micro_break, OverlayAction::ShowMicro)
            }
            Phase::Long { .. } => {
                self.phase = Phase::Long {
                    deadline: now.saturating_add(self.cfg.long_break),
                };
                armed_break(self.cfg.long_break, OverlayAction::ShowLong)
            }
            _ => self.begin_break(now),
        }
    }

    pub fn pause_hour(&mut self, now: Mono) -> Step {
        let saved = match self.phase {
            Phase::Working { deadline } => Saved::Working {
                remaining: deadline.saturating_duration_since(now),
            },
            Phase::Micro { deadline } => Saved::Micro {
                remaining: deadline.saturating_duration_since(now),
            },
            Phase::Long { deadline } => Saved::Long {
                remaining: deadline.saturating_duration_since(now),
            },
            Phase::Idle => Saved::Working {
                remaining: self.cfg.work,
            },
            Phase::Paused { saved, .. } => saved,
        };
        self.phase = Phase::Paused {
            resume_at: now.saturating_add(PAUSE_FOR),
            saved,
        };
        self.idle_grace_until = None;
        Step {
            overlay: OverlayAction::Hide,
            raw_input: false,
            arm: Some(arm_after(PAUSE_FOR)),
            tooltip: true,
        }
    }

    pub fn skip(&mut self, now: Mono, idle_for: Duration) -> Step {
        match self.phase {
            Phase::Micro { .. } | Phase::Long { .. } => self.finish_break(now, idle_for),
            Phase::Working { .. } | Phase::Paused { .. } => {
                self.note_segment_completed();
                self.phase = Phase::Working {
                    deadline: now.saturating_add(self.cfg.work),
                };
                let mut step = self.plan_working(now, idle_for, true);
                step.overlay = OverlayAction::Hide;
                step
            }
            Phase::Idle => self.on_user_returned(now),
        }
    }

    fn after_restore(&mut self, now: Mono, idle_for: Duration, blocks_popup: bool) -> Step {
        let mut step = self.on_wake(now, idle_for, blocks_popup);
        force_visible(self.phase, &mut step);
        step.tooltip = true;
        step
    }

    fn break_tick(&mut self, now: Mono, deadline: Mono, idle_for: Duration) -> Step {
        if now >= deadline {
            return self.finish_break(now, idle_for);
        }
        let left = deadline.saturating_duration_since(now);
        Step {
            overlay: OverlayAction::Tick,
            raw_input: false,
            arm: Some(Arm {
                delay: left.min(Duration::from_secs(1)).max(Duration::from_millis(1)),
                precise: true,
            }),
            tooltip: false,
        }
    }

    fn finish_break(&mut self, now: Mono, idle_for: Duration) -> Step {
        self.phase = Phase::Working {
            deadline: now.saturating_add(self.cfg.work),
        };
        let mut step = self.plan_working(now, idle_for, true);
        if matches!(
            step.overlay,
            OverlayAction::Unchanged | OverlayAction::Tick
        ) {
            step.overlay = OverlayAction::Hide;
        }
        step
    }

    fn begin_break(&mut self, now: Mono) -> Step {
        self.idle_grace_until = None;
        self.note_segment_completed();
        if self.segments_done == 0 {
            self.phase = Phase::Long {
                deadline: now.saturating_add(self.cfg.long_break),
            };
            armed_break(self.cfg.long_break, OverlayAction::ShowLong)
        } else {
            self.phase = Phase::Micro {
                deadline: now.saturating_add(self.cfg.micro_break),
            };
            armed_break(self.cfg.micro_break, OverlayAction::ShowMicro)
        }
    }

    fn note_segment_completed(&mut self) {
        self.segments_done = self.segments_done.saturating_add(1);
        if self.segments_done >= self.cfg.long_break_every.max(1) {
            self.segments_done = 0;
        }
    }

    fn enter_idle(&mut self) -> Step {
        self.phase = Phase::Idle;
        self.idle_grace_until = None;
        Step {
            overlay: OverlayAction::Hide,
            raw_input: true,
            arm: None,
            tooltip: true,
        }
    }

    fn grace_active(&mut self, now: Mono) -> bool {
        match self.idle_grace_until {
            Some(until) if now < until => true,
            Some(_) => {
                self.idle_grace_until = None;
                false
            }
            None => false,
        }
    }

    fn plan_working(&mut self, now: Mono, idle_for: Duration, announce: bool) -> Step {
        let in_grace = self.grace_active(now);
        if !in_grace && idle_for >= self.cfg.idle {
            return self.enter_idle();
        }
        let deadline = match self.phase {
            Phase::Working { deadline } => deadline,
            _ => now,
        };
        let until_deadline = deadline.saturating_duration_since(now);
        let until_idle = if in_grace {
            self.idle_grace_until
                .unwrap_or(now)
                .saturating_duration_since(now)
        } else {
            self.cfg.idle.saturating_sub(idle_for)
        };
        Step {
            overlay: if announce {
                OverlayAction::Hide
            } else {
                OverlayAction::Unchanged
            },
            raw_input: false,
            arm: Some(arm_after(until_deadline.min(until_idle))),
            tooltip: announce,
        }
    }
}

fn armed_break(length: Duration, overlay: OverlayAction) -> Step {
    Step {
        overlay,
        raw_input: false,
        arm: Some(Arm {
            delay: length.min(Duration::from_secs(1)).max(Duration::from_millis(1)),
            precise: true,
        }),
        tooltip: true,
    }
}

fn arm_after(delay: Duration) -> Arm {
    let delay = delay.max(Duration::from_millis(1));
    Arm {
        precise: delay < Duration::from_secs(5),
        delay,
    }
}

fn restore_saved(now: Mono, saved: Saved) -> Phase {
    match saved {
        Saved::Working { remaining } => Phase::Working {
            deadline: now.saturating_add(remaining),
        },
        Saved::Micro { remaining } => Phase::Micro {
            deadline: now.saturating_add(remaining),
        },
        Saved::Long { remaining } => Phase::Long {
            deadline: now.saturating_add(remaining),
        },
    }
}

fn shift_phase(frozen: Freeze, now: Mono) -> Phase {
    let remaining = |deadline: Mono| deadline.saturating_duration_since(frozen.at);
    match frozen.phase {
        Phase::Working { deadline } => Phase::Working {
            deadline: now.saturating_add(remaining(deadline)),
        },
        Phase::Micro { deadline } => Phase::Micro {
            deadline: now.saturating_add(remaining(deadline)),
        },
        Phase::Long { deadline } => Phase::Long {
            deadline: now.saturating_add(remaining(deadline)),
        },
        Phase::Idle => Phase::Idle,
        Phase::Paused { resume_at, saved } => Phase::Paused {
            resume_at: now.saturating_add(remaining(resume_at)),
            saved,
        },
    }
}

/// A break that already started has counted its segment. Leaving mid-break
/// keeps that count. `segments_done` is stored on the engine, so this only
/// has to avoid touching it — the frozen phase is informational.
fn segments_after_absence(segments_done: u32, _phase: Phase) -> u32 {
    segments_done
}

fn force_visible(phase: Phase, step: &mut Step) {
    match phase {
        Phase::Micro { .. } => step.overlay = OverlayAction::ShowMicro,
        Phase::Long { .. } => step.overlay = OverlayAction::ShowLong,
        Phase::Idle => {
            step.overlay = OverlayAction::Hide;
            step.raw_input = true;
            step.arm = None;
        }
        Phase::Working { .. } | Phase::Paused { .. } => {
            if !matches!(
                step.overlay,
                OverlayAction::ShowMicro | OverlayAction::ShowLong
            ) {
                step.overlay = OverlayAction::Hide;
            }
        }
    }
}

pub fn menu_status(view: View) -> String {
    match view {
        View::Working { remaining, .. } => format!("剩余 {}", fmt_span(remaining)),
        View::Micro { remaining } => format!("微休息 · 剩余 {}", fmt_span(remaining)),
        View::Long { remaining } => format!("长休息 · 剩余 {}", fmt_span(remaining)),
        View::Idle => "空闲中".to_string(),
        View::Paused { remaining } => format!("暂停剩余 {}", fmt_span(remaining)),
    }
}

pub fn tooltip_text(view: View, hhmm: &str) -> String {
    match view {
        View::Working { .. } => format!("下次休息: {hhmm}"),
        View::Micro { .. } => "微休息".to_string(),
        View::Long { .. } => "长休息".to_string(),
        View::Idle => "空闲中".to_string(),
        View::Paused { .. } => format!("已暂停至 {hhmm}"),
    }
}

pub fn fmt_countdown(duration: Duration) -> String {
    let millis = duration.as_millis();
    let seconds = if millis == 0 {
        0
    } else {
        ((millis + 999) / 1000) as u64
    };
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn fmt_span(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{} 分钟", seconds / 60)
    } else {
        format!("{seconds} 秒")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    fn at(seconds_from_zero: u64) -> Mono {
        Mono::from_ticks(seconds_from_zero * 10_000_000)
    }

    fn test_config() -> Config {
        Config {
            work: seconds(100),
            micro_break: seconds(10),
            long_break: seconds(40),
            long_break_every: 3,
            idle: seconds(50),
            defer: seconds(7),
        }
    }

    fn started() -> (Engine, Mono) {
        let now = at(1_000);
        let (engine, _) = Engine::start(test_config(), now);
        (engine, now)
    }

    #[test]
    fn startup_waits_for_the_idle_checkpoint_not_every_second() {
        let (engine, now) = started();
        let step = engine_step(&engine);
        let arm = step.arm.expect("armed");
        assert!(!arm.precise || arm.delay >= seconds(5));
        assert_eq!(arm.delay, test_config().idle);
        assert!(arm.delay < test_config().work);
        assert_eq!(engine.kind(), Kind::Working);
        assert_eq!(
            engine.view(now).clock_delta(),
            test_config().work
        );
        let _ = now;
    }

    fn engine_step(engine: &Engine) -> Step {
        // Recompute the boot plan without mutating through a fresh engine.
        let (engine, step) = Engine::start(engine.cfg, at(1_000));
        let _ = engine;
        step
    }

    #[test]
    fn third_completed_segment_is_a_long_break() {
        let (mut engine, mut now) = started();
        for index in 1..=3 {
            now = deadline_of(&engine, now);
            let step = engine.on_wake(now, Duration::ZERO, false);
            if index < 3 {
                assert_eq!(step.overlay, OverlayAction::ShowMicro);
                assert_eq!(engine.kind(), Kind::Micro);
            } else {
                assert_eq!(step.overlay, OverlayAction::ShowLong);
                assert_eq!(engine.kind(), Kind::Long);
            }
            now = deadline_of(&engine, now);
            engine.on_wake(now, Duration::ZERO, false);
            assert_eq!(engine.kind(), Kind::Working);
        }
    }

    #[test]
    fn fullscreen_defers_a_single_pending_break() {
        let (mut engine, mut now) = started();
        now = deadline_of(&engine, now);
        let step = engine.on_wake(now, Duration::ZERO, true);
        assert_eq!(step.overlay, OverlayAction::Hide);
        assert_eq!(engine.kind(), Kind::Working);
        match engine.view(now) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(7)),
            other => panic!("expected working, got {other:?}"),
        }
        now = deadline_of(&engine, now);
        engine.on_wake(now, Duration::ZERO, true);
        match engine.view(now) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(7)),
            other => panic!("expected one deferred break, got {other:?}"),
        }
        assert_eq!(engine.segments_done, 0);
    }

    #[test]
    fn idle_checkpoint_subscribes_raw_input_and_return_starts_a_full_segment() {
        let (mut engine, now) = started();
        // Grace lasts `idle` seconds. Step past it with a stale input time.
        let after_grace = now.saturating_add(seconds(50));
        let step = engine.on_wake(after_grace, seconds(50), false);
        assert_eq!(engine.kind(), Kind::Idle);
        assert!(step.raw_input);
        assert!(step.arm.is_none());
        assert_eq!(step.overlay, OverlayAction::Hide);

        let back = after_grace.saturating_add(seconds(3));
        let step = engine.on_user_returned(back);
        assert_eq!(engine.kind(), Kind::Working);
        assert!(!step.raw_input);
        match engine.view(back) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(100)),
            other => panic!("expected a fresh segment, got {other:?}"),
        }
    }

    #[test]
    fn grace_ignores_a_stale_idle_reading() {
        let (mut engine, now) = started();
        let step = engine.on_wake(now, seconds(10_000), false);
        assert_eq!(engine.kind(), Kind::Working);
        assert!(step.arm.is_some());
    }

    #[test]
    fn skip_during_break_does_not_count_another_segment() {
        let (mut engine, mut now) = started();
        now = deadline_of(&engine, now);
        engine.on_wake(now, Duration::ZERO, false);
        assert_eq!(engine.kind(), Kind::Micro);
        assert_eq!(engine.segments_done, 1);
        engine.skip(now, Duration::ZERO);
        assert_eq!(engine.kind(), Kind::Working);
        assert_eq!(engine.segments_done, 1);
    }

    #[test]
    fn two_skips_then_a_completed_segment_is_the_long_break() {
        let (mut engine, mut now) = started();
        engine.skip(now, Duration::ZERO);
        engine.skip(now, Duration::ZERO);
        assert_eq!(engine.segments_done, 2);
        now = deadline_of(&engine, now);
        let step = engine.on_wake(now, Duration::ZERO, false);
        assert_eq!(step.overlay, OverlayAction::ShowLong);
        assert_eq!(engine.segments_done, 0);
    }

    #[test]
    fn pause_restores_the_saved_remainder() {
        let (mut engine, now) = started();
        let later = now.saturating_add(seconds(40));
        // Drop the startup grace so the pause snapshot is the interesting part.
        engine.idle_grace_until = None;
        engine.phase = Phase::Working {
            deadline: later.saturating_add(seconds(15)),
        };
        let step = engine.pause_hour(later);
        assert_eq!(engine.kind(), Kind::Paused);
        assert_eq!(step.overlay, OverlayAction::Hide);
        let resume_at = later.saturating_add(PAUSE_FOR);
        let step = engine.on_wake(resume_at, Duration::ZERO, false);
        assert_eq!(engine.kind(), Kind::Working);
        assert_eq!(step.overlay, OverlayAction::Hide);
        match engine.view(resume_at) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(15)),
            other => panic!("expected restored remainder, got {other:?}"),
        }
    }

    #[test]
    fn short_absence_keeps_remaining_time_and_long_absence_resets() {
        let (mut engine, now) = started();
        engine.idle_grace_until = None;
        engine.phase = Phase::Working {
            deadline: now.saturating_add(seconds(100)),
        };
        engine.segments_done = 2;
        let frozen = engine.freeze(now.saturating_add(seconds(60)));

        engine.resume(now, seconds(30), frozen, Duration::ZERO, false);
        match engine.view(now) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(40)),
            other => panic!("expected preserved remainder, got {other:?}"),
        }
        assert_eq!(engine.segments_done, 2);

        let (mut engine, now) = started();
        engine.segments_done = 2;
        let frozen = engine.freeze(now);
        engine.resume(now, seconds(600), frozen, Duration::ZERO, false);
        match engine.view(now) {
            View::Working { until_break, .. } => assert_eq!(until_break, seconds(100)),
            other => panic!("expected a fresh segment, got {other:?}"),
        }
        assert_eq!(engine.segments_done, 2);
        assert_eq!(engine.kind(), Kind::Working);
    }

    #[test]
    fn countdown_rounds_up_to_the_next_second() {
        assert_eq!(fmt_countdown(Duration::from_millis(1)), "0:01");
        assert_eq!(fmt_countdown(Duration::from_secs(20)), "0:20");
        assert_eq!(fmt_countdown(Duration::from_secs(125)), "2:05");
    }

    fn deadline_of(engine: &Engine, now: Mono) -> Mono {
        match engine.view(now).clock_delta() {
            delta if !delta.is_zero() => now.saturating_add(delta),
            _ => now,
        }
    }
}
