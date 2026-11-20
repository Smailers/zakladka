//! Окно клавиш (часть 6; P 2846–2919). Публичного интерфейса в плане нет — на усмотрение части 6.
//!
//! Обычное окно с заголовком «Закладка — сочетания клавиш», поверх всех, без изменения размера,
//! по центру рабочей области монитора с указателем. Два поля: щелчок выбирает поле, нажатое
//! сочетание записывается словами «Ctrl+Alt+Z» по виртуальным кодам (от раскладки не зависит);
//! печатать в поле нельзя, как в Python (там поле отвечает "break" на любую клавишу). Проверки —
//! «два модификатора, один из них Alt или Win» и «сочетания должны различаться», тексты из Python.
//! Сверх Python: Esc — «Отмена», Enter — «Сохранить», Tab — следующее поле (эти клавиши без
//! модификаторов сочетанием быть не могут).
//!
//! Окно одно на UI-поток (состояние внутри модуля); всё рисуется через `ui::gfx`. Итог —
//! `on_done(Some((mark, extra)))` после «Сохранить» или `on_done(None)` после «Отмена» / закрытия;
//! вызывается, когда окно уже уничтожено и состояние отпущено.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null_mut;
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, RECT};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_ESCAPE, VK_F10, VK_F4,
    VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, IsIconic, IsWindow, SetForegroundWindow, ShowWindow, HMENU, SW_RESTORE,
    SW_SHOW, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_CAPTION, WS_EX_DLGMODALFRAME, WS_EX_TOPMOST,
    WS_SYSMENU,
};

use crate::core::hotkey::{combo_text, safe_hotkey};
use crate::core::text::APP_TITLE;
use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect, Rgb};
use crate::ui::gfx::{
    dark_title_bar, draw_button, instance, light_theme, palette, px, register_class, wide, Align, ButtonSpec, Canvas,
    Font,
};
use crate::win::Api;

/// В crate `windows` константа лежит в `Win32_UI_Controls` (feature не включён).
const WM_MOUSELEAVE: u32 = 0x02A3;

const CLASS: &str = "ZakladkaHotkeys";
pub const LABELS: [&str; 2] =
    ["Поставить или перенести закладку\n(без выделения — перейти к ней)", "Дополнительная закладка\n(старая остаётся)"];
pub const HINT: &str = "Щёлкните в поле и нажмите новое сочетание: два модификатора,\n\
                        один из них Alt или Win (например Ctrl+Alt+Z), и клавиша.";
pub const ERROR_FORMAT: &str = "Нужно два модификатора, один из них Alt или Win, и клавиша.";
pub const ERROR_SAME: &str = "Сочетания должны различаться.";
const SAVE: &str = "Сохранить";
const CANCEL: &str = "Отмена";

/// Итог окна: `Some((mark, extra))` — сохранить, `None` — отмена.
pub type OnDone = Box<dyn FnMut(Option<(String, String)>)>;

/// Проверка перед сохранением (P 2883–2893): оба сочетания безопасны и различаются.
/// Возвращает значения без пробелов по краям.
pub fn validate(mark: &str, extra: &str) -> Result<(String, String), &'static str> {
    let (mark, extra) = (mark.trim(), extra.trim());
    let (Ok(first), Ok(second)) = (safe_hotkey(mark), safe_hotkey(extra)) else {
        return Err(ERROR_FORMAT);
    };
    if first == second {
        return Err(ERROR_SAME);
    }
    Ok((mark.to_string(), extra.to_string()))
}

