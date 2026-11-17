//! Break veil.
//!
//! `WS_EX_LAYERED` plus `SetLayeredWindowAttributes` applies one alpha to the
//! whole window. The process never builds or uploads a screen-sized pixel
//! buffer (`UpdateLayeredWindow` is intentionally not used). GDI draws only
//! the centered words, the countdown, and the skip button.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, RoundRect, SelectObject, SetBkMode,
    SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos, LoadCursorW,
    PostMessageW, RegisterClassW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, CS_HREDRAW,
    CS_VREDRAW, GWLP_USERDATA, HICON, IDC_ARROW, LWA_ALPHA, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_SHOW, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP,
    WM_NCCREATE, WM_PAINT, WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, HWND_TOPMOST,
};
use windows::core::{w, PCWSTR};

use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};

use crate::state::fmt_countdown;
use crate::util::{self, WM_SKIP};

const ALPHA: u8 = 210;
static HOST: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakKind {
    Micro,
    Long,
}

struct Fonts {
    title: HFONT,
    big: HFONT,
    body: HFONT,
    button: HFONT,
}

struct Veil {
    hwnd: HWND,
    kind: BreakKind,
    dpi: u32,
    countdown: [u16; 16],
    fonts: Fonts,
    background: HBRUSH,
    button_fill: HBRUSH,
}

struct Inner {
    windows: Vec<Box<Veil>>,
    registered: bool,
}

pub struct Overlay {
    inner: RefCell<Inner>,
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(Inner {
                windows: Vec::new(),
                registered: false,
            }),
        }
    }

    pub fn bind_host(&self, hwnd: HWND) {
        HOST.store(hwnd.0 as isize, Ordering::Release);
    }

    pub fn show(&self, kind: BreakKind, remaining: Duration) {
        self.hide();
        let rects = match kind {
            BreakKind::Micro => vec![cursor_monitor()],
            BreakKind::Long => {
                let all = all_monitors();
                if all.is_empty() {
                    vec![cursor_monitor()]
                } else {
                    all
                }
            }
        };
        let hwnds = {
            let mut inner = self.inner.borrow_mut();
            inner.rebuild(kind, remaining, &rects);
            inner.hwnds()
        };
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let mut focus = HWND::default();
        let mut fallback = HWND::default();
        for hwnd in hwnds {
            unsafe {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA);
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
            }
            if fallback.is_invalid() {
                fallback = hwnd;
            }
            if point_in_window(hwnd, cursor) {
                focus = hwnd;
            }
        }
        if focus.is_invalid() {
            focus = fallback;
        }
        if !focus.is_invalid() {
            unsafe {
                let _ = SetForegroundWindow(focus);
                let _ = SetFocus(Some(focus));
            }
        }
    }

    pub fn tick(&self, remaining: Duration) {
        let text = wide_fixed(&fmt_countdown(remaining));
        let jobs: Vec<(HWND, RECT)> = {
            let mut inner = self.inner.borrow_mut();
            for veil in &mut inner.windows {
                veil.countdown = text;
            }
            inner
                .windows
                .iter()
                .filter_map(|veil| countdown_rect(veil.hwnd).map(|rect| (veil.hwnd, rect)))
                .collect()
        };
        for (hwnd, rect) in jobs {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), Some(std::ptr::from_ref(&rect)), false);
            }
        }
    }

    pub fn hide(&self) {
        let (hwnds, background, button) = {
            let mut inner = self.inner.borrow_mut();
            let background = inner.windows.first().map(|veil| veil.background);
            let button = inner.windows.first().map(|veil| veil.button_fill);
            let hwnds: Vec<HWND> = inner
                .windows
                .drain(..)
                .map(|veil| {
                    unsafe {
                        SetWindowLongPtrW(veil.hwnd, GWLP_USERDATA, 0);
                    }
                    veil.fonts.destroy();
                    veil.hwnd
                })
                .collect();
            (hwnds, background, button)
        };
        for hwnd in hwnds {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        unsafe {
            if let Some(background) = background {
                let _ = DeleteObject(background.into());
            }
            if let Some(button) = button {
                let _ = DeleteObject(button.into());
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.hide();
    }
}

impl Inner {
    fn rebuild(&mut self, kind: BreakKind, remaining: Duration, rects: &[RECT]) {
        self.ensure_class();
        let countdown = wide_fixed(&fmt_countdown(remaining));
        let background = unsafe { CreateSolidBrush(rgb(12, 16, 20)) };
        let button_fill = unsafe { CreateSolidBrush(rgb(232, 236, 230)) };
        for rect in rects {
            let width = (rect.right - rect.left).max(1);
            let height = (rect.bottom - rect.top).max(1);
            let veil = Box::new(Veil {
                hwnd: HWND::default(),
                kind,
                dpi: 96,
                countdown,
                fonts: Fonts::empty(),
                background,
                button_fill,
            });
            let raw = Box::into_raw(veil);
            let created = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                    w!("ReminderOverlay"),
                    w!(""),
                    WS_POPUP,
                    rect.left,
                    rect.top,
                    width,
                    height,
                    None,
                    None,
                    Some(util::instance()),
                    Some(raw.cast()),
                )
            };
            let hwnd = match created {
                Ok(hwnd) if !hwnd.is_invalid() => hwnd,
                _ => {
                    unsafe { drop(Box::from_raw(raw)) };
                    continue;
                }
            };
            unsafe {
                (*raw).hwnd = hwnd;
                let dpi = GetDpiForWindow(hwnd).max(96);
                (*raw).dpi = dpi;
                (*raw).fonts = Fonts::create(dpi);
            }
            self.windows.push(unsafe { Box::from_raw(raw) });
        }
        if self.windows.is_empty() {
            unsafe {
                let _ = DeleteObject(background.into());
                let _ = DeleteObject(button_fill.into());
            }
        }
    }

    fn hwnds(&self) -> Vec<HWND> {
        self.windows.iter().map(|veil| veil.hwnd).collect()
    }

    fn ensure_class(&mut self) {
        if self.registered {
            return;
        }
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(veil_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: util::instance(),
            hIcon: HICON::default(),
            hCursor: cursor,
            hbrBackground: unsafe { CreateSolidBrush(rgb(12, 16, 20)) },
            lpszMenuName: PCWSTR::null(),
            lpszClassName: w!("ReminderOverlay"),
        };
        self.registered = unsafe { RegisterClassW(&class) } != 0;
    }
}

