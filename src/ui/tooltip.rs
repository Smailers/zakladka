//! Всплывающие пояснения (часть 7; N 379–520): задержка, место, одно окно на всех.
//!
//! Окно одно на UI-поток, создаётся при первом пояснении: никогда не активируется, прозрачно для
//! мыши, поверх всех, слоёное с альфой 0,97 (пипетка его не снимает). Показ — через 400 мс, если
//! указатель всё ещё над окном-владельцем; место — у указателя в момент показа.

use std::cell::RefCell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    PAINTSTRUCT,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetAncestor, GetCursorPos, IsWindow, IsWindowVisible, KillTimer, SetLayeredWindowAttributes,
    SetTimer, SetWindowPos, WindowFromPoint, GA_ROOT, HMENU, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE,
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_DESTROY, WM_ERASEBKGND,
    WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::types::{to_hwnd, Hwnd, Point, Rect, Rgb};
use crate::ui::gfx::{instance, px, register_class, Canvas, Font};

/// Задержка показа, мс (N 460).
pub const DELAY_MS: u32 = 400;

const CLASS: &str = "ZakladkaTip";
const TIMER_SHOW: usize = 1;
const ALPHA: u8 = 247; // 0,97 × 255

/// Без состояния у вызывающего: одно окно пояснений на UI-поток (состояние — внутри модуля).
pub struct Tip;

impl Tip {
    /// Показать `text` через 400 мс, если указатель тогда всё ещё над окном `owner` (его корневым
    /// окном). `at` — точка указателя сейчас; при показе берётся точка указателя в тот момент
    /// (как `winfo_pointerxy` в N 509), `at` — запасная. `light` — тема пояснения.
    /// Повторный вызов заменяет ожидающее пояснение.
    pub fn schedule(owner: Hwnd, text: String, at: Point, light: bool) {
        if owner == 0 || text.is_empty() {
            return;
        }
        with_state(|tip| {
            if !tip.ensure_window() {
                return;
            }
            tip.pending = Some(Pending { owner, text, at, light });
            // SAFETY: окно своё; повторный SetTimer с тем же номером перезапускает отсчёт.
            unsafe {
                SetTimer(to_hwnd(tip.hwnd), TIMER_SHOW, DELAY_MS, None);
            }
        });
    }

    /// Спрятать пояснение и отменить ожидающее — только если они от `owner`; `owner = 0` — любые
    /// (как `_TipWindow.hide(None)`).
    pub fn hide(owner: Hwnd) {
        with_state(|tip| tip.hide(owner));
    }
}

struct Pending {
    owner: Hwnd,
    text: String,
    at: Point,
    light: bool,
}

#[derive(Default)]
struct TipState {
    hwnd: Hwnd,
    owner: Hwnd, // чьё пояснение показано; 0 — скрыто
    pending: Option<Pending>,
    text: String,
    light: bool,
    scale: f64,
    font: Option<Font>,
    size: (i32, i32),
}

thread_local! {
    static TIP: RefCell<TipState> = RefCell::new(TipState::default());
}

/// Состояние занято (сообщение пришло изнутри вызова Win32 отсюда же) или поток завершается —
/// ничего не делать.
fn with_state<R>(f: impl FnOnce(&mut TipState) -> R) -> Option<R> {
    TIP.try_with(|cell| cell.try_borrow_mut().ok().map(|mut state| f(&mut state))).ok().flatten()
}