/// Нажатое сочетание словами (P 2834–2844): клавиша и хотя бы один модификатор, иначе `None`.
pub fn capture(vk: u32, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<String> {
    combo_text(ctrl, alt, shift, win, vk)
}

/// Окно уже открыто.
pub fn is_open() -> bool {
    // SAFETY: проверка дескриптора своего окна.
    with_state(|d| d.hwnd != 0 && unsafe { IsWindow(to_hwnd(d.hwnd)).as_bool() }).unwrap_or(false)
}

/// Поднять открытое окно (P 2848–2851).
pub fn raise() {
    let hwnd = with_state(|d| d.hwnd).unwrap_or(0);
    if hwnd == 0 {
        return;
    }
    // SAFETY: окно своё.
    unsafe {
        let window = to_hwnd(hwnd);
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(window);
    }
}

/// Открыть окно со значениями `mark`, `extra`. Уже открыто — поднять его (`on_done` прежний).
/// `false` — окно не создалось; тогда `on_done` не вызывается.
pub fn open(api: Arc<Api>, mark: &str, extra: &str, on_done: OnDone) -> bool {
    if is_open() {
        raise();
        return true;
    }
    let cursor = api.cursor();
    let scale = api.dpi_scale_at(cursor);
    let scale = if scale.is_finite() { scale.clamp(1.0, 4.0) } else { 1.0 };
    let light = light_theme(false);
    let big = Font::new("Segoe UI", px(11.0 * 96.0 / 72.0, scale), false); // 11 pt, как Tk ("Segoe UI", 11)
    let small = Font::new("Segoe UI", px(9.0 * 96.0 / 72.0, scale), false);
    let layout = layout(&measure_metrics(&big, &small), scale);

    let style = WS_CAPTION | WS_SYSMENU;
    let ex = WS_EX_TOPMOST | WS_EX_DLGMODALFRAME;
    let mut frame = RECT { left: 0, top: 0, right: layout.size.0, bottom: layout.size.1 };
    let dpi = (scale * 96.0).round() as u32;
    // SAFETY: структура локальная.
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut frame, style, BOOL(0), ex, dpi);
    }
    let size = (frame.right - frame.left, frame.bottom - frame.top);
    let (x, y) = center(api.work_area(cursor), size);

    let ready = with_state(|d| {
        *d = Dialog {
            api: Some(api.clone()),
            values: [mark.to_string(), extra.to_string()],
            light,
            scale,
            fonts: Some((big, small)),
            layout,
            on_done: Some(on_done),
            ..Dialog::default()
        };
    });
    if ready.is_none() {
        return false;
    }
    let atom = register_class(CLASS, dialog_proc, false);
    let title = wide(&format!("{APP_TITLE} — сочетания клавиш"));
    // SAFETY: класс зарегистрирован, строки живут до конца вызова; состояние сейчас не занято.
    let created = unsafe {
        if atom == 0 {
            None
        } else {
            CreateWindowExW(
                ex,
                PCWSTR(atom as usize as *const u16),
                PCWSTR(title.as_ptr()),
                style,
                x,
                y,
                size.0,
                size.1,
                HWND(null_mut()),
                HMENU(null_mut()),
                instance(),
                None,
            )
            .ok()
            .filter(|w| !w.0.is_null())
        }
    };
    let Some(window) = created else {
        with_state(|d| *d = Dialog::default());
        return false;
    };
    with_state(|d| d.hwnd = from_hwnd(window));
    dark_title_bar(from_hwnd(window), !light);
    // SAFETY: окно своё; фокус — как `focus_force` в Python.
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
    }
    true
}

/// Закрыть без итога (выход программы): `on_done` не вызывается.
pub fn close() {
    let hwnd = with_state(|d| {
        let hwnd = d.hwnd;
        *d = Dialog::default();
        hwnd
    })
    .unwrap_or(0);
    if hwnd != 0 {
        // SAFETY: окно своё, состояние уже отпущено.
        unsafe {
            let _ = DestroyWindow(to_hwnd(hwnd));
        }
    }
}

/// Середина рабочей области (P 2903: целочисленное деление вниз, как `//` в Python).
pub(crate) fn center(work: Rect, size: (i32, i32)) -> (i32, i32) {
    ((work.l + work.r - size.0).div_euclid(2), (work.t + work.b - size.1).div_euclid(2))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Button {
    Save,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    Field(usize),
    Button(Button),
}

/// Размеры текста, от которых строится раскладка.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Metrics {
    pub labels: [(i32, i32); 2], // блоки подписей полей (2 строки)
    pub hint: (i32, i32),        // блок пояснения (2 строки, мелкий шрифт)
    pub line_big: i32,
    pub line_small: i32,
    pub zero: i32, // ширина «0» обычным шрифтом (ширина поля — 20 знаков, кнопки — 12, как в Tk)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Layout {
    pub size: (i32, i32), // клиентская область
    pub labels: [Rect; 2],
    pub fields: [Rect; 2],
    pub hint: Rect,
    pub status: Rect,
    pub save: Rect,
    pub cancel: Rect,
    pub line_big: i32,
    pub line_small: i32,
}

