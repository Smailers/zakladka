//! Пипетка (часть 8; N 1402–1555): слоёное окно на весь виртуальный экран, лупа 9×9.
//!
//! Два окна. Накрытие — на весь виртуальный экран (в том числе отрицательные координаты второго
//! монитора), слоёное с альфой 3/255: почти не видно, но получает щелчки и клавиатуру; курсор —
//! крест. Лупа — 9×9 клеток по `11 × scale` px и строка с образцом и «#RRGGBB»; прозрачна для мыши
//! и никогда не активируется. Оба окна слоёные, поэтому `Api::capture` (BitBlt без CAPTUREBLT)
//! видит экран под ними. Кадр — таймер 33 мс на накрытии; лупа держится рядом с курсором.
//!
//! Логика — в `PickerModel` (без окон), внешнее — `ScreenEnv` (в тестах подставное: окна остаются
//! скрытыми). Пока пипетка работает, её держит сам модуль (одна на UI-поток; вторая отменяет
//! первую): ручку `Eyedropper` можно отпустить — пипетка закончится сама по щелчку, Esc, правой
//! кнопке или через 120 с. Окна создаются при запуске и уничтожаются при конце.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HINSTANCE, HWND};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, GetStockObject, InvalidateRect, BLACK_BRUSH, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowLongPtrW, LoadCursorW, SetCursor, SetForegroundWindow,
    SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA, HMENU,
    HTTRANSPARENT, HWND_TOPMOST, IDC_CROSS, LWA_ALPHA, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect, Rgb};
use crate::ui::gfx::{instance, px, register_class, Align, Canvas, Font, WndProc};
use crate::win::{monotonic, Api, Bitmap};

/// Клеток лупы по стороне (N 1410).
pub const CELLS: i32 = 9;
/// Сторона клетки при 96 DPI (N 1410).
pub const CELL: f64 = 11.0;
/// Высота строки с образцом и «#RRGGBB» при 96 DPI (N 1440).
pub const FOOTER: f64 = 26.0;
/// Отступ лупы от курсора при 96 DPI (N 1517).
pub const GAP: f64 = 24.0;
/// Кадр, мс (N 1411).
pub const FRAME_MS: u32 = 33;
/// Без щелчка — отмена через, с (N 1411).
pub const TIMEOUT: f64 = 120.0;
/// Альфа накрытия: 0,01 × 255 — почти не видно, но щелчки получает (N 1425).
pub const OVERLAY_ALPHA: u8 = 3;
/// Альфа лупы: 0,99 × 255 — слоёное окно не попадает в снимок экрана (N 1438).
pub const LOUPE_ALPHA: u8 = 252;

const VK_ESCAPE: i32 = 0x1B;
const COUNT: usize = (CELLS * CELLS) as usize;
const BLACK: Rgb = Rgb(0, 0, 0);
const WHITE: Rgb = Rgb(0xFF, 0xFF, 0xFF);
const LOUPE_BG: Rgb = Rgb(0x20, 0x20, 0x20);
const LOUPE_BORDER: Rgb = Rgb(0x5A, 0x5A, 0x5A);

/// Всё внешнее, что нужно пипетке (и окну цветов). Настоящее — `Api`; в тестах — подставное,
/// которое не показывает окна, не берёт фокус и отдаёт заданный цвет.
pub trait ScreenEnv {
    fn cursor(&self) -> Point;
    fn work_area(&self, p: Point) -> Rect;
    /// Масштаб DPI монитора (1.0 = 96 DPI).
    fn scale_at(&self, p: Point) -> f64;
    /// Весь виртуальный экран (все мониторы).
    fn virtual_screen(&self) -> Rect;
    /// Снимок прямоугольника экрана без слоёных окон.
    fn capture(&self, r: Rect) -> Option<Bitmap>;
    /// Клавиша нажата сейчас (GetAsyncKeyState & 0x8000).
    fn key_down(&self, vk: i32) -> bool;
    /// Монотонные секунды.
    fn now(&self) -> f64;
    /// Показать окно поверх всех в `r`; `activate` — сделать активным и отдать ему клавиатуру
    /// (как `_take_focus`, N 257–263).
    fn show(&self, hwnd: Hwnd, r: Rect, activate: bool);
    /// Спрятать окно (`withdraw`).
    fn hide(&self, hwnd: Hwnd);
}

