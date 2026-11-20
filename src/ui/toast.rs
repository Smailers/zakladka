//! Подсказка-тост (часть 6; P 2711–2755): фон #2B2B2B, белый текст, щелчок прячет.
//!
//! Одно окно на `Toast`: `WS_POPUP` с `WS_EX_TOPMOST | NOACTIVATE | TOOLWINDOW | LAYERED`, альфа
//! 0,96, создаётся скрытым в (−32000, −32000). Место — у точки: правее на 16 и ниже на 22, не
//! помещается снизу — над точкой (P 2748–2752). Шрифт Segoe UI 14 (маленькая — 12) × масштаб
//! монитора, поля 7 и 3 × масштаб. Скрывает себя по таймеру окна (только пока показана) и по щелчку.
//! Решение «показывать ли неважную подсказку» (`hints`) принимает владелец (`ui::host`).

use std::cell::RefCell;
use std::ptr::null_mut;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowLongPtrW, IsIconic, IsWindowVisible, KillTimer,
    SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA, HMENU,
    HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEACTIVATE,
    WM_NCDESTROY, WM_PAINT, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect, Rgb};
use crate::ui::gfx::{instance, px, register_class, Align, Canvas, Font};
use crate::win::Api;

const CLASS: &str = "ZakladkaToast";
const TIMER_HIDE: usize = 1;
const BACKGROUND: Rgb = Rgb(0x2B, 0x2B, 0x2B);
const FOREGROUND: Rgb = Rgb(0xFF, 0xFF, 0xFF);
const ALPHA: u8 = 245; // 0,96 × 255

/// Место подсказки у точки `point` в рабочей области `work` (P 2748–2754).
pub(crate) fn place(point: Point, size: (i32, i32), work: Rect) -> (i32, i32) {
    let (width, height) = size;
    let x = (point.x + 16).max(work.l).min(work.r - width);
    let mut y = point.y + 22;
    if y + height > work.b {
        y = point.y - height - 12;
    }
    (x, y.max(work.t))
}

/// Строки текста (как многострочная надпись Tk): по `\n`, без `\r`.
pub(crate) fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()
}

/// Секунды показа -> миллисекунды таймера (не меньше 1 мс, не больше 10 минут).
pub(crate) fn timer_ms(seconds: f64) -> u32 {
    let ms = (seconds * 1000.0).round();
    if ms.is_nan() {
        1
    } else {
        ms.clamp(1.0, 600_000.0) as u32
    }
}

/// (высота шрифта, поля по бокам, поля сверху и снизу) при масштабе `scale`.
pub(crate) fn metrics(small: bool, scale: f64) -> (i32, i32, i32) {
    (px(if small { 12.0 } else { 14.0 }, scale), px(7.0, scale), px(3.0, scale))
}

#[derive(Default)]
struct State {
    hwnd: Hwnd,
    lines: Vec<String>,
    font: Option<Font>,
    font_px: i32,
    line_height: i32,
    pad: (i32, i32),
    size: (i32, i32),
    shown: Option<Rect>, // место показанной подсказки; None — скрыта
    probe: Option<Canvas>,
}

pub struct Toast {
    api: Arc<Api>,
    state: Rc<RefCell<State>>,
}

impl Toast {
    /// Создаёт скрытое окно подсказки (заранее, как `make_piece("toast")` в Python).
    pub fn new(api: Arc<Api>) -> Toast {
        let state = Rc::new(RefCell::new(State::default()));
        let hwnd = create_window(&state);
        state.borrow_mut().hwnd = hwnd;
        Toast { api, state }
    }

    /// Окно подсказки (0 — не создалось).
    pub fn hwnd(&self) -> Hwnd {
        self.state.try_borrow().map_or(0, |s| s.hwnd)
    }

    pub fn is_shown(&self) -> bool {
        self.state.try_borrow().is_ok_and(|s| s.shown.is_some())
    }