/// Сетка Tk из P 2859–2899: две строки «подпись | поле» (поля 12 и 8), пояснение и строка
/// ошибки по центру, ряд кнопок с отступом 10. Отступы умножаются на масштаб.
pub(crate) fn layout(m: &Metrics, scale: f64) -> Layout {
    let pad = px(12.0, scale);
    let pady = px(8.0, scale);
    let gap = px(6.0, scale);
    let buttons_pad = px(10.0, scale);
    let field = (20 * m.zero + 2 * px(6.0, scale), m.line_big + 2 * px(5.0, scale));
    let button = (12 * m.zero + 2 * px(8.0, scale), m.line_big + 2 * px(6.0, scale));
    let label_w = m.labels[0].0.max(m.labels[1].0);
    let grid_w = label_w + 2 * pad + field.0 + 2 * pad;
    let buttons_w = 2 * button.0 + 4 * gap;
    let width = grid_w.max(m.hint.0 + 2 * pad).max(buttons_w);
    let left = (width - grid_w) / 2;

    let mut y = 0;
    let mut labels = [Rect::default(); 2];
    let mut fields = [Rect::default(); 2];
    for i in 0..2 {
        let (lw, lh) = m.labels[i];
        let row = lh.max(field.1) + 2 * pady;
        let top = y + (row - lh) / 2;
        labels[i] = Rect { l: left + pad, t: top, r: left + pad + lw, b: top + lh };
        let fl = left + label_w + 3 * pad;
        let ft = y + (row - field.1) / 2;
        fields[i] = Rect { l: fl, t: ft, r: fl + field.0, b: ft + field.1 };
        y += row;
    }
    let hl = (width - m.hint.0) / 2;
    let hint = Rect { l: hl, t: y, r: hl + m.hint.0, b: y + m.hint.1 };
    y += m.hint.1;
    let status = Rect { l: pad, t: y, r: width - pad, b: y + m.line_small };
    y += m.line_small + buttons_pad;
    let bl = (width - buttons_w) / 2 + gap;
    let save = Rect { l: bl, t: y, r: bl + button.0, b: y + button.1 };
    let cl = save.r + 2 * gap;
    let cancel = Rect { l: cl, t: y, r: cl + button.0, b: y + button.1 };
    y += button.1 + buttons_pad;
    Layout {
        size: (width, y),
        labels,
        fields,
        hint,
        status,
        save,
        cancel,
        line_big: m.line_big,
        line_small: m.line_small,
    }
}

fn inside(r: &Rect, p: Point) -> bool {
    p.x >= r.l && p.x < r.r && p.y >= r.t && p.y < r.b
}

/// Что под точкой клиентской области.
pub(crate) fn hit(l: &Layout, p: Point) -> Option<Hit> {
    if let Some(i) = l.fields.iter().position(|r| inside(r, p)) {
        return Some(Hit::Field(i));
    }
    if inside(&l.save, p) {
        return Some(Hit::Button(Button::Save));
    }
    if inside(&l.cancel, p) {
        return Some(Hit::Button(Button::Cancel));
    }
    None
}

/// Блок строк: (наибольшая ширина, сумма высот).
fn block(probe: &Canvas, text: &str, font: &Font) -> (i32, i32) {
    text.split('\n').fold((0, 0), |(w, h), line| {
        let (lw, lh) = probe.measure(line, font);
        (w.max(lw), h + lh)
    })
}

fn measure_metrics(big: &Font, small: &Font) -> Metrics {
    let probe = Canvas::new(1, 1);
    Metrics {
        labels: [block(&probe, LABELS[0], big), block(&probe, LABELS[1], big)],
        hint: block(&probe, HINT, small),
        line_big: probe.measure("", big).1,
        line_small: probe.measure("", small).1,
        zero: probe.measure("0", big).0,
    }
}

enum Outcome {
    Save(String, String),
    Cancel,
    Destroyed, // окно уничтожил кто-то другой
}