impl ScreenEnv for Api {
    fn cursor(&self) -> Point {
        Api::cursor(self)
    }

    fn work_area(&self, p: Point) -> Rect {
        Api::work_area(self, p)
    }

    fn scale_at(&self, p: Point) -> f64 {
        self.dpi_scale_at(p)
    }

    fn virtual_screen(&self) -> Rect {
        Api::virtual_screen(self)
    }

    fn capture(&self, r: Rect) -> Option<Bitmap> {
        Api::capture(self, r, None)
    }

    fn key_down(&self, vk: i32) -> bool {
        Api::key_down(self, vk)
    }

    fn now(&self) -> f64 {
        monotonic()
    }

    fn show(&self, hwnd: Hwnd, r: Rect, activate: bool) {
        show_window(hwnd, r, activate);
    }

    fn hide(&self, hwnd: Hwnd) {
        hide_window(hwnd);
    }
}

/// Показать окно поверх всех в `r`; `activate` — на передний план и с клавиатурой.
pub fn show_window(hwnd: Hwnd, r: Rect, activate: bool) {
    if hwnd == 0 {
        return;
    }
    let flags = if activate { SWP_SHOWWINDOW } else { SWP_SHOWWINDOW | SWP_NOACTIVATE };
    // SAFETY: окно своё; ошибка показа или фокуса не опасна.
    unsafe {
        let window = to_hwnd(hwnd);
        let _ = SetWindowPos(window, HWND_TOPMOST, r.l, r.t, r.r - r.l, r.b - r.t, flags);
        if activate {
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(window);
        }
    }
}

/// Спрятать окно.
pub fn hide_window(hwnd: Hwnd) {
    if hwnd == 0 {
        return;
    }
    // SAFETY: окно своё.
    unsafe {
        let _ = ShowWindow(to_hwnd(hwnd), SW_HIDE);
    }
}

// ------------------------------------------------------------ модель (без окон)

/// Чем кончилась пипетка.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Pick(Rgb),
    Cancel,
}

/// Что сделать окнам после шага модели.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Ничего (пипетка уже кончилась или нажатия не было).
    Idle,
    /// Кадр: перерисовать лупу и/или сдвинуть её в точку `moved`.
    Frame { redraw: bool, moved: Option<Point> },
    /// Пипетка кончилась: уничтожить окна и вызвать `on_pick`/`on_cancel`.
    Done(Outcome),
}

/// Пипетка без окон (N 1406–1543): клетки лупы, последний цвет, нажатие, отмена, место лупы.
#[derive(Clone, Debug)]
pub struct PickerModel {
    active: bool,
    deadline: f64,
    scale: f64,
    cells: [Option<Rgb>; COUNT],
    last: Option<Rgb>,
    pressed: Option<Rgb>,
    position: Option<Point>,
}