impl TipState {
    fn ensure_window(&mut self) -> bool {
        // SAFETY: проверка дескриптора своего окна.
        if self.hwnd != 0 && unsafe { IsWindow(to_hwnd(self.hwnd)).as_bool() } {
            return true;
        }
        self.hwnd = 0;
        self.owner = 0;
        let atom = register_class(CLASS, tip_proc, false);
        if atom == 0 {
            return false;
        }
        // SAFETY: класс зарегистрирован; окно создаётся скрытым за экраном.
        unsafe {
            let created = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                PCWSTR(atom as usize as *const u16),
                w!("Закладка"),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                instance(),
                None,
            );
            let Ok(window) = created else { return false };
            if window.0.is_null() {
                return false;
            }
            let _ = SetLayeredWindowAttributes(window, COLORREF(0), ALPHA, LWA_ALPHA);
            self.hwnd = window.0 as isize;
        }
        true
    }

    fn hide(&mut self, owner: Hwnd) {
        if self.pending.as_ref().is_some_and(|p| owner == 0 || p.owner == owner) {
            self.pending = None;
            if self.hwnd != 0 {
                // SAFETY: окно своё.
                unsafe {
                    let _ = KillTimer(to_hwnd(self.hwnd), TIMER_SHOW);
                }
            }
        }
        if self.owner != 0 && (owner == 0 || self.owner == owner) {
            self.owner = 0;
            if self.hwnd != 0 {
                // SAFETY: окно своё.
                unsafe {
                    let _ = SetWindowPos(
                        to_hwnd(self.hwnd),
                        HWND(std::ptr::null_mut()),
                        0,
                        0,
                        0,
                        0,
                        SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                    );
                }
            }
        }
    }

    /// Истекла задержка: показать, если указатель всё ещё над владельцем (N 504–517).
    fn fire(&mut self) {
        let Some(pending) = self.pending.take() else { return };
        let cursor = cursor_pos().unwrap_or(pending.at);
        if !over_owner(pending.owner, cursor) {
            return;
        }
        let scale = scale_at(cursor);
        if self.font.is_none() || self.scale != scale {
            self.font = Some(Font::new("Segoe UI", px(12.0, scale), false));
            self.scale = scale;
        }
        let Some(font) = self.font.as_ref() else { return };
        let text_size = Canvas::new(1, 1).measure_wrapped(&pending.text, font, px(340.0, scale));
        let size = tip_size(text_size, scale);
        let (x, y) = place(cursor, size, work_area(cursor), scale);
        self.text = pending.text;
        self.light = pending.light;
        self.size = size;
        self.owner = pending.owner;
        // SAFETY: окно своё; показ без активации.
        unsafe {
            let window = to_hwnd(self.hwnd);
            let _ = SetWindowPos(window, HWND_TOPMOST, x, y, size.0, size.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = InvalidateRect(window, None, BOOL(0));
        }
    }

    fn paint(&mut self, hwnd: Hwnd) {
        let window = to_hwnd(hwnd);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: обработка WM_PAINT своего окна.
        unsafe {
            let dc = BeginPaint(window, &mut ps);
            if let Some(font) = self.font.as_ref() {
                let canvas = render(&self.text, font, self.light, self.size, self.scale);
                canvas.present(dc.0 as isize, 0, 0);
            }
            let _ = EndPaint(window, &ps);
        }
    }
}

/// (фон, текст, рамка) пояснения (N 424–425).
fn colors(light: bool) -> (Rgb, Rgb, Rgb) {
    if light {
        (Rgb(0xFF, 0xFF, 0xFF), Rgb(0x1A, 0x1A, 0x1A), Rgb(0xC4, 0xC4, 0xC4))
    } else {
        (Rgb(0x32, 0x32, 0x32), Rgb(0xF3, 0xF3, 0xF3), Rgb(0x55, 0x55, 0x55))
    }
}

/// Размер окна: текст + поля 8 и 4 + рамка 1 (N 426–430).
fn tip_size(text: (i32, i32), scale: f64) -> (i32, i32) {
    (text.0 + 2 * px(8.0, scale) + 2, text.1 + 2 * px(4.0, scale) + 2)
}

/// Место у указателя (N 431–436): правее на 12 и ниже на 22; не влезает снизу — над указателем;
/// не выходит за рабочую область.
fn place(point: Point, size: (i32, i32), work: Rect, scale: f64) -> (i32, i32) {
    let (width, height) = size;
    let x = (point.x + px(12.0, scale)).max(work.l).min(work.r - width);
    let mut y = point.y + px(22.0, scale);
    if y + height > work.b {
        y = point.y - height - px(10.0, scale);
    }
    (x, y.max(work.t))
}

fn render(text: &str, font: &Font, light: bool, size: (i32, i32), scale: f64) -> Canvas {
    let (background, foreground, border) = colors(light);
    let (w, h) = size;
    let mut canvas = Canvas::new(w, h);
    canvas.fill(Rect { l: 0, t: 0, r: w, b: h }, border);
    canvas.fill(Rect { l: 1, t: 1, r: w - 1, b: h - 1 }, background);
    let (padx, pady) = (px(8.0, scale), px(4.0, scale));
    canvas.text_wrapped(Rect { l: 1 + padx, t: 1 + pady, r: w - 1 - padx, b: h - 1 - pady }, text, font, foreground);
    canvas
}

fn cursor_pos() -> Option<Point> {
    let mut point = POINT::default();
    // SAFETY: точка локальная.
    unsafe { GetCursorPos(&mut point).ok().map(|_| point.into()) }
}

/// Указатель над видимым окном `owner` (сравнение корневых окон).
fn over_owner(owner: Hwnd, cursor: Point) -> bool {
    // SAFETY: только чтение состояния окон.
    unsafe {
        let owner = to_hwnd(owner);
        if !IsWindowVisible(owner).as_bool() {
            return false;
        }
        let under = WindowFromPoint(cursor.into());
        !under.0.is_null() && GetAncestor(under, GA_ROOT) == GetAncestor(owner, GA_ROOT)
    }
}

