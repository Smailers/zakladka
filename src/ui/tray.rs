//! Значок в трее (часть 6; P 2554–2557, 2758–2761, 2916–2918, 3096–3100): NIF_MESSAGE | NIF_ICON |
//! NIF_TIP, `WM_APP_TRAY`, `TaskbarCreated`. Публичного интерфейса в плане нет — на усмотрение части 6.
//!
//! Значок добавляется на окно-владельца с `uID = 1`; отпускание левой или правой кнопки приходит
//! владельцу сообщением `WM_APP_TRAY` (`lparam` = сообщение мыши). После перезапуска Проводника
//! система рассылает `TaskbarCreated` — тогда значок добавляется заново (`add`).

use std::ptr::null_mut;

use windows::core::w;
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, LoadIconW, RegisterWindowMessageW, HICON, IDI_INFORMATION, WM_LBUTTONUP, WM_RBUTTONUP,
};

use crate::core::text::APP_TITLE;
use crate::types::{to_hwnd, Hwnd, WM_APP_TRAY};

/// Номер значка у окна-владельца (как `uID=1` в Python).
const TRAY_ID: u32 = 1;
/// `szTip` — 128 символов UTF-16 вместе с завершающим нулём.
const TIP_CAPACITY: usize = 128;

/// Подсказка значка: «Закладка — Ctrl+Alt+Z», у dev-профиля «Закладка (новая) — Ctrl+Alt+X».
pub fn tip_text(profile_suffix: &str, hotkey: &str) -> String {
    format!("{APP_TITLE}{profile_suffix} — {hotkey}")
}

/// Сообщение мыши от значка открывает панель: отпускание левой или правой кнопки (P 2577).
pub fn opens_panel(lparam: isize) -> bool {
    let mouse = (lparam & 0xFFFF) as u32;
    mouse == WM_LBUTTONUP || mouse == WM_RBUTTONUP
}

/// `szTip`: не больше 127 единиц UTF-16 и ноль; суррогатная пара не разрезается.
fn tip_buffer(tip: &str) -> [u16; TIP_CAPACITY] {
    let mut buffer = [0u16; TIP_CAPACITY];
    let mut used = 0;
    for c in tip.chars() {
        let mut units = [0u16; 2];
        let encoded = c.encode_utf16(&mut units);
        if used + encoded.len() > TIP_CAPACITY - 1 {
            break;
        }
        buffer[used..used + encoded.len()].copy_from_slice(encoded);
        used += encoded.len();
    }
    buffer
}

/// Значок в трее одного окна-владельца.
pub struct Tray {
    owner: Hwnd,
    icon: isize,      // HICON; 0 — нет
    owned_icon: bool, // наш значок (DestroyIcon), а не системный
    tip: String,
    added: bool,
    taskbar_created: u32,
}

impl Tray {
    /// Ничего не показывает: только готовит данные и узнаёт номер `TaskbarCreated`.
    /// `icon` — HICON (`Api::make_icon`), свой; `None` — системный значок «сведения».
    pub fn new(owner: Hwnd, icon: Option<isize>, tip: &str) -> Tray {
        // SAFETY: строка-литерал с нулём.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let mut tray = Tray { owner, icon: 0, owned_icon: false, tip: tip.to_string(), added: false, taskbar_created };
        tray.take_icon(icon);
        tray
    }

    /// Номер сообщения `TaskbarCreated` (0 — не удалось зарегистрировать).
    pub fn taskbar_created(&self) -> u32 {
        self.taskbar_created
    }

    pub fn is_added(&self) -> bool {
        self.added
    }

    /// Добавить значок (и после `TaskbarCreated`). Если система говорит, что он уже есть, —
    /// обновить его. `true` — значок в трее.
    pub fn add(&mut self) -> bool {
        self.added = self.notify(NIM_ADD) || self.notify(NIM_MODIFY);
        self.added
    }