impl PickerModel {
    /// `now` — монотонные секунды запуска (отсчёт 120 с), `scale` — масштаб монитора с курсором.
    pub fn new(now: f64, scale: f64) -> PickerModel {
        let scale = if scale.is_finite() { scale.clamp(1.0, 4.0) } else { 1.0 };
        PickerModel {
            active: true,
            deadline: now + TIMEOUT,
            scale,
            cells: [None; COUNT],
            last: None,
            pressed: None,
            position: None,
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// Цвет под курсором в последнем кадре.
    pub fn last(&self) -> Option<Rgb> {
        self.last
    }

    /// Надпись лупы: «#RRGGBB» или пусто до первого кадра.
    pub fn label(&self) -> String {
        self.last.map(|c| c.hex()).unwrap_or_default()
    }

    /// Клетки лупы по строкам; `None` — ещё не снято (рисуется чёрным).
    pub fn cells(&self) -> &[Option<Rgb>] {
        &self.cells
    }

    /// Левый верхний угол лупы на экране (после первого кадра).
    pub fn position(&self) -> Option<Point> {
        self.position
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn cell(&self) -> i32 {
        px(CELL, self.scale)
    }

    pub fn footer(&self) -> i32 {
        px(FOOTER, self.scale)
    }

    /// Размер окна лупы: клетки + строка + рамка по 1 px (N 1456).
    pub fn size(&self) -> (i32, i32) {
        let side = CELLS * self.cell();
        (side + 2, side + self.footer() + 2)
    }

    /// Кадр (N 1490–1512): 120 с прошли или нажат Esc (в том числе когда клавиатура не у
    /// накрытия) — отмена; иначе снимок 9×9 вокруг курсора и место лупы.
    pub fn tick(&mut self, env: &dyn ScreenEnv) -> Step {
        if !self.active {
            return Step::Idle;
        }
        if env.now() > self.deadline || env.key_down(VK_ESCAPE) {
            self.active = false;
            return Step::Done(Outcome::Cancel);
        }
        let p = env.cursor();
        let half = CELLS / 2;
        let mut redraw = false;
        let around = Rect { l: p.x - half, t: p.y - half, r: p.x + half + 1, b: p.y + half + 1 };
        if let Some(bmp) = env.capture(around) {
            if bmp.width == CELLS && bmp.height == CELLS && bmp.bgra.len() >= COUNT * 4 {
                for (index, cell) in self.cells.iter_mut().enumerate() {
                    let color = Some(bgra(&bmp.bgra, index * 4));
                    if *cell != color {
                        *cell = color;
                        redraw = true;
                    }
                }
                let center = self.cells[COUNT / 2];
                if center != self.last {
                    self.last = center;
                    redraw = true;
                }
            }
        }
        let moved = self.follow(p, env.work_area(p));
        Step::Frame { redraw, moved }
    }

    /// Лупа рядом с курсором, но не под ним (N 1514–1527): правее и ниже на 24, у правого или
    /// нижнего края — с другой стороны. `Some` — новое место.
    pub fn follow(&mut self, p: Point, work: Rect) -> Option<Point> {
        let (width, height) = self.size();
        let gap = px(GAP, self.scale);
        let mut at = Point { x: p.x + gap, y: p.y + gap };
        if at.x + width > work.r {
            at.x = p.x - gap - width;
        }
        if at.y + height > work.b {
            at.y = p.y - gap - height;
        }
        if self.position == Some(at) {
            return None;
        }
        self.position = Some(at);
        Some(at)
    }

    /// Нажатие (N 1529–1532): цвет точки под курсором; не снялся — последний цвет лупы.
    pub fn press(&mut self, env: &dyn ScreenEnv) {
        if !self.active {
            return;
        }
        let p = env.cursor();
        let pixel = env
            .capture(Rect { l: p.x, t: p.y, r: p.x + 1, b: p.y + 1 })
            .filter(|b| b.width == 1 && b.height == 1 && b.bgra.len() >= 4)
            .map(|b| bgra(&b.bgra, 0));
        self.pressed = pixel.or(self.last);
    }

    /// Отпускание (N 1534–1543): цвет отдаётся при отпускании, чтобы оно не дошло до окна под
    /// пипеткой. Без нажатия (или без цвета) — ничего.
    pub fn release(&mut self) -> Step {
        if !self.active {
            return Step::Idle;
        }
        let Some(color) = self.pressed else { return Step::Idle };
        self.active = false;
        Step::Done(Outcome::Pick(color))
    }

    /// Отмена (N 1468–1473): только у работающей пипетки.
    pub fn cancel(&mut self) -> Step {
        if !self.active {
            return Step::Idle;
        }
        self.active = false;
        Step::Done(Outcome::Cancel)
    }
}

fn bgra(data: &[u8], at: usize) -> Rgb {
    Rgb(data[at + 2], data[at + 1], data[at])
}

/// Рамка толщиной 1 px по краю `r` (`r` без правого и нижнего края).
fn frame(c: &mut Canvas, r: Rect, color: Rgb) {
    c.fill(Rect { l: r.l, t: r.t, r: r.r, b: r.t + 1 }, color);
    c.fill(Rect { l: r.l, t: r.b - 1, r: r.r, b: r.b }, color);
    c.fill(Rect { l: r.l, t: r.t, r: r.l + 1, b: r.b }, color);
    c.fill(Rect { l: r.r - 1, t: r.t, r: r.r, b: r.b }, color);
}

/// Картинка лупы (N 1439–1456): рамка #5A5A5A, клетки, рамки средней клетки (чёрная снаружи,
/// белая внутри), образец в белой рамке и «#RRGGBB». `font` — Segoe UI `13 × scale`.
pub fn render_loupe(model: &PickerModel, font: &Font) -> Canvas {
    let (width, height) = model.size();
    let s = |v: f64| px(v, model.scale());
    let cell = model.cell();
    let side = CELLS * cell;
    let mut c = Canvas::new(width, height);
    c.fill(Rect { l: 0, t: 0, r: width, b: height }, LOUPE_BORDER);
    c.fill(Rect { l: 1, t: 1, r: width - 1, b: height - 1 }, LOUPE_BG);
    for (index, color) in model.cells().iter().enumerate() {
        let x = 1 + (index as i32 % CELLS) * cell;
        let y = 1 + (index as i32 / CELLS) * cell;
        c.fill(Rect { l: x, t: y, r: x + cell, b: y + cell }, color.unwrap_or(BLACK));
    }
    // Тк-прямоугольники (m-1, m-1, m+cell, m+cell) и (m, m, m+cell-1, m+cell-1) включают края.
    let m = 1 + CELLS / 2 * cell;
    frame(&mut c, Rect { l: m - 1, t: m - 1, r: m + cell + 1, b: m + cell + 1 }, BLACK);
    frame(&mut c, Rect { l: m, t: m, r: m + cell, b: m + cell }, WHITE);
    let swatch = Rect { l: 1 + s(6.0), t: 1 + side + s(6.0), r: 2 + s(20.0), b: 2 + side + s(20.0) };
    c.fill(swatch, WHITE);
    let inner = Rect { l: swatch.l + 1, t: swatch.t + 1, r: swatch.r - 1, b: swatch.b - 1 };
    c.fill(inner, model.last().unwrap_or(BLACK));
    let label = model.label();
    let line = Rect { l: 1 + s(28.0), t: 1 + side, r: width - 1, b: 1 + side + model.footer() };
    c.text(line, &label, font, WHITE, Align::Left);
    c
}

// ------------------------------------------------------------ окна

const OVERLAY_CLASS: &str = "ZakladkaPicker";
const LOUPE_CLASS: &str = "ZakladkaLoupe";
const TIMER_FRAME: usize = 1;

struct Inner {
    env: Arc<dyn ScreenEnv>,
    model: PickerModel,
    overlay: Hwnd,
    loupe: Hwnd,
    font: Option<Font>,
    canvas: Option<Canvas>,
    dirty: bool,
    on_pick: Option<Box<dyn FnMut(Rgb)>>,
    on_cancel: Option<Box<dyn FnMut()>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Страховка: окна уже уничтожены в `finish`; если нет — не оставлять накрытие на экране.
        for hwnd in [std::mem::take(&mut self.loupe), std::mem::take(&mut self.overlay)] {
            destroy(hwnd);
        }
    }
}

thread_local! {
    /// Работающая пипетка UI-потока: её держит модуль, а не ручка (N 1550–1554).
    static CURRENT: RefCell<Option<Rc<RefCell<Inner>>>> = const { RefCell::new(None) };
}

fn take_current() -> Option<Rc<RefCell<Inner>>> {
    CURRENT.try_with(|c| c.try_borrow_mut().ok().and_then(|mut c| c.take())).ok().flatten()
}

fn set_current(inner: Option<Rc<RefCell<Inner>>>) {
    let _ = CURRENT.try_with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            *c = inner;
        }
    });
}

