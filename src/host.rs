//! Hidden top-level window, message loop, and the one place that applies a
//! state-machine step to the timer, raw input, tray, and overlay.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_ESCAPE,
};
use windows::Win32::UI::Shell::NIN_POPUPOPEN;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW,
    MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    TranslateMessage, HCURSOR, HICON, MB_ICONERROR, MB_OK, MSG, WM_CLOSE, WM_CONTEXTMENU,
    WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_INPUT, WM_POWERBROADCAST, WM_QUERYENDSESSION,
    WM_RBUTTONUP, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::core::{w, PCWSTR};

use crate::clock::{self, Interrupt, Mono};
use crate::config::{self, Config};
use crate::overlay::{BreakKind, Overlay};
use crate::scheduler::Scheduler;
use crate::state::{self, Engine, Freeze, Kind, OverlayAction, Step};
use crate::tray::{self, MenuCommand, Tray};
use crate::util::{self, WM_SKIP, WM_TRAY};

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());
static IN_HANDLER: AtomicBool = AtomicBool::new(false);

const HOTKEY_SKIP: i32 = 1;

struct Hold {
    lock: bool,
    suspend: bool,
    freeze: Freeze,
    mark: Interrupt,
}

enum HoldKind {
    Lock,
    Suspend,
}

struct App {
    engine: Engine,
    boot: Option<Step>,
    hwnd: HWND,
    scheduler: Scheduler,
    tray: Tray,
    overlay: Overlay,
    raw_on: bool,
    esc_on: bool,
    hold: Option<Hold>,
    taskbar_msg: u32,
    closed: bool,
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if already_running() {
        report("久坐提醒已经在运行");
        return;
    }
    let config = config::load();
    let app = Box::new(App::new(config));
    let ptr = Box::into_raw(app);
    APP.store(ptr, Ordering::Release);
    unsafe {
        if (*ptr).start() {
            (*ptr).message_loop();
        }
        drop(Box::from_raw(ptr));
    }
    APP.store(std::ptr::null_mut(), Ordering::Release);
}

fn already_running() -> bool {
    match unsafe { FindWindowW(w!("ReminderHost"), PCWSTR::null()) } {
        Ok(hwnd) => !hwnd.is_invalid(),
        Err(_) => false,
    }
}

fn report(text: &str) {
    let text = util::wide(text);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("久坐提醒"),
            MB_OK | MB_ICONERROR,
        );
    }
}

impl App {
    fn new(config: Config) -> Self {
        let (engine, step) = Engine::start(config, clock::mono_now());
        Self {
            engine,
            boot: Some(step),
            hwnd: HWND::default(),
            scheduler: Scheduler::new(),
            tray: Tray::new(),
            overlay: Overlay::new(),
            raw_on: false,
            esc_on: false,
            hold: None,
            taskbar_msg: 0,
            closed: false,
        }
    }