    /// Убрать значок (выход).
    pub fn remove(&mut self) {
        if self.added {
            self.notify(NIM_DELETE);
            self.added = false;
        }
    }

    /// Новая подсказка (после смены клавиш).
    pub fn set_tip(&mut self, tip: &str) {
        self.tip = tip.to_string();
        if self.added {
            self.notify(NIM_MODIFY);
        }
    }

    /// Новый значок (после смены акцента). `None` — оставить прежний (как `if icon:` в Python).
    pub fn set_icon(&mut self, icon: Option<isize>) {
        if icon.is_none() {
            return;
        }
        let (old, old_owned) = (self.icon, self.owned_icon);
        self.take_icon(icon);
        if self.added {
            self.notify(NIM_MODIFY);
        }
        if old_owned && old != 0 && old != self.icon {
            // SAFETY: значок создан нами (make_icon) и в трее уже заменён.
            unsafe {
                let _ = DestroyIcon(HICON(old as *mut std::ffi::c_void));
            }
        }
    }

    fn take_icon(&mut self, icon: Option<isize>) {
        match icon.filter(|&h| h != 0) {
            Some(h) => {
                self.icon = h;
                self.owned_icon = true;
            }
            None => {
                // SAFETY: системный значок; освобождать не нужно.
                let system = unsafe { LoadIconW(HINSTANCE(null_mut()), IDI_INFORMATION) };
                self.icon = system.map(|h| h.0 as isize).unwrap_or(0);
                self.owned_icon = false;
            }
        }
    }

    fn notify(&self, message: NOTIFY_ICON_MESSAGE) -> bool {
        if self.owner == 0 {
            return false;
        }
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: to_hwnd(self.owner),
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: HICON(self.icon as *mut std::ffi::c_void),
            szTip: tip_buffer(&self.tip),
            ..Default::default()
        };
        // SAFETY: структура локальная и заполнена; размер указан в cbSize.
        unsafe { Shell_NotifyIconW(message, &data).as_bool() }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if self.owned_icon && self.icon != 0 {
            // SAFETY: значок создан нами; из трея уже убран.
            unsafe {
                let _ = DestroyIcon(HICON(self.icon as *mut std::ffi::c_void));
            }
            self.icon = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip_has_profile_suffix_and_hotkey() {
        assert_eq!(tip_text("", "Ctrl+Alt+Z"), "Закладка — Ctrl+Alt+Z");
        assert_eq!(tip_text(crate::app_id::DEV.tray_suffix, "Ctrl+Alt+X"), "Закладка (новая) — Ctrl+Alt+X");
    }

    #[test]
    fn tip_buffer_is_cut_and_terminated() {
        let short = tip_buffer("Закладка");
        assert_eq!(String::from_utf16_lossy(&short[..8]), "Закладка");
        assert_eq!(short[8], 0);
        let long = tip_buffer(&"я".repeat(300));
        assert_eq!(long[126], 'я' as u16);
        assert_eq!(long[127], 0);
        // суррогатная пара на границе не режется: 126 «я» + эмодзи (2 единицы) не влезают
        let mut text = "я".repeat(126);
        text.push('😀');
        let cut = tip_buffer(&text);
        assert_eq!(cut[125], 'я' as u16);
        assert_eq!(cut[126], 0);
    }

    #[test]
    fn release_of_either_button_opens_panel() {
        assert!(opens_panel(0x0202));
        assert!(opens_panel(0x0205));
        assert!(opens_panel(0x0001_0202)); // старшее слово не важно
        assert!(!opens_panel(0x0201));
        assert!(!opens_panel(0x0200));
        assert!(!opens_panel(0x0203));
    }

    #[test]
    fn tray_without_owner_touches_nothing() {
        let mut tray = Tray::new(0, None, "Закладка — тест");
        assert!(!tray.add());
        assert!(!tray.is_added());
        tray.set_tip("другая");
        tray.remove();
        assert_ne!(tray.taskbar_created(), 0);
    }
}