/// Убрать `inner` из текущей, если это она (выданная ссылка живёт до конца вызова у вызывающего).
fn forget_current(inner: &Rc<RefCell<Inner>>) {
    let taken = CURRENT
        .try_with(|c| {
            let mut c = c.try_borrow_mut().ok()?;
            if c.as_ref().is_some_and(|x| Rc::ptr_eq(x, inner)) {
                c.take()
            } else {
                None
            }
        })
        .ok()
        .flatten();
    drop(taken);
}

pub struct Eyedropper {
    inner: Rc<RefCell<Inner>>,
}

impl Eyedropper {
    pub fn start(api: Arc<Api>, on_pick: Box<dyn FnMut(Rgb)>, on_cancel: Box<dyn FnMut()>) -> Eyedropper {
        Eyedropper::with_env(api, on_pick, on_cancel)
    }

    /// Пипетка с подставным окружением (тесты: окна остаются скрытыми, цвет — заданный).
    /// Работающая пипетка этого потока сначала отменяется (её `on_cancel` вызывается).
    /// Если окна не создались — пипетка сразу неактивна, обработчики не вызываются.
    pub fn with_env(env: Arc<dyn ScreenEnv>, on_pick: Box<dyn FnMut(Rgb)>, on_cancel: Box<dyn FnMut()>) -> Eyedropper {
        // Вторая пипетка отменяет первую; обработчик отмены мог запустить ещё одну — их тоже.
        for _ in 0..4 {
            match take_current() {
                Some(previous) => cancel(&previous),
                None => break,
            }
        }
        let cursor = env.cursor();
        let model = PickerModel::new(env.now(), env.scale_at(cursor));
        let font = Font::new("Segoe UI", px(13.0, model.scale()), false);
        let (width, height) = model.size();
        let inner = Rc::new(RefCell::new(Inner {
            env: env.clone(),
            model,
            overlay: 0,
            loupe: 0,
            font: Some(font),
            canvas: None,
            dirty: true,
            on_pick: Some(on_pick),
            on_cancel: Some(on_cancel),
        }));
        let screen = env.virtual_screen();
        let overlay = create(
            OVERLAY_CLASS,
            overlay_proc,
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            screen,
            OVERLAY_ALPHA,
            &inner,
        );
        let loupe = create(
            LOUPE_CLASS,
            loupe_proc,
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            Rect { l: screen.l, t: screen.t, r: screen.l + width, b: screen.t + height },
            LOUPE_ALPHA,
            &inner,
        );
        {
            let mut state = inner.borrow_mut();
            state.overlay = overlay;
            state.loupe = loupe;
            if overlay == 0 || loupe == 0 {
                state.model.cancel();
                state.on_pick = None;
                state.on_cancel = None;
                state.font = None;
                state.overlay = 0;
                state.loupe = 0;
                drop(state);
                destroy(loupe);
                destroy(overlay);
                return Eyedropper { inner };
            }
        }
        set_current(Some(inner.clone()));
        env.show(overlay, screen, true);
        // SAFETY: окно своё; таймер умирает вместе с окном.
        unsafe {
            SetTimer(to_hwnd(overlay), TIMER_FRAME, FRAME_MS, None);
        }
        tick(&inner); // первый кадр сразу (N 1466): лупа появляется уже у курсора
        Eyedropper { inner }
    }