    fn start(&mut self) -> bool {
        if !register_host_class() {
            report("无法注册窗口类");
            return false;
        }
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("ReminderHost"),
                w!("久坐提醒"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(util::instance()),
                None,
            )
        };
        let Ok(hwnd) = created else {
            report("无法创建窗口");
            return false;
        };
        if hwnd.is_invalid() {
            report("无法创建窗口");
            return false;
        }
        self.hwnd = hwnd;
        self.scheduler.bind(hwnd);
        self.overlay.bind_host(hwnd);
        let _ = crate::session::register(hwnd);
        self.tray.add(hwnd);
        self.taskbar_msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if let Some(step) = self.boot.take() {
            let now = clock::mono_now();
            self.apply(now, step);
        }
        true
    }

    fn message_loop(&mut self) {
        let mut message = MSG::default();
        loop {
            let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if status.0 == 0 || status.0 == -1 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    fn dispatch(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if self.taskbar_msg != 0 && msg == self.taskbar_msg {
            self.tray.add(hwnd);
            self.refresh_tip();
            return LRESULT(0);
        }
        match msg {
            WM_TIMER => {
                self.scheduler.kill();
                if self.hold.is_none() {
                    self.on_timer();
                }
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                if let Some(event) = crate::session::power_event(wparam) {
                    match event {
                        crate::session::PowerEvent::Suspend => self.begin_hold(HoldKind::Suspend),
                        crate::session::PowerEvent::Resume => self.end_hold(HoldKind::Suspend),
                    }
                }
                LRESULT(1)
            }
            WM_WTSSESSION_CHANGE => {
                if let Some(event) = crate::session::session_event(wparam) {
                    match event {
                        crate::session::SessionEvent::Lock => self.begin_hold(HoldKind::Lock),
                        crate::session::SessionEvent::Unlock => self.end_hold(HoldKind::Lock),
                    }
                }
                LRESULT(0)
            }
            WM_INPUT => {
                crate::idle::drain(lparam);
                if self.hold.is_none() && self.engine.is_idle() {
                    let now = clock::mono_now();
                    let step = self.engine.on_user_returned(now);
                    self.apply(now, step);
                }
                LRESULT(0)
            }
            WM_HOTKEY | WM_SKIP => {
                self.command_skip();
                LRESULT(0)
            }
            WM_TRAY => {
                self.on_tray(lparam);
                LRESULT(0)
            }
            WM_QUERYENDSESSION | WM_ENDSESSION => {
                self.shutdown();
                LRESULT(1)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn on_timer(&mut self) {
        let now = clock::mono_now();
        let idle_for = crate::idle::idle_for();
        let blocks = if self.engine.popup_possible(now) {
            crate::fullscreen::blocks_break()
        } else {
            false
        };
        let step = self.engine.on_wake(now, idle_for, blocks);
        self.apply(now, step);
    }

    fn on_tray(&mut self, lparam: LPARAM) {
        let code = (lparam.0 as u32) & 0xFFFF;
        if code == NIN_POPUPOPEN {
            self.refresh_tip();
            return;
        }
        if code != WM_RBUTTONUP && code != WM_CONTEXTMENU {
            return;
        }
        let status = state::menu_status(self.engine.view(clock::mono_now()));
        // The menu runs its own message loop. Drop the re-entry guard so a
        // timer that fires while the menu is open still reaches the engine.
        IN_HANDLER.store(false, Ordering::Release);
        let command = tray::show_menu(self.hwnd, &status);
        IN_HANDLER.store(true, Ordering::Release);
        if let Some(command) = command {
            self.apply_menu(command);
        }
    }

    fn apply_menu(&mut self, command: MenuCommand) {
        if self.hold.is_some() {
            return;
        }
        let now = clock::mono_now();
        let step = match command {
            MenuCommand::Rest => self.engine.rest_now(now),
            MenuCommand::Pause => self.engine.pause_hour(now),
            MenuCommand::Skip => self.engine.skip(now, crate::idle::idle_for()),
            MenuCommand::Exit => {
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                return;
            }
        };
        self.apply(now, step);
    }

    fn command_skip(&mut self) {
        if self.hold.is_some() {
            return;
        }
        let now = clock::mono_now();
        let step = self.engine.skip(now, crate::idle::idle_for());
        self.apply(now, step);
    }

    fn begin_hold(&mut self, kind: HoldKind) {
        if self.hold.is_none() {
            let now = clock::mono_now();
            self.hold = Some(Hold {
                lock: false,
                suspend: false,
                freeze: self.engine.freeze(now),
                mark: clock::interrupt_now(),
            });
            self.scheduler.kill();
            if self.raw_on {
                crate::idle::unsubscribe();
                self.raw_on = false;
            }
            self.overlay.hide();
            self.release_esc();
        }
        if let Some(hold) = self.hold.as_mut() {
            match kind {
                HoldKind::Lock => hold.lock = true,
                HoldKind::Suspend => hold.suspend = true,
            }
        }
    }

    fn end_hold(&mut self, kind: HoldKind) {
        let Some(hold) = self.hold.as_mut() else {
            return;
        };
        match kind {
            HoldKind::Lock => hold.lock = false,
            HoldKind::Suspend => hold.suspend = false,
        };
        if hold.lock || hold.suspend {
            return;
        }
        let hold = self.hold.take().expect("hold");
        let now = clock::mono_now();
        let away = clock::interrupt_now().saturating_duration_since(hold.mark);
        let blocks = if self.engine.kind() == Kind::Working {
            crate::fullscreen::blocks_break()
        } else {
            false
        };
        let step = self.engine.resume(
            now,
            away,
            hold.freeze,
            crate::idle::idle_for(),
            blocks,
        );
        self.apply(now, step);
    }

    fn apply(&mut self, now: Mono, step: Step) {
        match step.overlay {
            OverlayAction::Unchanged => {}
            OverlayAction::Hide => {
                self.overlay.hide();
                self.release_esc();
            }
            OverlayAction::ShowMicro => {
                let left = self.engine.view(now).countdown().unwrap_or(Duration::ZERO);
                self.overlay.show(BreakKind::Micro, left);
                self.grab_esc();
            }
            OverlayAction::ShowLong => {
                let left = self.engine.view(now).countdown().unwrap_or(Duration::ZERO);
                self.overlay.show(BreakKind::Long, left);
                self.grab_esc();
            }
            OverlayAction::Tick => {
                if let Some(left) = self.engine.view(now).countdown() {
                    self.overlay.tick(left);
                }
            }
        }
        if step.raw_input && !self.raw_on {
            self.raw_on = crate::idle::subscribe(self.hwnd);
        } else if !step.raw_input && self.raw_on {
            crate::idle::unsubscribe();
            self.raw_on = false;
        }
        match step.arm {
            Some(arm) => self.scheduler.arm(arm.delay, arm.precise),
            None if step.raw_input && !self.raw_on => {
                self.scheduler.arm(Duration::from_secs(30), false);
            }
            None => self.scheduler.kill(),
        }
        if step.tooltip {
            self.refresh_tip();
        }
    }

    fn refresh_tip(&mut self) {
        let view = self.engine.view(clock::mono_now());
        let hhmm = clock::local_hhmm_after(view.clock_delta());
        self.tray.set_tip(&state::tooltip_text(view, &hhmm));
    }

    fn grab_esc(&mut self) {
        if self.esc_on {
            return;
        }
        let registered = unsafe {
            RegisterHotKey(
                Some(self.hwnd),
                HOTKEY_SKIP,
                MOD_NOREPEAT,
                u32::from(VK_ESCAPE.0),
            )
        };
        self.esc_on = registered.is_ok();
    }

    fn release_esc(&mut self) {
        if !self.esc_on {
            return;
        }
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_SKIP);
        }
        self.esc_on = false;
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.release_esc();
        self.scheduler.kill();
        if self.raw_on {
            crate::idle::unsubscribe();
            self.raw_on = false;
        }
        self.overlay.hide();
        crate::session::unregister(self.hwnd);
        self.tray.remove();
    }
}

fn register_host_class() -> bool {
    let class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(host_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: util::instance(),
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: w!("ReminderHost"),
    };
    unsafe { RegisterClassW(&class) != 0 }
}

unsafe extern "system" fn host_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = APP.load(Ordering::Acquire);
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if msg == WM_CLOSE {
        if !IN_HANDLER.swap(true, Ordering::AcqRel) {
            (*ptr).shutdown();
            IN_HANDLER.store(false, Ordering::Release);
        }
        let _ = DestroyWindow(hwnd);
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    if IN_HANDLER.swap(true, Ordering::AcqRel) {
        if msg == WM_POWERBROADCAST || msg == WM_WTSSESSION_CHANGE {
            let _ = PostMessageW(Some(hwnd), msg, wparam, lparam);
            return LRESULT(1);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let result = (*ptr).dispatch(hwnd, msg, wparam, lparam);
    IN_HANDLER.store(false, Ordering::Release);
    result
}