impl Fonts {
    fn empty() -> Self {
        Self {
            title: HFONT::default(),
            big: HFONT::default(),
            body: HFONT::default(),
            button: HFONT::default(),
        }
    }

    fn create(dpi: u32) -> Self {
        Self {
            title: font(scale(32, dpi), FW_SEMIBOLD.0 as i32),
            big: font(scale(96, dpi), FW_NORMAL.0 as i32),
            body: font(scale(20, dpi), FW_NORMAL.0 as i32),
            button: font(scale(18, dpi), FW_SEMIBOLD.0 as i32),
        }
    }

    fn destroy(&self) {
        for font in [self.title, self.big, self.body, self.button] {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
    }
}

fn font(height: i32, weight: i32) -> HFONT {
    let face = util::wide("Microsoft YaHei UI");
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn scale(pixels: i32, dpi: u32) -> i32 {
    (pixels * dpi as i32) / 96
}

fn rgb(red: u32, green: u32, blue: u32) -> COLORREF {
    COLORREF(red | (green << 8) | (blue << 16))
}

fn wide_fixed(text: &str) -> [u16; 16] {
    let mut buffer = [0u16; 16];
    for (index, unit) in text.encode_utf16().take(15).enumerate() {
        buffer[index] = unit;
    }
    buffer
}

fn countdown_rect(hwnd: HWND) -> Option<RECT> {
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    Some(layout(client, dpi).count)
}

struct Layout {
    title: RECT,
    count: RECT,
    hint: RECT,
    button: RECT,
}

fn layout(client: RECT, dpi: u32) -> Layout {
    let width = client.right - client.left;
    let height = client.bottom - client.top;
    let mid_y = client.top + height / 2;
    let title_h = scale(48, dpi);
    let count_h = scale(120, dpi);
    let hint_h = scale(36, dpi);
    let button_w = scale(168, dpi);
    let button_h = scale(48, dpi);
    Layout {
        title: RECT {
            left: client.left,
            right: client.right,
            top: mid_y - scale(150, dpi),
            bottom: mid_y - scale(150, dpi) + title_h,
        },
        count: RECT {
            left: client.left,
            right: client.right,
            top: mid_y - count_h / 2,
            bottom: mid_y + count_h / 2,
        },
        hint: RECT {
            left: client.left,
            right: client.right,
            top: mid_y + scale(70, dpi),
            bottom: mid_y + scale(70, dpi) + hint_h,
        },
        button: RECT {
            left: client.left + (width - button_w) / 2,
            right: client.left + (width - button_w) / 2 + button_w,
            top: mid_y + scale(130, dpi),
            bottom: mid_y + scale(130, dpi) + button_h,
        },
    }
}

fn point_in_window(hwnd: HWND, point: POINT) -> bool {
    let mut rect = RECT::default();
    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect) }.is_err()
    {
        return false;
    }
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

fn cursor_monitor() -> RECT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    monitor_rect(unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) })
        .unwrap_or(RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        })
}