    /// Отменить (обработчик отмены вызывается один раз; у законченной — ничего).
    pub fn cancel(&mut self) {
        cancel(&self.inner);
    }

    pub fn active(&self) -> bool {
        self.inner.try_borrow().is_ok_and(|i| i.model.active())
    }

    /// Окно-накрытие (0 — пипетка закончилась).
    pub fn overlay(&self) -> Hwnd {
        self.inner.try_borrow().map_or(0, |i| i.overlay)
    }

    /// Окно лупы (0 — пипетка закончилась).
    pub fn loupe(&self) -> Hwnd {
        self.inner.try_borrow().map_or(0, |i| i.loupe)
    }

    /// Последний цвет под курсором.
    pub fn last(&self) -> Option<Rgb> {
        self.inner.try_borrow().ok().and_then(|i| i.model.last())
    }
}

fn create(class: &str, proc: WndProc, ex: WINDOW_EX_STYLE, r: Rect, alpha: u8, inner: &Rc<RefCell<Inner>>) -> Hwnd {
    let atom = register_class(class, proc, false);
    if atom == 0 {
        return 0;
    }
    // SAFETY: класс зарегистрирован; окно создаётся скрытым; привязка к состоянию ставится после
    // создания и снимается в WM_NCDESTROY.
    unsafe {
        let created = CreateWindowExW(
            ex,
            PCWSTR(atom as usize as *const u16),
            w!("Закладка"),
            WS_POPUP,
            r.l,
            r.t,
            (r.r - r.l).max(1),
            (r.b - r.t).max(1),
            HWND(std::ptr::null_mut()),
            HMENU(std::ptr::null_mut()),
            instance(),
            None,
        );
        let Ok(window) = created else { return 0 };
        if window.0.is_null() {
            return 0;
        }
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA);
        let link = Box::into_raw(Box::new(Rc::downgrade(inner)));
        SetWindowLongPtrW(window, GWLP_USERDATA, link as isize);
        from_hwnd(window)
    }
}