#[derive(Default)]
struct Dialog {
    hwnd: Hwnd,
    api: Option<Arc<Api>>,
    values: [String; 2],
    focus: Option<usize>,
    hover: Option<Button>,
    pressed: Option<Button>,
    tracking: bool,
    status: &'static str,
    light: bool,
    scale: f64,
    fonts: Option<(Font, Font)>,
    layout: Layout,
    on_done: Option<OnDone>,
}

thread_local! {
    static DIALOG: RefCell<Dialog> = RefCell::new(Dialog::default());
}

/// Состояние занято (сообщение пришло изнутри вызова Win32 отсюда же) или поток завершается —
/// ничего не делать.
fn with_state<R>(f: impl FnOnce(&mut Dialog) -> R) -> Option<R> {
    DIALOG.try_with(|cell| cell.try_borrow_mut().ok().map(|mut d| f(&mut d))).ok().flatten()
}

fn point_from(lparam: isize) -> Point {
    Point { x: (lparam & 0xFFFF) as u16 as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32 }
}

impl Dialog {
    fn invalidate(&self) {
        // SAFETY: окно своё.
        unsafe {
            let _ = InvalidateRect(to_hwnd(self.hwnd), None, BOOL(0));
        }
    }

    /// (Ctrl, Alt, Shift, Win) — нажаты сейчас (GetAsyncKeyState, как в P 2837–2841).
    fn modifiers(&self) -> (bool, bool, bool, bool) {
        let Some(api) = self.api.as_ref() else { return (false, false, false, false) };
        let down = |vk: u16| api.key_down(vk as i32);
        (down(VK_CONTROL.0), down(VK_MENU.0), down(VK_SHIFT.0), down(VK_LWIN.0) || down(VK_RWIN.0))
    }

    fn press(&mut self, button: Button) -> Option<Outcome> {
        match button {
            Button::Cancel => Some(Outcome::Cancel),
            Button::Save => match validate(&self.values[0], &self.values[1]) {
                Ok((mark, extra)) => Some(Outcome::Save(mark, extra)),
                Err(text) => {
                    self.status = text;
                    self.invalidate();
                    None
                }
            },
        }
    }

    fn key(&mut self, msg: u32, vk: u32) -> (Option<isize>, Option<Outcome>) {
        let (ctrl, alt, shift, win) = self.modifiers();
        if msg == WM_SYSKEYDOWN && vk == VK_F4.0 as u32 && alt && !ctrl && !shift && !win {
            return (None, None); // Alt+F4 — закрыть окно, как везде
        }
        if !(ctrl || alt || shift || win) {
            if vk == VK_TAB.0 as u32 {
                self.focus = Some(match self.focus {
                    Some(0) => 1,
                    _ => 0,
                });
                self.invalidate();
                return (Some(0), None);
            }
            if vk == VK_ESCAPE.0 as u32 {
                return (Some(0), Some(Outcome::Cancel));
            }
            if vk == VK_RETURN.0 as u32 {
                return (Some(0), self.press(Button::Save));
            }
        }
        match self.focus {
            Some(i) => {
                if let Some(text) = capture(vk, ctrl, alt, shift, win) {
                    self.values[i] = text;
                    self.invalidate();
                }
                (Some(0), None)
            }
            None if msg == WM_SYSKEYDOWN => (None, None),
            None => (Some(0), None),
        }
    }

