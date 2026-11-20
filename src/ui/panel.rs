//! Окно панели (часть 7): одно WS_POPUP | WS_EX_TOPMOST | TOOLWINDOW, всё рисуется GDI в Canvas;
//! создаётся при первом открытии и уничтожается при закрытии.
//!
//! Логика — в `PanelModel`; здесь окно, двойная буферизация, мышь, клавиши, таймеры (опрос 50 мс
//! только пока панель открыта) и пояснения. Действие (`on_action`) вызывается после того, как
//! окно уже уничтожено и состояние панели отпущено: из обработчика можно снова вызывать `show`.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, EndPaint, InvalidateRect, ScreenToClient, UpdateWindow, PAINTSTRUCT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DELETE, VK_DOWN, VK_ESCAPE,
    VK_NEXT, VK_PRIOR, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowLongPtrW, IsWindowVisible, KillTimer, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, GWLP_USERDATA, HMENU, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_PAINT, WM_TIMER, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// В crate `windows` константа лежит в `Win32_UI_Controls` (feature не включён).
const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect};
use crate::ui::gfx::{
    self, draw_button, instance, light_theme, px, register_class, style_popup, Align, ButtonSpec, Canvas, Font,
};
use crate::ui::panel_model::{
    Draw, Focus, Key, Measure, Op, PanelAction, PanelModel, PanelRow, PanelState, ShowContext, Target, TextStyle,
    CONFIRM_MS, WATCH_MS,
};
use crate::ui::tooltip::Tip;
use crate::win::Api;

/// Всё внешнее, что нужно окну панели. Настоящее — `Api` (`Panel::new`); в тестах — подставное,
/// которое оставляет окно скрытым и не берёт фокус.
pub trait PanelEnv {
    fn work_area(&self, p: Point) -> Rect;
    /// Масштаб DPI монитора (1.0 = 96 DPI).
    fn scale_at(&self, p: Point) -> f64;
    fn cursor(&self) -> Point;
    /// Нажаты ли левая, правая, средняя кнопки мыши (GetAsyncKeyState 1, 2, 4).
    fn mouse_buttons(&self) -> [bool; 3];
    fn foreground(&self) -> Hwnd;
    /// Светлая тема панели задач.
    fn light(&self) -> bool;
    /// Показать готовое скрытое окно панели в `r` поверх всех и отдать ему клавиатуру.
    fn reveal(&self, hwnd: Hwnd, r: Rect);
}

impl PanelEnv for Api {
    fn work_area(&self, p: Point) -> Rect {
        Api::work_area(self, p)
    }

    fn scale_at(&self, p: Point) -> f64 {
        self.dpi_scale_at(p)
    }

    fn cursor(&self) -> Point {
        Api::cursor(self)
    }

    fn mouse_buttons(&self) -> [bool; 3] {
        [self.key_down(0x01), self.key_down(0x02), self.key_down(0x04)]
    }

    fn foreground(&self) -> Hwnd {
        Api::foreground(self)
    }

    fn light(&self) -> bool {
        light_theme(true)
    }

    fn reveal(&self, hwnd: Hwnd, r: Rect) {
        reveal_window(hwnd, r);
    }
}