fn all_monitors() -> Vec<RECT> {
    let mut list = Vec::new();
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor),
            LPARAM(&mut list as *mut Vec<RECT> as isize),
        );
    }
    list
}

unsafe extern "system" fn enum_monitor(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    if let Some(rect) = monitor_rect(monitor) {
        let list = &mut *(data.0 as *mut Vec<RECT>);
        list.push(rect);
    }
    windows::core::BOOL(1)
}

fn monitor_rect(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> Option<RECT> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Some(info.rcMonitor)
    } else {
        None
    }
}

unsafe extern "system" fn veil_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let pointer = veil_from(hwnd);
    if pointer.is_null() {
        return match msg {
            WM_PAINT => {
                let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint);
                let _ = EndPaint(hwnd, &paint);
                let _ = hdc;
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        };
    }
    let veil = &mut *pointer;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                windows::Win32::UI::WindowsAndMessaging::SetCursor(Some(cursor));
            }
            LRESULT(1)
        }
        WM_PAINT => {
            paint_veil(veil);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_ok() {
                let hit = layout(client, veil.dpi).button;
                let x = util::x_lparam(lparam);
                let y = util::y_lparam(lparam);
                if x >= hit.left && x < hit.right && y >= hit.top && y < hit.bottom {
                    post_skip();
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            post_skip();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = ((wparam.0 >> 16) as u32).max(96);
            veil.dpi = dpi;
            veil.fonts.destroy();
            veil.fonts = Fonts::create(dpi);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn veil_from(hwnd: HWND) -> *mut Veil {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Veil }
}

fn post_skip() {
    let raw = HOST.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_SKIP, WPARAM(0), LPARAM(0));
    }
}

fn paint_veil(veil: &mut Veil) {
    let mut paint = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(veil.hwnd, &mut paint) };
    if hdc.is_invalid() {
        return;
    }
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(veil.hwnd, &mut client);
        let _ = FillRect(hdc, &client, veil.background);
        let _ = SetBkMode(hdc, TRANSPARENT);
    }
    let places = layout(client, veil.dpi);
    let (title, hint) = match veil.kind {
        BreakKind::Micro => ("看向远处", "眨眨眼，让眼睛休息一下"),
        BreakKind::Long => ("起来活动", "离开座位，看看远处"),
    };
    draw_line(hdc, title, places.title, veil.fonts.title, rgb(236, 240, 236));
    draw_units(hdc, &veil.countdown, places.count, veil.fonts.big, rgb(244, 241, 234));
    draw_line(hdc, hint, places.hint, veil.fonts.body, rgb(186, 196, 190));
    unsafe {
        let _ = SelectObject(hdc, veil.button_fill.into());
        let _ = RoundRect(
            hdc,
            places.button.left,
            places.button.top,
            places.button.right,
            places.button.bottom,
            scale(12, veil.dpi),
            scale(12, veil.dpi),
        );
    }
    draw_line(hdc, "跳过", places.button, veil.fonts.button, rgb(24, 28, 26));
    unsafe {
        let _ = EndPaint(veil.hwnd, &paint);
    }
}

fn draw_line(hdc: HDC, text: &str, rect: RECT, font: HFONT, color: COLORREF) {
    let wide = util::wide(text);
    draw_units(hdc, &wide, rect, font, color);
}

fn draw_units(hdc: HDC, text: &[u16], mut rect: RECT, font: HFONT, color: COLORREF) {
    if font.is_invalid() {
        return;
    }
    let chars = text
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(text.len())
        .min(127);
    let mut buffer = [0u16; 128];
    buffer[..chars].copy_from_slice(&text[..chars]);
    unsafe {
        let _ = SetTextColor(hdc, color);
        let previous = SelectObject(hdc, font.into());
        let _ = DrawTextW(
            hdc,
            &mut buffer[..chars],
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        let _ = SelectObject(hdc, previous);
    }
}