fn destroy(hwnd: Hwnd) {
    if hwnd != 0 {
        // SAFETY: окно своё; WM_NCDESTROY освобождает привязку.
        unsafe {
            let _ = DestroyWindow(to_hwnd(hwnd));
        }
    }
}

fn tick(inner: &Rc<RefCell<Inner>>) {
    let step = {
        let Ok(mut state) = inner.try_borrow_mut() else { return };
        let env = state.env.clone();
        state.model.tick(&*env)
    };
    apply(inner, step);
}

fn cancel(inner: &Rc<RefCell<Inner>>) {
    let step = match inner.try_borrow_mut() {
        Ok(mut state) => state.model.cancel(),
        Err(_) => return,
    };
    apply(inner, step);
}

/// Выполнить шаг модели. Состояние не занято (вызывается после заимствования).
fn apply(inner: &Rc<RefCell<Inner>>, step: Step) {
    match step {
        Step::Idle => {}
        Step::Frame { redraw, moved } => {
            let (env, loupe, size) = {
                let Ok(mut state) = inner.try_borrow_mut() else { return };
                if redraw {
                    state.dirty = true;
                }
                (state.env.clone(), state.loupe, state.model.size())
            };
            if redraw && loupe != 0 {
                // SAFETY: окно своё.
                unsafe {
                    let _ = InvalidateRect(to_hwnd(loupe), None, BOOL(0));
                }
            }
            if let Some(at) = moved {
                env.show(loupe, Rect { l: at.x, t: at.y, r: at.x + size.0, b: at.y + size.1 }, false);
            }
        }
        Step::Done(outcome) => finish(inner, outcome),
    }
}

/// Конец: окна уничтожить, отпустить шрифт и холст, потом (без заимствования состояния) вызвать
/// обработчик — из него можно снова запускать пипетку.
fn finish(inner: &Rc<RefCell<Inner>>, outcome: Outcome) {
    let (overlay, loupe, on_pick, on_cancel) = match inner.try_borrow_mut() {
        Ok(mut state) => {
            state.canvas = None;
            state.font = None;
            (
                std::mem::take(&mut state.overlay),
                std::mem::take(&mut state.loupe),
                state.on_pick.take(),
                state.on_cancel.take(),
            )
        }
        Err(_) => return,
    };
    forget_current(inner);
    destroy(loupe);
    destroy(overlay);
    // Паника в обработчике не должна выйти наружу: место паники пишет panic hook.
    match outcome {
        Outcome::Pick(color) => {
            if let Some(mut callback) = on_pick {
                let _ = catch_unwind(AssertUnwindSafe(|| callback(color)));
            }
        }
        Outcome::Cancel => {
            if let Some(mut callback) = on_cancel {
                let _ = catch_unwind(AssertUnwindSafe(|| callback()));
            }
        }
    }
}