/// Показать поверх всех и взять фокус (Esc, стрелки), как `_take_focus` в N 257–263.
pub fn reveal_window(hwnd: Hwnd, r: Rect) {
    // SAFETY: окно своё; ошибки показа и фокуса не опасны (опрос закроет панель по щелчку вне).
    unsafe {
        let window = to_hwnd(hwnd);
        let _ = SetWindowPos(window, HWND_TOPMOST, r.l, r.t, r.r - r.l, r.b - r.t, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(window);
    }
}

/// Шрифты панели одного масштаба + холст 1×1 для измерений.
pub struct PanelFonts {
    scale: f64,
    fonts: Vec<Font>,
    probe: Canvas,
}

impl PanelFonts {
    pub fn new(scale: f64) -> PanelFonts {
        let fonts = TextStyle::ALL
            .iter()
            .map(|style| {
                let (size, semibold, family) = style.spec();
                Font::new(family, px(size, scale), semibold)
            })
            .collect();
        PanelFonts { scale, fonts, probe: Canvas::new(1, 1) }
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn font(&self, style: TextStyle) -> &Font {
        &self.fonts[style as usize]
    }
}

impl Measure for PanelFonts {
    fn width(&self, style: TextStyle, s: &str) -> i32 {
        self.probe.measure(s, self.font(style)).0
    }

    fn height(&self, style: TextStyle) -> i32 {
        self.probe.measure("", self.font(style)).1
    }

    fn wrapped(&self, style: TextStyle, s: &str, width: i32) -> (i32, i32) {
        self.probe.measure_wrapped(s, self.font(style), width)
    }
}

/// Нарисовать вид панели (`PanelModel::scene`) в холст.
pub fn paint(canvas: &mut Canvas, scene: &[Draw], fonts: &PanelFonts) {
    for draw in scene {
        match &draw.op {
            Op::Fill { r, c } => canvas.fill(*r, *c),
            Op::Round { r, radius, fill, outline } => canvas.round_rect(*r, *radius, *fill, *outline),
            Op::Polygon { points, fill } => canvas.polygon(points, *fill),
            Op::Text { r, s, style, c, align, wrap } => {
                if *wrap {
                    canvas.text_wrapped(*r, s, fonts.font(*style), *c);
                } else {
                    canvas.text(*r, s, fonts.font(*style), *c, *align);
                }
            }
            Op::Button { r, caption, hover, pressed, fg, hover_fill } => {
                let spec = ButtonSpec {
                    rect: *r,
                    caption,
                    hover: *hover,
                    pressed: *pressed,
                    selected: false,
                    fill: None,
                    hover_fill: *hover_fill,
                    fg: *fg,
                    align: Align::Left,
                };
                draw_button(canvas, &spec, fonts.font(TextStyle::Text), fonts.scale);
            }
        }
    }
}

const CLASS: &str = "ZakladkaPanel";
const TIMER_WATCH: usize = 1;
const TIMER_CONFIRM: usize = 2;

/// Секунды от первого вызова (монотонные; нужны только разности).
fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Измеритель для закрытой панели: модель тогда ничего не меряет.
struct NoMeasure;

impl Measure for NoMeasure {
    fn width(&self, _: TextStyle, _: &str) -> i32 {
        0
    }

    fn height(&self, _: TextStyle) -> i32 {
        0
    }

    fn wrapped(&self, _: TextStyle, _: &str, _: i32) -> (i32, i32) {
        (0, 0)
    }
}

struct Inner {
    env: Arc<dyn PanelEnv>,
    on_action: Option<Box<dyn FnMut(PanelAction)>>,
    model: PanelModel,
    fonts: Option<PanelFonts>,
    canvas: Option<Canvas>,
    hwnd: Hwnd,
    dirty: bool,
    tracking: bool,
    tip_target: Option<Target>,
    me: Weak<RefCell<Inner>>,
}

pub struct Panel {
    inner: Rc<RefCell<Inner>>,
}

impl Panel {
    pub fn new(api: Arc<Api>, on_action: Box<dyn FnMut(PanelAction)>) -> Panel {
        Panel::with_env(api, on_action)
    }

    /// Панель с подставным окружением (тесты: окно остаётся скрытым).
    pub fn with_env(env: Arc<dyn PanelEnv>, on_action: Box<dyn FnMut(PanelAction)>) -> Panel {
        let inner = Rc::new_cyclic(|me| {
            RefCell::new(Inner {
                env,
                on_action: Some(on_action),
                model: PanelModel::new(),
                fonts: None,
                canvas: None,
                hwnd: 0,
                dirty: true,
                tracking: false,
                tip_target: None,
                me: me.clone(),
            })
        });
        Panel { inner }
    }

    /// Повторный вызов закрывает.
    pub fn show(&mut self, anchor: Point, rows: Vec<PanelRow>, state: PanelState) -> bool {
        let toggled = match self.inner.try_borrow_mut() {
            Ok(mut inner) if inner.model.is_open() => inner.model.dismiss(None),
            Ok(_) => None,
            Err(_) => return false,
        };
        if let Some(action) = toggled {
            finish(&self.inner, action);
            return false;
        }
        let env = self.inner.borrow().env.clone();
        let scale = env.scale_at(anchor);
        let scale = if scale.is_finite() { scale.clamp(1.0, 4.0) } else { 1.0 };
        let ctx = ShowContext {
            now: clock(),
            anchor,
            work_area: env.work_area(anchor),
            scale,
            light: env.light(),
            rounded: gfx::rounded_corners(),
            buttons: env.mouse_buttons(),
        };
        let (hwnd, rect) = {
            let Ok(mut guard) = self.inner.try_borrow_mut() else { return false };
            let inner = &mut *guard;
            if inner.fonts.as_ref().map_or(true, |f| f.scale() != scale) {
                inner.fonts = Some(PanelFonts::new(scale));
            }
            let opened = match inner.fonts.as_ref() {
                Some(fonts) => inner.model.show(&ctx, rows, state, fonts).0,
                None => false,
            };
            let rect = inner.model.geometry();
            match rect {
                Some(rect) if opened && inner.create_window(rect) => {
                    inner.render();
                    (inner.hwnd, rect)
                }
                _ => {
                    inner.model.close();
                    inner.take_window();
                    return false;
                }
            }
        };
        style_popup(hwnd);
        env.reveal(hwnd, rect);
        // SAFETY: окно своё; таймер умирает вместе с окном.
        unsafe {
            SetTimer(to_hwnd(hwnd), TIMER_WATCH, WATCH_MS, None);
            let _ = UpdateWindow(to_hwnd(hwnd));
        }
        true
    }

    pub fn update(&mut self, rows: Vec<PanelRow>, state: PanelState) {
        let Ok(mut guard) = self.inner.try_borrow_mut() else { return };
        let inner = &mut *guard;
        let live = inner.hwnd != 0 && inner.model.is_open();
        let moved = match (live, inner.fonts.as_ref()) {
            (true, Some(fonts)) => inner.model.update(rows, state, fonts),
            _ => inner.model.update(rows, state, &NoMeasure),
        };
        if !live {
            return;
        }
        if moved {
            if let Some(g) = inner.model.geometry() {
                // SAFETY: окно своё.
                unsafe {
                    let _ = SetWindowPos(
                        to_hwnd(inner.hwnd),
                        HWND(std::ptr::null_mut()),
                        g.l,
                        g.t,
                        g.r - g.l,
                        g.b - g.t,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
        }
        inner.invalidate();
    }

    pub fn hide(&mut self) {
        let hwnd = match self.inner.try_borrow_mut() {
            Ok(mut inner) => {
                inner.model.close();
                inner.take_window()
            }
            Err(_) => return,
        };
        destroy(hwnd);
    }

    pub fn is_open(&self) -> bool {
        self.inner.try_borrow().is_ok_and(|inner| inner.model.is_open() && inner.hwnd != 0)
    }

    /// Окно открытой панели (0 — закрыта).
    pub fn hwnd(&self) -> Hwnd {
        self.inner.try_borrow().map_or(0, |inner| inner.hwnd)
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.hide();
    }
}

/// Действие: окно уничтожить, потом (без заимствования состояния) вызвать `on_action`.
fn finish(inner: &Rc<RefCell<Inner>>, action: PanelAction) {
    let hwnd = match inner.try_borrow_mut() {
        Ok(mut i) => i.take_window(),
        Err(_) => return,
    };
    destroy(hwnd);
    call(inner, action);
}

/// Действие, после которого панель остаётся открытой (`keeps_panel`): окно живо, пояснение
/// убирается (строка под указателем могла смениться), обработчик вызывается так же.
fn notify(inner: &Rc<RefCell<Inner>>, action: PanelAction) {
    Tip::hide(0);
    if let Ok(mut i) = inner.try_borrow_mut() {
        i.tip_target = None;
        i.invalidate();
    }
    call(inner, action);
}

/// `on_action` без заимствования состояния.
fn call(inner: &Rc<RefCell<Inner>>, action: PanelAction) {
    let callback = match inner.try_borrow_mut() {
        Ok(mut i) => i.on_action.take(),
        Err(_) => return,
    };
    if let Some(mut callback) = callback {
        // Паника в обработчике не должна терять его: место паники пишет panic hook.
        let _ = catch_unwind(AssertUnwindSafe(|| callback(action)));
        if let Ok(mut i) = inner.try_borrow_mut() {
            if i.on_action.is_none() {
                i.on_action = Some(callback);
            }
        }
    }
}

fn destroy(hwnd: Hwnd) {
    if hwnd == 0 {
        return;
    }
    Tip::hide(0); // как `_TipWindow.hide_for` в N 1083: любое пояснение
    // SAFETY: окно своё; WM_NCDESTROY освобождает привязку к состоянию.
    unsafe {
        let _ = DestroyWindow(to_hwnd(hwnd));
    }
}

fn point_from(lparam: isize) -> Point {
    Point { x: (lparam & 0xFFFF) as u16 as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32 }
}

fn to_screen(hwnd: Hwnd, p: Point) -> Point {
    let mut point = POINT::from(p);
    // SAFETY: окно своё, точка локальная.
    unsafe {
        let _ = ClientToScreen(to_hwnd(hwnd), &mut point);
    }
    point.into()
}

fn to_client(hwnd: Hwnd, p: Point) -> Point {
    let mut point = POINT::from(p);
    // SAFETY: окно своё, точка локальная.
    unsafe {
        let _ = ScreenToClient(to_hwnd(hwnd), &mut point);
    }
    point.into()
}

impl Inner {
    /// Создать скрытое окно панели в `r` и привязать к нему это состояние (GWLP_USERDATA).
    fn create_window(&mut self, r: Rect) -> bool {
        let atom = register_class(CLASS, panel_proc, false);
        if atom == 0 {
            return false;
        }
        // SAFETY: класс зарегистрирован; привязка ставится после создания и снимается в WM_NCDESTROY.
        unsafe {
            let created = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(atom as usize as *const u16),
                w!("Закладка"),
                WS_POPUP,
                r.l,
                r.t,
                r.r - r.l,
                r.b - r.t,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                instance(),
                None,
            );
            let Ok(window) = created else { return false };
            if window.0.is_null() {
                return false;
            }
            let link = Box::into_raw(Box::new(self.me.clone()));
            SetWindowLongPtrW(window, GWLP_USERDATA, link as isize);
            self.hwnd = from_hwnd(window);
        }
        self.dirty = true;
        true
    }

    /// Отвязать окно (его уничтожает вызывающий) и отпустить холст и шрифты.
    fn take_window(&mut self) -> Hwnd {
        let hwnd = self.hwnd;
        self.hwnd = 0;
        self.canvas = None;
        self.fonts = None;
        self.tracking = false;
        self.tip_target = None;
        self.dirty = true;
        hwnd
    }

    /// Перерисовать вид в задний буфер.
    fn render(&mut self) {
        let Some(g) = self.model.geometry() else { return };
        let size = (g.r - g.l, g.b - g.t);
        if self.canvas.as_ref().map_or(true, |c| c.size() != size) {
            self.canvas = Some(Canvas::new(size.0, size.1));
        }
        if let (Some(canvas), Some(fonts)) = (self.canvas.as_mut(), self.fonts.as_ref()) {
            let scene = self.model.scene(fonts);
            paint(canvas, &scene, fonts);
        }
        self.dirty = false;
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        if self.hwnd != 0 {
            // SAFETY: окно своё.
            unsafe {
                let _ = InvalidateRect(to_hwnd(self.hwnd), None, BOOL(0));
            }
        }
    }

    fn visual(&self) -> (Option<(usize, crate::ui::panel_model::Zone)>, Option<crate::ui::panel_model::Item>) {
        (self.model.hover(), self.model.item_hover())
    }

    /// Пояснение к части под указателем: новая часть — спрятать и ждать снова (N 976–985).
    fn update_tip(&mut self, hwnd: Hwnd, p: Point) {
        let target = self.model.pointer_target();
        if target == self.tip_target {
            return;
        }
        self.tip_target = target;
        Tip::hide(hwnd);
        if let Some(target) = target {
            let text = self.model.tip_text(target);
            if !text.is_empty() {
                Tip::schedule(hwnd, text, to_screen(hwnd, p), self.model.light());
            }
        }
    }

    fn handle(&mut self, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> (Option<isize>, Option<PanelAction>) {
        if hwnd != self.hwnd {
            return (None, None);
        }
        let window = to_hwnd(hwnd);
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: обработка WM_PAINT своего окна.
                unsafe {
                    let dc = BeginPaint(window, &mut ps);
                    if self.dirty || self.canvas.is_none() {
                        self.render();
                    }
                    if let Some(canvas) = &self.canvas {
                        canvas.present(dc.0 as isize, 0, 0);
                    }
                    let _ = EndPaint(window, &ps);
                }
                (Some(0), None)
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    // SAFETY: структура локальная.
                    self.tracking = unsafe { TrackMouseEvent(&mut track).is_ok() };
                }
                let p = point_from(lparam);
                let before = self.visual();
                self.model.mouse_move(p);
                if self.visual() != before {
                    self.invalidate();
                }
                self.update_tip(hwnd, p);
                (Some(0), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.model.mouse_leave();
                self.tip_target = None;
                Tip::hide(hwnd);
                self.invalidate();
                (Some(0), None)
            }
            WM_LBUTTONDOWN => {
                Tip::hide(hwnd);
                // SAFETY: окно своё. Скрытому окну (тесты) мышь не захватывать.
                unsafe {
                    if IsWindowVisible(window).as_bool() {
                        SetCapture(window);
                    }
                }
                self.model.press(point_from(lparam));
                self.invalidate();
                (Some(0), None)
            }
            WM_LBUTTONUP => {
                // SAFETY: отпустить свой захват мыши.
                unsafe {
                    let _ = ReleaseCapture();
                }
                let was_confirming = self.model.confirm_all();
                if let Some(action) = self.model.release(point_from(lparam)) {
                    return (Some(0), Some(action));
                }
                if self.model.confirm_all() && !was_confirming {
                    // SAFETY: окно своё.
                    unsafe {
                        SetTimer(window, TIMER_CONFIRM, CONFIRM_MS, None);
                    }
                }
                self.invalidate();
                (Some(0), None)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                let p = to_client(hwnd, point_from(lparam));
                if self.model.wheel(delta, Some(p)) {
                    Tip::hide(hwnd);
                    self.tip_target = self.model.pointer_target();
                    self.invalidate();
                }
                (Some(0), None)
            }
            WM_KEYDOWN => {
                let vk = wparam as u16;
                let key = match vk {
                    v if v == VK_ESCAPE.0 => Key::Escape,
                    v if v == VK_UP.0 => Key::Up,
                    v if v == VK_DOWN.0 => Key::Down,
                    v if v == VK_PRIOR.0 => Key::PageUp,
                    v if v == VK_NEXT.0 => Key::PageDown,
                    v if v == VK_RETURN.0 => Key::Enter,
                    v if v == VK_DELETE.0 => Key::Delete,
                    _ => return (None, None),
                };
                let action = self.model.key(key);
                if action.is_none() {
                    self.invalidate();
                }
                (Some(0), action)
            }
            WM_TIMER if wparam == TIMER_WATCH => {
                let foreground = self.env.foreground();
                let focus = if foreground == 0 {
                    Focus::None
                } else if foreground == hwnd {
                    Focus::Panel
                } else {
                    Focus::Other
                };
                let action = self.model.watch(clock(), focus, self.env.mouse_buttons(), self.env.cursor());
                (Some(0), action)
            }
            WM_TIMER if wparam == TIMER_CONFIRM => {
                // SAFETY: окно своё.
                unsafe {
                    let _ = KillTimer(window, TIMER_CONFIRM);
                }
                self.model.end_confirm();
                self.invalidate();
                (Some(0), None)
            }
            WM_CLOSE => (Some(0), self.model.dismiss(None)),
            WM_DESTROY => {
                // Окно уничтожил кто-то другой (наше закрытие отвязывает окно раньше).
                self.model.close();
                self.take_window();
                (None, None)
            }
            _ => (None, None),
        }
    }
}

/// Оконная процедура панели (вызывается переходником `register_class` внутри `catch_unwind`).
fn panel_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    let window = to_hwnd(hwnd);
    match msg {
        WM_ERASEBKGND => return Some(1), // фон рисует WM_PAINT из заднего буфера
        WM_NCDESTROY => {
            // SAFETY: привязку ставит create_window (Box<Weak>), снимаем один раз.
            unsafe {
                let link = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut Weak<RefCell<Inner>>;
                if !link.is_null() {
                    drop(Box::from_raw(link));
                }
            }
            return None;
        }
        _ => {}
    }
    // SAFETY: в GWLP_USERDATA только наш Box<Weak> (или 0 до привязки).
    let inner = unsafe {
        let link = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Weak<RefCell<Inner>>;
        if link.is_null() {
            return None;
        }
        (*link).upgrade()?
    };
    let (result, action) = {
        // Занято (сообщение пришло изнутри вызова Win32 из самой панели) — обработка по умолчанию.
        let Ok(mut state) = inner.try_borrow_mut() else { return None };
        state.handle(hwnd, msg, wparam, lparam)
    };
    if let Some(action) = action {
        if action.keeps_panel() {
            notify(&inner, action);
        } else {
            finish(&inner, action);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lparam_points_are_signed() {
        assert_eq!(point_from(0x0002_0001), Point { x: 1, y: 2 });
        let negative = ((-5i16 as u16 as isize) << 16) | (-7i16 as u16 as isize);
        assert_eq!(point_from(negative), Point { x: -7, y: -5 });
    }
}