    /// Показать `text` у точки `at` (нет — у указателя) на `seconds` секунд.
    pub fn show(&mut self, text: &str, at: Option<Point>, seconds: f64, small: bool) {
        let point = at.unwrap_or_else(|| self.api.cursor());
        let scale = self.api.dpi_scale_at(point);
        let (font_px, padx, pady) = metrics(small, scale);
        let size = {
            let Ok(mut guard) = self.state.try_borrow_mut() else { return };
            let s = &mut *guard;
            if s.hwnd == 0 {
                return;
            }
            if s.font.is_none() || s.font_px != font_px {
                s.font = Some(Font::new("Segoe UI", font_px, false));
                s.font_px = font_px;
            }
            let probe = s.probe.get_or_insert_with(|| Canvas::new(1, 1));
            let (Some(font), probe) = (s.font.as_ref(), &*probe) else { return };
            s.lines = lines(text);
            let mut width = 0;
            let mut line_height = 0;
            for line in &s.lines {
                let (w, h) = probe.measure(line, font);
                width = width.max(w);
                line_height = line_height.max(h);
            }
            s.line_height = line_height;
            s.pad = (padx, pady);
            s.size = ((width + 2 * padx).max(1), (line_height * s.lines.len() as i32 + 2 * pady).max(1));
            s.size
        };
        let (x, y) = place(point, size, self.api.work_area(point));
        let geometry = Rect { l: x, t: y, r: x + size.0, b: y + size.1 };
        let hwnd = {
            let Ok(mut s) = self.state.try_borrow_mut() else { return };
            s.shown = Some(geometry);
            s.hwnd
        };
        // SAFETY: окно своё; показ без активации; таймер умирает вместе с окном.
        unsafe {
            let window = to_hwnd(hwnd);
            if IsIconic(window).as_bool() {
                // Что-то свернуло наше окно (Win+M): вернуть без активации (P 2668–2671).
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            }
            let _ = SetWindowPos(window, HWND_TOPMOST, x, y, size.0, size.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = InvalidateRect(window, None, BOOL(0));
            SetTimer(window, TIMER_HIDE, timer_ms(seconds), None);
        }
    }

    pub fn hide(&mut self) {
        hide(&self.state);
    }

    /// Чужая программа свернула или показала наше окно — вернуть его состояние (P 2711–2722).
    pub fn guard(&self) {
        let (hwnd, shown) = match self.state.try_borrow() {
            Ok(s) => (s.hwnd, s.shown),
            Err(_) => return,
        };
        if hwnd == 0 {
            return;
        }
        // SAFETY: окно своё.
        unsafe {
            let window = to_hwnd(hwnd);
            match shown {
                Some(g) if IsIconic(window).as_bool() => {
                    let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
                    let _ = SetWindowPos(
                        window,
                        HWND_TOPMOST,
                        g.l,
                        g.t,
                        g.r - g.l,
                        g.b - g.t,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None if IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() => hide(&self.state),
                _ => {}
            }
        }
    }
}

impl Drop for Toast {
    fn drop(&mut self) {
        let hwnd = match self.state.try_borrow_mut() {
            Ok(mut s) => {
                s.shown = None;
                std::mem::take(&mut s.hwnd)
            }
            Err(_) => return,
        };
        if hwnd != 0 {
            // SAFETY: окно своё; WM_NCDESTROY освобождает привязку к состоянию.
            unsafe {
                let _ = DestroyWindow(to_hwnd(hwnd));
            }
        }
    }
}

fn create_window(state: &Rc<RefCell<State>>) -> Hwnd {
    let atom = register_class(CLASS, toast_proc, false);
    if atom == 0 {
        return 0;
    }
    // SAFETY: класс зарегистрирован; окно скрыто за экраном; альфа ставится до показа; привязка
    // (Box<Weak>) снимается в WM_NCDESTROY.
    unsafe {
        let created = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            PCWSTR(atom as usize as *const u16),
            w!("Закладка"),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            HWND(null_mut()),
            HMENU(null_mut()),
            instance(),
            None,
        );
        let Ok(window) = created else { return 0 };
        if window.0.is_null() {
            return 0;
        }
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), ALPHA, LWA_ALPHA);
        let link = Box::into_raw(Box::new(Rc::downgrade(state)));
        SetWindowLongPtrW(window, GWLP_USERDATA, link as isize);
        from_hwnd(window)
    }
}