    fn handle(&mut self, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> (Option<isize>, Option<Outcome>) {
        match msg {
            WM_ERASEBKGND => (Some(1), None),
            WM_PAINT => {
                self.paint(hwnd);
                (Some(0), None)
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: to_hwnd(hwnd),
                        dwHoverTime: 0,
                    };
                    // SAFETY: структура локальная.
                    self.tracking = unsafe { TrackMouseEvent(&mut track).is_ok() };
                }
                let hover = match hit(&self.layout, point_from(lparam)) {
                    Some(Hit::Button(b)) => Some(b),
                    _ => None,
                };
                if hover != self.hover {
                    self.hover = hover;
                    self.invalidate();
                }
                (Some(0), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if self.hover.take().is_some() {
                    self.invalidate();
                }
                (Some(0), None)
            }
            WM_LBUTTONDOWN => {
                match hit(&self.layout, point_from(lparam)) {
                    Some(Hit::Field(i)) => self.focus = Some(i),
                    Some(Hit::Button(b)) => {
                        self.pressed = Some(b);
                        // SAFETY: захват мыши своим окном до отпускания.
                        unsafe {
                            SetCapture(to_hwnd(hwnd));
                        }
                    }
                    None => {}
                }
                self.invalidate();
                (Some(0), None)
            }
            WM_LBUTTONUP => {
                // SAFETY: отпустить свой захват мыши.
                unsafe {
                    let _ = ReleaseCapture();
                }
                let pressed = self.pressed.take();
                self.invalidate();
                match (pressed, hit(&self.layout, point_from(lparam))) {
                    (Some(b), Some(Hit::Button(h))) if b == h => (Some(0), self.press(b)),
                    _ => (Some(0), None),
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => self.key(msg, wparam as u32),
            WM_SYSKEYUP => {
                let menu = wparam as u32 == VK_MENU.0 as u32 || wparam as u32 == VK_F10.0 as u32;
                // Отпускание Alt не открывает системное меню, пока ждём сочетание.
                if self.focus.is_some() && menu {
                    (Some(0), None)
                } else {
                    (None, None)
                }
            }
            WM_SYSCHAR if self.focus.is_some() => (Some(0), None),
            WM_CHAR => (Some(0), None),
            WM_CLOSE => (Some(0), Some(Outcome::Cancel)),
            WM_DESTROY => (None, self.on_done.is_some().then_some(Outcome::Destroyed)),
            _ => (None, None),
        }
    }

    fn paint(&self, hwnd: Hwnd) {
        let window = to_hwnd(hwnd);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: обработка WM_PAINT своего окна.
        unsafe {
            let dc = BeginPaint(window, &mut ps);
            if let Some(canvas) = self.render() {
                canvas.present(dc.0 as isize, 0, 0);
            }
            let _ = EndPaint(window, &ps);
        }
    }

    fn render(&self) -> Option<Canvas> {
        let (big, small) = self.fonts.as_ref()?;
        let p = palette(self.light);
        let l = &self.layout;
        let (w, h) = l.size;
        let mut c = Canvas::new(w, h);
        c.fill(Rect { l: 0, t: 0, r: w, b: h }, p.bg);
        let lines = |c: &mut Canvas, r: Rect, text: &str, font: &Font, color: Rgb, align: Align, height: i32| {
            for (i, line) in text.split('\n').enumerate() {
                let top = r.t + i as i32 * height;
                c.text(Rect { l: r.l, t: top, r: r.r, b: top + height }, line, font, color, align);
            }
        };
        for i in 0..2 {
            lines(&mut c, l.labels[i], LABELS[i], big, p.fg, Align::Left, l.line_big);
            let r = l.fields[i];
            let focused = self.focus == Some(i);
            let radius = px(4.0, self.scale);
            let fill = if self.light { Rgb(0xFF, 0xFF, 0xFF) } else { Rgb(0x1F, 0x1F, 0x1F) };
            c.round_rect(r, radius, fill, Some(if focused { p.primary } else { p.border }));
            if focused {
                c.round_frame(r, radius, p.primary, px(2.0, self.scale));
            }
            c.text(r, &self.values[i], big, p.fg, Align::Center);
        }
        let (hint, error) = if self.light { (Rgb(0x55, 0x55, 0x55), Rgb(0xB0, 0x00, 0x20)) } else { (p.muted, p.danger) };
        lines(&mut c, l.hint, HINT, small, hint, Align::Center, l.line_small);
        if !self.status.is_empty() {
            c.text(l.status, self.status, small, error, Align::Center);
        }
        for (button, r, caption) in [(Button::Save, l.save, SAVE), (Button::Cancel, l.cancel, CANCEL)] {
            let primary = button == Button::Save;
            let spec = ButtonSpec {
                rect: r,
                caption,
                hover: self.hover == Some(button),
                pressed: self.pressed == Some(button),
                selected: false,
                fill: Some(if primary { p.primary } else { p.button }),
                hover_fill: if primary { p.primary_hover } else { p.button_hover },
                fg: if primary { p.primary_fg } else { p.fg },
                align: Align::Center,
            };
            draw_button(&mut c, &spec, big, self.scale);
        }
        Some(c)
    }
}

/// Итог: окно уничтожить (если его не уничтожили раньше), состояние сбросить и потом, без
/// заимствования состояния, вызвать `on_done`.
fn finish(outcome: Outcome) {
    let taken = with_state(|d| {
        let hwnd = d.hwnd;
        let callback = d.on_done.take();
        *d = Dialog::default();
        (hwnd, callback)
    });
    let Some((hwnd, callback)) = taken else { return };
    let result = match outcome {
        Outcome::Save(mark, extra) => Some((mark, extra)),
        Outcome::Cancel => None,
        Outcome::Destroyed => {
            if let Some(mut callback) = callback {
                let _ = catch_unwind(AssertUnwindSafe(|| callback(None)));
            }
            return;
        }
    };
    if hwnd != 0 {
        // SAFETY: окно своё; состояние уже сброшено, WM_DESTROY пройдёт мимо.
        unsafe {
            let _ = DestroyWindow(to_hwnd(hwnd));
        }
    }
    if let Some(mut callback) = callback {
        // Паника в обработчике не выходит за оконную процедуру; место пишет panic hook.
        let _ = catch_unwind(AssertUnwindSafe(|| callback(result)));
    }
}

fn dialog_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    let handled = with_state(|d| (d.hwnd == hwnd).then(|| d.handle(hwnd, msg, wparam, lparam))).flatten();
    let Some((result, outcome)) = handled else { return None };
    if let Some(outcome) = outcome {
        finish(outcome);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_like_python() {
        assert_eq!(validate("Ctrl+Alt+Z", "Ctrl+Alt+Shift+Z"), Ok(("Ctrl+Alt+Z".into(), "Ctrl+Alt+Shift+Z".into())));
        assert_eq!(validate("  Win+Alt+Z ", "Win+Alt+Shift+Z"), Ok(("Win+Alt+Z".into(), "Win+Alt+Shift+Z".into())));
        assert_eq!(validate("Ctrl+Z", "Ctrl+Alt+X"), Err(ERROR_FORMAT)); // один модификатор
        assert_eq!(validate("Ctrl+Alt+X", "Ctrl+Shift+X"), Err(ERROR_FORMAT)); // нет Alt и Win
        assert_eq!(validate("", "Ctrl+Alt+X"), Err(ERROR_FORMAT));
        assert_eq!(validate("Ctrl+Alt+Q+W", "Ctrl+Alt+X"), Err(ERROR_FORMAT)); // две клавиши
        assert_eq!(validate("Ctrl+Alt+Z", "alt+ctrl+z"), Err(ERROR_SAME)); // то же сочетание другими словами
        assert_eq!(validate("Ctrl+Alt+F5", "Ctrl+Alt+F6"), Ok(("Ctrl+Alt+F5".into(), "Ctrl+Alt+F6".into())));
    }

    #[test]
    fn capture_by_virtual_keys() {
        assert_eq!(capture(0x5A, true, true, false, false).as_deref(), Some("Ctrl+Alt+Z"));
        assert_eq!(capture(0x5A, true, true, true, false).as_deref(), Some("Ctrl+Alt+Shift+Z"));
        assert_eq!(capture(0x74, false, true, false, true).as_deref(), Some("Alt+Win+F5"));
        assert_eq!(capture(0x31, true, true, false, false).as_deref(), Some("Ctrl+Alt+1"));
        assert_eq!(capture(0x2E, true, true, false, false).as_deref(), Some("Ctrl+Alt+Delete"));
        assert_eq!(capture(0x5A, false, false, false, false), None); // без модификаторов
        assert_eq!(capture(0x11, true, false, false, false), None); // сам Ctrl — не клавиша
        assert_eq!(capture(0x09, true, true, false, false), None); // Tab — нет имени
        // записанное окно клавиш проходит проверку
        let mark = capture(0x5A, true, true, false, false).unwrap_or_default();
        let extra = capture(0x5A, true, true, true, false).unwrap_or_default();
        assert!(validate(&mark, &extra).is_ok());
    }

    #[test]
    fn center_floors_like_python() {
        let work = Rect { l: 0, t: 0, r: 1920, b: 1040 };
        assert_eq!(center(work, (501, 300)), (709, 370));
        let left = Rect { l: -1920, t: 0, r: 0, b: 1040 };
        assert_eq!(center(left, (501, 301)), (-1211, 369)); // (-2421) // 2 == -1211
    }

    fn overlap(a: &Rect, b: &Rect) -> bool {
        a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b
    }

    fn within(r: &Rect, size: (i32, i32)) -> bool {
        r.l >= 0 && r.t >= 0 && r.r <= size.0 && r.b <= size.1 && r.r > r.l && r.b > r.t
    }

    #[test]
    fn layout_has_no_overlaps_and_fits() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let m = Metrics {
                labels: [(230, 40), (170, 40)],
                hint: (420, 32),
                line_big: 20,
                line_small: 16,
                zero: 8,
            };
            let m = Metrics {
                labels: m.labels.map(|(w, h)| (px(w as f64, scale), px(h as f64, scale))),
                hint: (px(420.0, scale), px(32.0, scale)),
                line_big: px(20.0, scale),
                line_small: px(16.0, scale),
                zero: px(8.0, scale),
            };
            let l = layout(&m, scale);
            let all = [l.labels[0], l.labels[1], l.fields[0], l.fields[1], l.hint, l.status, l.save, l.cancel];
            for r in &all {
                assert!(within(r, l.size), "{scale}: {r:?} вне {:?}", l.size);
            }
            for (i, a) in all.iter().enumerate() {
                for b in &all[i + 1..] {
                    assert!(!overlap(a, b), "{scale}: {a:?} и {b:?}");
                }
            }
            assert_eq!(l.fields[0].l, l.fields[1].l, "поля одно под другим");
            assert_eq!(l.fields[0].r - l.fields[0].l, l.fields[1].r - l.fields[1].l);
            assert!(l.fields[0].l > l.labels[0].r, "поле справа от подписи");
            assert!(l.save.r < l.cancel.l, "«Сохранить» левее «Отмена»");
            let (left, right) = (l.save.l, l.size.0 - l.cancel.r);
            assert!((left - right).abs() <= 1, "кнопки по центру: {left} / {right}");
            assert!(l.hint.t >= l.fields[1].b && l.save.t >= l.status.b);
        }
    }