/// Масштаб монитора с точкой (N 270–275): 1…4. Приватно, потому что у `Tip` нет `Api`.
fn scale_at(p: Point) -> f64 {
    let (mut x, mut y) = (96u32, 96u32);
    // SAFETY: только чтение; при ошибке — 96 DPI.
    let ok = unsafe {
        let monitor = MonitorFromPoint(p.into(), MONITOR_DEFAULTTONEAREST);
        !monitor.0.is_null() && GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok()
    };
    if ok && x > 0 {
        (x as f64 / 96.0).clamp(1.0, 4.0)
    } else {
        1.0
    }
}

/// Рабочая область монитора с точкой (N 207–213).
fn work_area(p: Point) -> Rect {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: структура локальная, размер задан.
    let ok = unsafe {
        let monitor = MonitorFromPoint(p.into(), MONITOR_DEFAULTTONEAREST);
        !monitor.0.is_null() && GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    if ok {
        info.rcWork.into()
    } else {
        Rect { l: 0, t: 0, r: 1920, b: 1080 }
    }
}

fn tip_proc(hwnd: Hwnd, msg: u32, wparam: usize, _lparam: isize) -> Option<isize> {
    match msg {
        WM_NCHITTEST => Some(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => Some(1),
        WM_TIMER if wparam == TIMER_SHOW => {
            // SAFETY: окно своё.
            unsafe {
                let _ = KillTimer(to_hwnd(hwnd), TIMER_SHOW);
            }
            with_state(|tip| {
                if tip.hwnd == hwnd {
                    tip.fire();
                }
            });
            Some(0)
        }
        // Занято — None: DefWindowProc сам подтвердит перерисовку.
        WM_PAINT => with_state(|tip| tip.paint(hwnd)).map(|_| 0),
        WM_DESTROY => {
            with_state(|tip| {
                if tip.hwnd == hwnd {
                    tip.hwnd = 0;
                    tip.owner = 0;
                    tip.pending = None;
                }
            });
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { l: 0, t: 0, r: 1000, b: 800 };

    #[test]
    fn place_below_right_of_pointer() {
        assert_eq!(place(Point { x: 100, y: 100 }, (200, 30), WORK, 1.0), (112, 122));
        assert_eq!(place(Point { x: 100, y: 100 }, (200, 30), WORK, 1.5), (118, 133));
    }

    #[test]
    fn place_flips_up_and_stays_inside() {
        // снизу не влезает — над указателем; справа не влезает — к правому краю
        assert_eq!(place(Point { x: 950, y: 790 }, (200, 30), WORK, 1.0), (800, 750));
        // слева — не левее рабочей области, сверху — не выше
        let work = Rect { l: -1920, t: -50, r: 0, b: 0 };
        assert_eq!(place(Point { x: -1930, y: -40 }, (100, 60), work, 1.0), (-1918, -50));
    }

    #[test]
    fn size_and_render() {
        assert_eq!(tip_size((100, 16), 1.0), (118, 26));
        assert_eq!(tip_size((100, 16), 2.0), (134, 34));
        let font = Font::new("Segoe UI", 12, false);
        let (w, h) = tip_size(Canvas::new(1, 1).measure_wrapped("Перейти к закладке", &font, 340), 1.0);
        let canvas = render("Перейти к закладке", &font, false, (w, h), 1.0);
        assert_eq!(canvas.pixel(0, 0), Some(Rgb(0x55, 0x55, 0x55)));
        assert_eq!(canvas.pixel(2, 2), Some(Rgb(0x32, 0x32, 0x32)));
    }

    #[test]
    fn long_hint_wraps_at_340() {
        let font = Font::new("Segoe UI", 12, false);
        let text = crate::core::text::CLOSED_HINT;
        let one = Canvas::new(1, 1).measure(text, &font);
        let (w, h) = Canvas::new(1, 1).measure_wrapped(text, &font, 340);
        assert!(one.0 > 340 && w <= 340 && h >= 2 * one.1, "{one:?} -> {w}x{h}");
    }

    #[test]
    fn hide_without_window_is_harmless() {
        Tip::hide(0);
        Tip::hide(12345);
    }

    /// Картинки пояснений для сверки вида: `cargo test --lib tip_bmp -- --ignored`.
    #[test]
    #[ignore]
    fn tip_bmp() {
        let dir = std::env::temp_dir().join("zakladka-part7");
        std::fs::create_dir_all(&dir).unwrap();
        for (light, scale) in [(true, 1.0), (false, 1.0), (true, 1.5)] {
            let font = Font::new("Segoe UI", px(12.0, scale), false);
            let text = crate::core::text::CLOSED_HINT;
            let size = tip_size(Canvas::new(1, 1).measure_wrapped(text, &font, px(340.0, scale)), scale);
            let name = format!("tip-{}-{}.bmp", if light { "light" } else { "dark" }, scale);
            render(text, &font, light, size, scale).save_bmp(&dir.join(&name)).unwrap();
            println!("{}", dir.join(name).display());
        }
    }
}