/// Состояние окна по GWLP_USERDATA (Box<Weak>, ставит `create`).
fn link(hwnd: Hwnd) -> Option<Rc<RefCell<Inner>>> {
    // SAFETY: в GWLP_USERDATA окон этого модуля только наш Box<Weak> (или 0 до привязки).
    unsafe {
        let link = GetWindowLongPtrW(to_hwnd(hwnd), GWLP_USERDATA) as *const Weak<RefCell<Inner>>;
        if link.is_null() {
            return None;
        }
        (*link).upgrade()
    }
}

fn unlink(hwnd: Hwnd) {
    // SAFETY: привязку ставит `create`, снимаем один раз (WM_NCDESTROY).
    unsafe {
        let link = SetWindowLongPtrW(to_hwnd(hwnd), GWLP_USERDATA, 0) as *mut Weak<RefCell<Inner>>;
        if !link.is_null() {
            drop(Box::from_raw(link));
        }
    }
}

/// Накрытие: курсор-крест, щелчки, Esc, правая кнопка, кадры.
fn overlay_proc(hwnd: Hwnd, msg: u32, wparam: usize, _lparam: isize) -> Option<isize> {
    let window = to_hwnd(hwnd);
    match msg {
        WM_ERASEBKGND => return Some(1),
        WM_SETCURSOR => {
            // SAFETY: системный курсор, освобождать не нужно.
            unsafe {
                if let Ok(cross) = LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_CROSS) {
                    SetCursor(cross);
                }
            }
            return Some(1);
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: обработка WM_PAINT своего окна; стоковая кисть не удаляется.
            unsafe {
                let dc = BeginPaint(window, &mut ps);
                FillRect(dc, &ps.rcPaint, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                let _ = EndPaint(window, &ps);
            }
            return Some(0);
        }
        WM_NCDESTROY => {
            unlink(hwnd);
            return None;
        }
        _ => {}
    }
    let inner = link(hwnd)?;
    let step = {
        // Занято (сообщение пришло изнутри вызова Win32 отсюда же) — обработка по умолчанию.
        let Ok(mut state) = inner.try_borrow_mut() else { return None };
        if state.overlay != hwnd {
            return None;
        }
        let env = state.env.clone();
        match msg {
            WM_LBUTTONDOWN => {
                state.model.press(&*env);
                return Some(0);
            }
            WM_LBUTTONUP => state.model.release(),
            WM_RBUTTONDOWN => return Some(0),
            WM_RBUTTONUP => state.model.cancel(), // при отпускании: без контекстного меню под пипеткой
            WM_KEYDOWN if wparam == VK_ESCAPE as usize => state.model.cancel(),
            WM_CLOSE => state.model.cancel(),
            WM_TIMER if wparam == TIMER_FRAME => state.model.tick(&*env),
            WM_DESTROY => {
                // Окно уничтожил кто-то другой: пипетка кончается отменой.
                state.overlay = 0;
                state.model.cancel()
            }
            _ => return None,
        }
    };
    apply(&inner, step);
    Some(0)
}

/// Лупа: только рисование; прозрачна для мыши и не активируется.
fn loupe_proc(hwnd: Hwnd, msg: u32, _wparam: usize, _lparam: isize) -> Option<isize> {
    let window = to_hwnd(hwnd);
    match msg {
        WM_ERASEBKGND => Some(1),
        WM_NCHITTEST => Some(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_NCDESTROY => {
            unlink(hwnd);
            None
        }
        WM_PAINT => {
            let inner = link(hwnd);
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: обработка WM_PAINT своего окна.
            unsafe {
                let dc = BeginPaint(window, &mut ps);
                if let Some(inner) = inner {
                    if let Ok(mut state) = inner.try_borrow_mut() {
                        let state = &mut *state;
                        if state.loupe == hwnd {
                            if state.dirty || state.canvas.is_none() {
                                if let Some(font) = state.font.as_ref() {
                                    state.canvas = Some(render_loupe(&state.model, font));
                                    state.dirty = false;
                                }
                            }
                            if let Some(canvas) = state.canvas.as_ref() {
                                canvas.present(dc.0 as isize, 0, 0);
                            }
                        }
                    }
                }
                let _ = EndPaint(window, &ps);
            }
            Some(0)
        }
        _ => None,
    }
}