fn hide(state: &Rc<RefCell<State>>) {
    let hwnd = match state.try_borrow_mut() {
        Ok(mut s) => {
            s.shown = None;
            s.hwnd
        }
        Err(_) => return,
    };
    if hwnd == 0 {
        return;
    }
    // SAFETY: окно своё.
    unsafe {
        let window = to_hwnd(hwnd);
        let _ = KillTimer(window, TIMER_HIDE);
        if IsWindowVisible(window).as_bool() {
            let _ = SetWindowPos(
                window,
                HWND(null_mut()),
                0,
                0,
                0,
                0,
                SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
    }
}

fn render(s: &State) -> Option<Canvas> {
    let font = s.font.as_ref()?;
    let (w, h) = s.size;
    let mut canvas = Canvas::new(w, h);
    canvas.fill(Rect { l: 0, t: 0, r: w, b: h }, BACKGROUND);
    let (padx, pady) = s.pad;
    for (i, line) in s.lines.iter().enumerate() {
        let top = pady + i as i32 * s.line_height;
        let r = Rect { l: padx, t: top, r: w - padx, b: top + s.line_height };
        canvas.text(r, line, font, FOREGROUND, Align::Center);
    }
    Some(canvas)
}

fn paint(hwnd: Hwnd, state: &Rc<RefCell<State>>) {
    let window = to_hwnd(hwnd);
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: обработка WM_PAINT своего окна.
    unsafe {
        let dc = BeginPaint(window, &mut ps);
        if let Ok(s) = state.try_borrow() {
            if let Some(canvas) = render(&s) {
                canvas.present(dc.0 as isize, 0, 0);
            }
        }
        let _ = EndPaint(window, &ps);
    }
}

fn toast_proc(hwnd: Hwnd, msg: u32, wparam: usize, _lparam: isize) -> Option<isize> {
    let window = to_hwnd(hwnd);
    match msg {
        WM_MOUSEACTIVATE => return Some(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return Some(1),
        WM_NCDESTROY => {
            // SAFETY: привязку ставит create_window (Box<Weak>), снимаем один раз.
            unsafe {
                let link = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut Weak<RefCell<State>>;
                if !link.is_null() {
                    drop(Box::from_raw(link));
                }
            }
            return None;
        }
        _ => {}
    }
    // SAFETY: в GWLP_USERDATA только наш Box<Weak> (или 0 до привязки).
    let state = unsafe {
        let link = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Weak<RefCell<State>>;
        if link.is_null() {
            return None;
        }
        (*link).upgrade()?
    };
    match msg {
        WM_PAINT => {
            paint(hwnd, &state);
            Some(0)
        }
        WM_LBUTTONDOWN => {
            hide(&state);
            Some(0)
        }
        WM_TIMER if wparam == TIMER_HIDE => {
            hide(&state);
            Some(0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { l: 0, t: 0, r: 1920, b: 1080 };

    #[test]
    fn place_below_right_of_point() {
        assert_eq!(place(Point { x: 100, y: 100 }, (80, 20), WORK), (116, 122));
    }

    #[test]
    fn place_stays_inside_work_area() {
        // справа не влезает — к правому краю
        assert_eq!(place(Point { x: 1900, y: 100 }, (80, 20), WORK), (1840, 122));
        // снизу не влезает — над точкой
        assert_eq!(place(Point { x: 100, y: 1070 }, (80, 20), WORK), (116, 1038));
        // слева — не левее рабочей области
        assert_eq!(place(Point { x: -50, y: 100 }, (80, 20), WORK), (0, 122));
        // низкая область: и снизу, и сверху не влезает — к верхнему краю
        let low = Rect { l: 0, t: 0, r: 500, b: 40 };
        assert_eq!(place(Point { x: 10, y: 5 }, (60, 30), low), (26, 0));
        // второй монитор слева (отрицательные координаты)
        let left = Rect { l: -1920, t: 0, r: 0, b: 1040 };
        assert_eq!(place(Point { x: -100, y: 500 }, (200, 26), left), (-200, 522));
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(metrics(false, 1.0), (14, 7, 3));
        assert_eq!(metrics(true, 1.0), (12, 7, 3));
        assert_eq!(metrics(false, 1.5), (21, 10, 4)); // round(10.5) == 10, round(4.5) == 4
        assert_eq!(metrics(true, 2.0), (24, 14, 6));
        assert_eq!(timer_ms(1.4), 1400);
        assert_eq!(timer_ms(2.5), 2500);
        assert_eq!(timer_ms(3.0), 3000);
        assert_eq!(timer_ms(0.0), 1);
        assert_eq!(timer_ms(f64::NAN), 1);
        assert_eq!(lines("Поставлена"), vec!["Поставлена"]);
        assert_eq!(lines("а\r\nб"), vec!["а", "б"]);
    }

    #[test]
    fn toast_window_is_created_hidden() {
        let mut toast = Toast::new(Arc::new(Api::new()));
        let hwnd = toast.hwnd();
        assert_ne!(hwnd, 0);
        assert!(!toast.is_shown());
        // SAFETY: чтение состояния своего окна.
        assert!(!unsafe { IsWindowVisible(to_hwnd(hwnd)) }.as_bool());
        toast.hide(); // скрытую спрятать — безвредно
        toast.guard();
        assert!(!toast.is_shown());
        // нарисовать без показа: размер и цвета холста
        if let Ok(mut s) = toast.state.try_borrow_mut() {
            s.font = Some(Font::new("Segoe UI", 14, false));
            s.lines = lines("Снята");
            s.line_height = 19;
            s.pad = (7, 3);
            s.size = (60, 25);
        }
        let canvas = toast.state.try_borrow().ok().and_then(|s| render(&s));
        let canvas = canvas.expect("холст");
        assert_eq!(canvas.size(), (60, 25));
        assert_eq!(canvas.pixel(1, 1), Some(BACKGROUND));
        drop(toast);
        // SAFETY: окно уничтожено — проверка дескриптора.
        assert!(!unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(to_hwnd(hwnd)) }.as_bool());
    }
}
