//! Notification-area icon. The tooltip is rewritten on state changes and when
//! the shell is about to show it (`NIN_POPUPOPEN`), never on a one-second tick.

use std::mem::{size_of, zeroed};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    LoadIconW, PostMessageW, SetForegroundWindow, TrackPopupMenuEx, HICON, ICONINFO, IDI_APPLICATION,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};
use windows::core::PCWSTR;

use crate::util::{self, WM_TRAY};

const ICON_ID: u32 = 1;
const CMD_REST: usize = 1;
const CMD_PAUSE: usize = 2;
const CMD_SKIP: usize = 3;
const CMD_EXIT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Rest,
    Pause,
    Skip,
    Exit,
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    tip: [u16; 128],
    added: bool,
}

impl Tray {
    pub fn new() -> Self {
        Self {
            hwnd: HWND::default(),
            icon: HICON::default(),
            tip: [0; 128],
            added: false,
        }
    }

    pub fn add(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        if self.icon.is_invalid() {
            self.icon = load_icon();
        }
        let mut data = blank_nid(hwnd, self.icon);
        write_tip(&mut data.szTip, &self.tip);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
                self.added = true;
            }
        }
    }

    pub fn remove(&mut self) {
        if self.added {
            let data = blank_nid(self.hwnd, self.icon);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            self.added = false;
        }
        if !self.icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = HICON::default();
        }
    }

    pub fn set_tip(&mut self, text: &str) {
        self.tip = [0; 128];
        for (index, unit) in text.encode_utf16().take(127).enumerate() {
            self.tip[index] = unit;
        }
        if !self.added {
            return;
        }
        let mut data = blank_nid(self.hwnd, self.icon);
        data.uFlags = NIF_TIP;
        write_tip(&mut data.szTip, &self.tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
    }
}

pub fn show_menu(hwnd: HWND, status: &str) -> Option<MenuCommand> {
    let menu = unsafe { CreatePopupMenu() };
    let Ok(menu) = menu else {
        return None;
    };
    let status = util::wide(status);
    let rest = util::wide("立即休息");
    let pause = util::wide("暂停 1 小时");
    let skip = util::wide("跳过本次");
    let exit = util::wide("退出");
    unsafe {
        let _ = AppendMenuW(menu, MF_GRAYED | MF_STRING, 100, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, CMD_REST, PCWSTR(rest.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, CMD_PAUSE, PCWSTR(pause.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, CMD_SKIP, PCWSTR(skip.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, PCWSTR(exit.as_ptr()));
        let _ = SetForegroundWindow(hwnd);
    }
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let selected = unsafe {
        TrackPopupMenuEx(
            menu,
            (TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY).0,
            cursor.x,
            cursor.y,
            hwnd,
            None,
        )
    };
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
    match selected.0 as usize {
        CMD_REST => Some(MenuCommand::Rest),
        CMD_PAUSE => Some(MenuCommand::Pause),
        CMD_SKIP => Some(MenuCommand::Skip),
        CMD_EXIT => Some(MenuCommand::Exit),
        _ => None,
    }
}

fn blank_nid(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    data
}

fn write_tip(dest: &mut [u16; 128], src: &[u16; 128]) {
    *dest = *src;
}

fn load_icon() -> HICON {
    draw_icon().unwrap_or_else(|| unsafe {
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    })
}

fn draw_icon() -> Option<HICON> {
    const SIZE: i32 = 32;
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            biHeight: -SIZE,
            biPlanes: 1,
            biBitCount: 32,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let color = unsafe { CreateDIBSection(None, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) };
    let Ok(color) = color else {
        return None;
    };
    if bits.is_null() {
        unsafe { let _ = DeleteObject(color.into()); }
        return None;
    }
    // GDI Ellipse leaves the alpha byte at 0, so the shell draws a black square.
    paint_icon_pixels(bits.cast::<u8>());

    // 1bpp AND mask, bottom-up, DWORD-aligned. 0 = opaque, 1 = transparent.
    let mut mask_bits = [0xFFu8; (SIZE as usize) * 4];
    for y in 0..SIZE {
        let src = (SIZE - 1 - y) as usize * (SIZE as usize);
        let row = unsafe { bits.cast::<u8>().add(src * 4) };
        for x in 0..SIZE {
            let alpha = unsafe { *row.add(x as usize * 4 + 3) };
            if alpha != 0 {
                let byte = (y as usize) * 4 + (x as usize) / 8;
                mask_bits[byte] &= !(0x80u8 >> (x % 8));
            }
        }
    }
    let mask = unsafe {
        CreateBitmap(
            SIZE,
            SIZE,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        )
    };
    if mask.is_invalid() {
        unsafe { let _ = DeleteObject(color.into()); }
        return None;
    }
    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    unsafe {
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
    }
    match icon {
        Ok(icon) if !icon.is_invalid() => Some(icon),
        _ => None,
    }
}

/// Straight BGRA. Only 0 and 255 alpha, so premultiplication does not matter.
fn paint_icon_pixels(bits: *mut u8) {
    const SIZE: i32 = 32;
    const FILL: [u8; 4] = [0x7A, 0x8A, 0x2E, 0xFF];
    const EYE: [u8; 4] = [0xF4, 0xF1, 0xEA, 0xFF];
    const PUPIL: [u8; 4] = [0x18, 0x16, 0x12, 0xFF];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let pixel = if (dx * dx) / (2.2 * 2.2) + (dy * dy) / (3.2 * 3.2) <= 1.0 {
                PUPIL
            } else if (dx * dx) / (8.0 * 8.0) + (dy * dy) / (5.0 * 5.0) <= 1.0 {
                EYE
            } else if dx * dx + dy * dy <= 14.0 * 14.0 {
                FILL
            } else {
                [0, 0, 0, 0]
            };
            unsafe {
                let dest = bits.add(((y * SIZE + x) * 4) as usize);
                std::ptr::copy_nonoverlapping(pixel.as_ptr(), dest, 4);
            }
        }
    }
}