    #[test]
    fn hit_finds_fields_and_buttons() {
        let m = Metrics { labels: [(230, 40), (170, 40)], hint: (420, 32), line_big: 20, line_small: 16, zero: 8 };
        let l = layout(&m, 1.0);
        let middle = |r: Rect| Point { x: (r.l + r.r) / 2, y: (r.t + r.b) / 2 };
        assert_eq!(hit(&l, middle(l.fields[0])), Some(Hit::Field(0)));
        assert_eq!(hit(&l, middle(l.fields[1])), Some(Hit::Field(1)));
        assert_eq!(hit(&l, middle(l.save)), Some(Hit::Button(Button::Save)));
        assert_eq!(hit(&l, middle(l.cancel)), Some(Hit::Button(Button::Cancel)));
        assert_eq!(hit(&l, middle(l.labels[0])), None);
        assert_eq!(hit(&l, Point { x: l.save.r, y: l.save.t }), None); // правый край не входит
    }

    #[test]
    fn closed_dialog_is_harmless() {
        assert!(!is_open());
        raise();
        close();
        assert!(!is_open());
    }

    #[test]
    fn dialog_renders_without_window() {
        let big = Font::new("Segoe UI", 15, false);
        let small = Font::new("Segoe UI", 12, false);
        let m = measure_metrics(&big, &small);
        assert!(m.labels[0].1 >= 2 * m.line_big && m.hint.1 >= 2 * m.line_small && m.zero > 0, "{m:?}");
        let d = Dialog {
            values: ["Ctrl+Alt+Z".into(), "Ctrl+Alt+Shift+Z".into()],
            focus: Some(0),
            status: ERROR_SAME,
            light: true,
            scale: 1.0,
            layout: layout(&m, 1.0),
            fonts: Some((big, small)),
            ..Dialog::default()
        };
        let c = d.render().expect("холст");
        assert_eq!(c.size(), d.layout.size);
        assert_eq!(c.pixel(1, 1), Some(palette(true).bg));
        let s = d.layout.save;
        assert_eq!(c.pixel((s.l + s.r) / 2, s.t + 3), Some(palette(true).primary));
    }
}
