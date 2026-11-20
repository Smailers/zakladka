//! Тесты окна цветов и пипетки (часть 8): `ColorDialogTests` (4) и `EyedropperTests` (3) из
//! test_panel.py — на моделях (`ColorModel`, `PickerModel`) и на скрытых окнах с подставным
//! окружением; раскладка; утечки GDI/USER.
//!
//! Окна только скрытые и за экраном (рабочая область и виртуальный экран подставные:
//! -32000…-30000), фокус не берётся, мышь не захватывается, системное окно выбора цвета не
//! открывается, буфер обмена и клавиатура не трогаются.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsWindowVisible, SendMessageW, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONUP,
    WM_TIMER,
};

use zakladka::core::color::{Colors, PRESETS};
use zakladka::types::{to_hwnd, Hwnd, LabelColor, Point, Rect, Rgb};
use zakladka::ui::color_dialog::{render, ColorDialog, DialogEnv, DialogFonts, Layout, Target};
use zakladka::ui::color_model::{ColorModel, Field, LabelChoice, Pick, DEFAULT_COLORS};
use zakladka::ui::eyedropper::{render_loupe, Eyedropper, Outcome, PickerModel, ScreenEnv, Step, CELLS};
use zakladka::ui::gfx::{palette, px, Canvas, Font};
use zakladka::win::Bitmap;

const OFF_SCREEN: Rect = Rect { l: -32000, t: -32000, r: -30000, b: -30000 };
/// Виртуальный экран теста Python: (-32000, -32000, 400, 300).
const VIRTUAL: Rect = Rect { l: -32000, t: -32000, r: -31600, b: -31700 };
const CURSOR: Point = Point { x: -30500, y: -30500 };
/// BGRA (0x33, 0x22, 0x11, 0xFF) в Python.
const PIXEL: Rgb = Rgb(0x11, 0x22, 0x33);
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;
const VK_RIGHT: usize = 0x27;

fn rgb(hex: &str) -> Rgb {
    Rgb::parse(hex).unwrap()
}

/// `CONFIG` из ColorDialogTests: {"color": "#ffd400", "opacity": 0.3, "bar_color": "#FF7A00", "label_color": "auto"}.
fn config() -> Colors {
    Colors { color: rgb("#FFD400"), opacity: 0.3, bar_color: rgb("#FF7A00"), label_color: LabelColor::Auto }
}

// ------------------------------------------------------------ подставное окружение

struct FakeEnv {
    cursor: Cell<Point>,
    pixel: Cell<Rgb>,
    capture_ok: Cell<bool>,
    keys: RefCell<Vec<i32>>,
    now: Cell<f64>,
    work: Cell<Rect>,
    shows: RefCell<Vec<(Hwnd, Rect, bool)>>,
    hides: RefCell<Vec<Hwnd>>,
    chosen: Cell<Option<Rgb>>,
    choose_calls: RefCell<Vec<(Rgb, String)>>,
    light: Cell<bool>,
}

impl FakeEnv {
    fn new() -> Arc<FakeEnv> {
        Arc::new(FakeEnv {
            cursor: Cell::new(CURSOR),
            pixel: Cell::new(PIXEL),
            capture_ok: Cell::new(true),
            keys: RefCell::new(Vec::new()),
            now: Cell::new(1000.0),
            work: Cell::new(OFF_SCREEN),
            shows: RefCell::new(Vec::new()),
            hides: RefCell::new(Vec::new()),
            chosen: Cell::new(None),
            choose_calls: RefCell::new(Vec::new()),
            light: Cell::new(true),
        })
    }

    /// Окно-накрытие последней пипетки: показано с фокусом во весь виртуальный экран.
    fn overlay(&self) -> Hwnd {
        self.shows.borrow().iter().rev().find(|(_, r, activate)| *activate && *r == VIRTUAL).map_or(0, |s| s.0)
    }

    /// Сколько раз лупа сдвигалась за курсором (показ без фокуса).
    fn loupe_moves(&self) -> usize {
        self.shows.borrow().iter().filter(|(_, _, activate)| !*activate).count()
    }
}

impl ScreenEnv for FakeEnv {
    fn cursor(&self) -> Point {
        self.cursor.get()
    }
    fn work_area(&self, _: Point) -> Rect {
        self.work.get()
    }
    fn scale_at(&self, _: Point) -> f64 {
        1.0
    }
    fn virtual_screen(&self) -> Rect {
        VIRTUAL
    }
    fn capture(&self, r: Rect) -> Option<Bitmap> {
        if !self.capture_ok.get() {
            return None;
        }
        let (w, h) = (r.r - r.l, r.b - r.t);
        let c = self.pixel.get();
        Some(Bitmap { width: w, height: h, bgra: [c.2, c.1, c.0, 0xFF].repeat((w * h) as usize) })
    }
    fn key_down(&self, vk: i32) -> bool {
        self.keys.borrow().contains(&vk)
    }
    fn now(&self) -> f64 {
        self.now.get()
    }
    fn show(&self, hwnd: Hwnd, r: Rect, activate: bool) {
        self.shows.borrow_mut().push((hwnd, r, activate)); // только запись: окно не показывается
    }
    fn hide(&self, hwnd: Hwnd) {
        self.hides.borrow_mut().push(hwnd);
    }
}

impl DialogEnv for FakeEnv {
    fn light(&self) -> bool {
        self.light.get()
    }
    fn choose_color(&self, _owner: Hwnd, start: Rgb, title: &str) -> Option<Rgb> {
        self.choose_calls.borrow_mut().push((start, title.to_string()));
        self.chosen.get()
    }
    fn icon(&self, _: Rgb) -> Option<isize> {
        None
    }
}

fn lparam(p: Point) -> LPARAM {
    LPARAM(((p.y as u16 as isize) << 16) | p.x as u16 as isize)
}

fn send(hwnd: Hwnd, msg: u32, wparam: usize, lparam: LPARAM) {
    // SAFETY: своё скрытое окно в этом же потоке.
    unsafe {
        SendMessageW(to_hwnd(hwnd), msg, WPARAM(wparam), lparam);
    }
}

fn exists(hwnd: Hwnd) -> bool {
    // SAFETY: проверка дескриптора.
    hwnd != 0 && unsafe { IsWindow(to_hwnd(hwnd)).as_bool() }
}

fn visible(hwnd: Hwnd) -> bool {
    // SAFETY: проверка своего окна.
    unsafe { IsWindowVisible(to_hwnd(hwnd)).as_bool() }
}

fn gui_objects() -> (u32, u32) {
    // SAFETY: только чтение счётчиков своего процесса.
    unsafe { (GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS)) }
}

fn recording_dialog(env: Arc<FakeEnv>, current: Colors) -> (ColorDialog, Rc<RefCell<Vec<Colors>>>) {
    let saved = Rc::new(RefCell::new(Vec::new()));
    let sink = saved.clone();
    let dialog = ColorDialog::with_env(env, current, Box::new(move |c| sink.borrow_mut().push(c)));
    assert!(dialog.is_open());
    assert!(!visible(dialog.hwnd()));
    (dialog, saved)
}

struct Picks {
    picked: Rc<RefCell<Vec<Rgb>>>,
    cancelled: Rc<Cell<u32>>,
}

fn start_picker(env: &Arc<FakeEnv>) -> (Eyedropper, Picks) {
    let picks = Picks { picked: Rc::new(RefCell::new(Vec::new())), cancelled: Rc::new(Cell::new(0)) };
    let (picked, cancelled) = (picks.picked.clone(), picks.cancelled.clone());
    let picker = Eyedropper::with_env(
        env.clone(),
        Box::new(move |c| picked.borrow_mut().push(c)),
        Box::new(move || cancelled.set(cancelled.get() + 1)),
    );
    (picker, picks)
}

/// Середина части окна при раскладке `layout`.
fn center(layout: &Layout, target: Target) -> Point {
    let r = layout.rect(target).unwrap_or_else(|| panic!("нет {target:?}"));
    Point { x: (r.l + r.r) / 2, y: (r.t + r.b) / 2 }
}

/// Щелчок по части скрытого окна цветов; раскладка — по модели-зеркалу.
fn click(hwnd: Hwnd, mirror: &ColorModel, target: Target) {
    let layout = Layout::new(&DialogFonts::new(1.0), mirror);
    let p = center(&layout, target);
    assert_eq!(layout.hit(p), Some(target));
    send(hwnd, WM_LBUTTONDOWN, 1, lparam(p));
    send(hwnd, WM_LBUTTONUP, 0, lparam(p));
}

// ------------------------------------------------------------ ColorDialogTests

#[test]
fn test_save_returns_same_keys() {
    let expected =
        Colors { color: rgb("#FFD400"), opacity: 0.3, bar_color: rgb("#FF7A00"), label_color: LabelColor::Auto };
    assert_eq!(ColorModel::new(config()).result(), expected);
    // окно: Enter сохраняет один раз, окно уничтожено до вызова on_save
    let env = FakeEnv::new();
    let (dialog, saved) = recording_dialog(env.clone(), config());
    let hwnd = dialog.hwnd();
    assert_eq!(env.shows.borrow().last().map(|s| (s.0, s.2)), Some((hwnd, true))); // показ с фокусом
    send(hwnd, WM_KEYDOWN, VK_RETURN, LPARAM(0));
    assert!(!dialog.is_open());
    assert!(!exists(hwnd));
    assert_eq!(*saved.borrow(), [expected]);
}

#[test]
fn test_changes_and_contrast() {
    let mut m = ColorModel::new(config());
    m.set(Field::Fill, rgb("#34C759"));
    assert!(m.set_opacity_percent(45));
    m.set(Field::Accent, rgb("#FFD400"));
    assert!(m.choose_label(LabelChoice::White));
    let (text, good) = m.contrast_text();
    assert!(!good);
    assert!(text.contains("Плохо читается — лучше чёрный"), "{text}");
    assert!(text.starts_with("Контраст 1,"), "{text}"); // запятая вместо точки
    assert!(m.choose_label(LabelChoice::Auto));
    let (text, good) = m.contrast_text();
    assert!(good);
    assert!(text.ends_with("Хорошо читается"), "{text}");
    assert_eq!(m.selected_label(), LabelChoice::Auto);
    m.set(Field::Accent, rgb("#123456")); // не из готовых: свой образец
    assert_eq!(m.custom_swatch(Field::Accent), Some(rgb("#123456")));
    assert_eq!(m.custom_swatch(Field::Fill), None);
    assert_eq!(m.custom_tip(Field::Accent), "Свой цвет #123456");
    let layout = Layout::new(&DialogFonts::new(1.0), &m);
    assert!(layout.accent.custom.is_some() && layout.fill.custom.is_none());
    assert_eq!(
        m.result(),
        Colors { color: rgb("#34C759"), opacity: 0.45, bar_color: rgb("#123456"), label_color: LabelColor::Auto }
    );
}

#[test]
fn test_reset_and_cancel() {
    let start = Colors {
        color: rgb("#FF5CA8"),
        opacity: 0.55,
        bar_color: rgb("#9B6CFF"),
        label_color: LabelColor::Custom(rgb("#FFFFFF")),
    };
    let mut m = ColorModel::new(start);
    assert_eq!(m.selected_label(), LabelChoice::White);
    m.reset();
    assert_eq!(m.result(), DEFAULT_COLORS);
    // окно: Esc — закрыто, ничего не сохранено; WM_CLOSE (крестик) — так же
    let env = FakeEnv::new();
    for msg in [WM_KEYDOWN, WM_CLOSE] {
        let (dialog, saved) = recording_dialog(env.clone(), start);
        let hwnd = dialog.hwnd();
        send(hwnd, msg, VK_ESCAPE, LPARAM(0));
        assert!(!dialog.is_open());
        assert!(!exists(hwnd));
        assert!(saved.borrow().is_empty());
    }
}

#[test]
fn test_pipette_hides_and_restores() {
    // модель: окно прячется, пипетка отдаёт цвет при отпускании, окно возвращается
    let env = FakeEnv::new();
    let mut m = ColorModel::new(config());
    let mut picker = PickerModel::new(env.now(), 1.0);
    assert!(picker.active());
    m.pipette_started(Field::Fill);
    assert!(m.hidden());
    picker.press(&*env);
    let Step::Done(Outcome::Pick(color)) = picker.release() else { panic!("нет цвета") };
    assert!(m.pipette_finished(Some(color)));
    assert_eq!(m.get(Field::Fill), PIXEL);
    assert!(!m.hidden());

    // окна: «Пипетка» прячет окно цветов, щелчок по накрытию отдаёт цвет и возвращает окно
    let (dialog, saved) = recording_dialog(env.clone(), config());
    let hwnd = dialog.hwnd();
    let mirror = ColorModel::new(config());
    click(hwnd, &mirror, Target::Pipette(Field::Fill));
    assert!(dialog.hidden());
    assert_eq!(env.hides.borrow().last(), Some(&hwnd));
    let overlay = env.overlay();
    assert!(exists(overlay) && !visible(overlay));
    send(overlay, WM_LBUTTONDOWN, 1, lparam(Point { x: 10, y: 10 }));
    send(overlay, WM_LBUTTONUP, 0, lparam(Point { x: 10, y: 10 }));
    assert!(!exists(overlay));
    assert!(!dialog.hidden());
    assert_eq!(env.shows.borrow().last().map(|s| (s.0, s.2)), Some((hwnd, true))); // вернулось с фокусом
    assert_eq!(dialog.values().map(|v| v.color), Some(PIXEL));
    // пипетка, затем закрытие окна цветов — пипетка отменяется, накрытие уничтожено
    click(hwnd, &mirror_with(Field::Fill, PIXEL), Target::Pipette(Field::Accent));
    let overlay = env.overlay();
    assert!(exists(overlay));
    let mut dialog = dialog;
    dialog.close();
    assert!(!exists(overlay));
    assert!(!exists(hwnd));
    assert!(saved.borrow().is_empty());
}

fn mirror_with(field: Field, color: Rgb) -> ColorModel {
    let mut m = ColorModel::new(config());
    m.set(field, color);
    m
}

// ------------------------------------------------------------ EyedropperTests

#[test]
fn test_magnifier_and_pick() {
    let env = FakeEnv::new();
    let mut p = PickerModel::new(env.now(), 1.0);
    assert!(p.active());
    let step = p.tick(&*env);
    assert_eq!(p.last(), Some(PIXEL));
    assert_eq!(p.label(), "#112233");
    assert!(p.cells().iter().all(|c| *c == Some(PIXEL)));
    let Step::Frame { redraw: true, moved: Some(at) } = step else { panic!("{step:?}") };
    assert_eq!(at, Point { x: CURSOR.x + 24, y: CURSOR.y + 24 }); // лупа пошла за курсором
    assert_eq!(p.tick(&*env), Step::Frame { redraw: false, moved: None }); // ничего не изменилось
    p.press(&*env);
    assert_eq!(p.release(), Step::Done(Outcome::Pick(PIXEL)));
    assert!(!p.active());
    assert_eq!(p.release(), Step::Idle);

    // окна: накрытие скрыто, лупа сдвинута, щелчок отдаёт цвет и уничтожает окна
    let (picker, picks) = start_picker(&env);
    assert!(picker.active());
    assert_eq!(picker.last(), Some(PIXEL));
    assert!(env.loupe_moves() >= 1);
    let (overlay, loupe) = (picker.overlay(), picker.loupe());
    assert!(exists(overlay) && exists(loupe) && !visible(overlay) && !visible(loupe));
    send(overlay, WM_LBUTTONDOWN, 1, LPARAM(0));
    send(overlay, WM_LBUTTONUP, 0, LPARAM(0));
    assert_eq!(*picks.picked.borrow(), [PIXEL]);
    assert_eq!(picks.cancelled.get(), 0);
    assert!(!picker.active());
    assert!(!exists(overlay) && !exists(loupe));
}

#[test]
fn test_escape_and_cancel() {
    let env = FakeEnv::new();
    let mut p = PickerModel::new(env.now(), 1.0);
    env.keys.borrow_mut().push(0x1B);
    assert_eq!(p.tick(&*env), Step::Done(Outcome::Cancel)); // Esc и без клавиатуры у накрытия
    assert!(!p.active());
    env.keys.borrow_mut().clear();
    let mut p = PickerModel::new(env.now(), 1.0);
    assert_eq!(p.cancel(), Step::Done(Outcome::Cancel));
    assert_eq!(p.cancel(), Step::Idle);

    // окна: Esc через GetAsyncKeyState на кадре
    let (picker, picks) = start_picker(&env);
    let overlay = picker.overlay();
    env.keys.borrow_mut().push(0x1B);
    send(overlay, WM_TIMER, 1, LPARAM(0));
    env.keys.borrow_mut().clear();
    assert!(!picker.active());
    assert_eq!(picks.cancelled.get(), 1);
    assert!(!exists(overlay));
    // Esc на накрытии, правая кнопка, Alt+F4
    for (msg, wparam) in [(WM_KEYDOWN, VK_ESCAPE), (WM_RBUTTONUP, 0), (WM_CLOSE, 0)] {
        let (picker, picks) = start_picker(&env);
        let overlay = picker.overlay();
        send(overlay, msg, wparam, LPARAM(0));
        assert!(!picker.active());
        assert_eq!(picks.cancelled.get(), 1);
        assert!(picks.picked.borrow().is_empty());
        assert!(!exists(overlay));
    }
    // отмена дважды — обработчик один раз
    let (mut picker, picks) = start_picker(&env);
    picker.cancel();
    picker.cancel();
    assert_eq!(picks.cancelled.get(), 1);
    assert!(picks.picked.borrow().is_empty());
}

#[test]
fn test_second_picker_cancels_first() {
    let env = FakeEnv::new();
    let (first, first_picks) = start_picker(&env);
    let (mut second, second_picks) = start_picker(&env);
    assert!(!first.active());
    assert_eq!(first_picks.cancelled.get(), 1);
    assert!(!exists(first.overlay()));
    assert!(second.active());
    second.cancel();
    assert_eq!(second_picks.cancelled.get(), 1);
}

// ------------------------------------------------------------ сверх Python

#[test]
fn eyedropper_timeout_press_fallback_and_edges() {
    let env = FakeEnv::new();
    // 120 с без щелчка — отмена
    let mut p = PickerModel::new(env.now(), 1.0);
    env.now.set(env.now() + 119.0);
    assert!(matches!(p.tick(&*env), Step::Frame { .. }));
    env.now.set(env.now() + 1.5);
    assert_eq!(p.tick(&*env), Step::Done(Outcome::Cancel));
    // снимок не удался: при нажатии — последний цвет лупы; без него отпускание ничего не делает
    let mut p = PickerModel::new(env.now(), 1.0);
    env.capture_ok.set(false);
    p.press(&*env);
    assert_eq!(p.release(), Step::Idle);
    assert!(p.active());
    env.capture_ok.set(true);
    p.tick(&*env);
    env.capture_ok.set(false);
    p.press(&*env);
    assert_eq!(p.release(), Step::Done(Outcome::Pick(PIXEL)));
    env.capture_ok.set(true);
    // у правого и нижнего края лупа — с другой стороны курсора
    let mut p = PickerModel::new(env.now(), 1.0);
    let (w, h) = p.size();
    assert_eq!((w, h), (CELLS * 11 + 2, CELLS * 11 + 26 + 2));
    let work = Rect { l: 0, t: 0, r: 1920, b: 1080 };
    assert_eq!(p.follow(Point { x: 1900, y: 1070 }, work), Some(Point { x: 1900 - 24 - w, y: 1070 - 24 - h }));
    assert_eq!(p.follow(Point { x: 1900, y: 1070 }, work), None);
    assert_eq!(p.follow(Point { x: -1500, y: 300 }, work), Some(Point { x: -1476, y: 324 })); // второй монитор слева
    // масштаб 150 %: клетка 16, строка 39
    let p = PickerModel::new(0.0, 1.5);
    assert_eq!((p.cell(), p.footer()), (16, 39));
    // окно: таймаут на кадре
    let (picker, picks) = start_picker(&env);
    env.now.set(env.now() + 121.0);
    send(picker.overlay(), WM_TIMER, 1, LPARAM(0));
    assert!(!picker.active());
    assert_eq!(picks.cancelled.get(), 1);
}

#[test]
fn hidden_dialog_clicks_choose_and_slider() {
    let env = FakeEnv::new();
    let (dialog, saved) = recording_dialog(env.clone(), config());
    let hwnd = dialog.hwnd();
    let mut mirror = ColorModel::new(config());
    // готовый цвет заливки
    click(hwnd, &mirror, Target::Swatch(Field::Fill, 2));
    mirror.set(Field::Fill, PRESETS[2].0);
    assert_eq!(dialog.values().unwrap().color, rgb("#34C759"));
    // надпись «Белый»
    click(hwnd, &mirror, Target::Label(LabelChoice::White));
    mirror.choose_label(LabelChoice::White);
    assert_eq!(dialog.values().unwrap().label_color, LabelColor::Custom(rgb("#FFFFFF")));
    // «Другой…» акцента: системное окно (подставное) с начальным цветом и заголовком
    env.chosen.set(Some(rgb("#123456")));
    click(hwnd, &mirror, Target::Other(Field::Accent));
    mirror.apply_pick(Pick::Field(Field::Accent), rgb("#123456"));
    assert_eq!(env.choose_calls.borrow().last().cloned(), Some((rgb("#FF7A00"), "Закладка — цвет".to_string())));
    assert_eq!(dialog.values().unwrap().bar_color, rgb("#123456"));
    // свой образец появился: «Пипетка» акцента сдвинулась, щелчок по своему образцу ничего не меняет
    click(hwnd, &mirror, Target::Custom(Field::Accent));
    assert_eq!(dialog.values().unwrap().bar_color, rgb("#123456"));
    // «Другой…» надписи: отмена системного окна ничего не меняет
    env.chosen.set(None);
    click(hwnd, &mirror, Target::Label(LabelChoice::Custom));
    assert_eq!(
        env.choose_calls.borrow().last().cloned(),
        Some((rgb("#FFFFFF"), "Закладка — цвет надписи".to_string()))
    );
    assert_eq!(dialog.values().unwrap().label_color, LabelColor::Custom(rgb("#FFFFFF")));
    // ползунок: нажатие у левого края желоба — 20 %, правее стрелкой — 21 %
    let layout = Layout::new(&DialogFonts::new(1.0), &mirror);
    let p = Point { x: layout.trough.l, y: (layout.trough.t + layout.trough.b) / 2 };
    send(hwnd, WM_LBUTTONDOWN, 1, lparam(p));
    send(hwnd, WM_LBUTTONUP, 0, lparam(p));
    assert_eq!(dialog.values().unwrap().opacity, 0.2);
    send(hwnd, WM_KEYDOWN, VK_RIGHT, LPARAM(0));
    assert_eq!(dialog.values().unwrap().opacity, 0.21);
    mirror.set_opacity_percent(21);
    // нажатие на одной части и отпускание на другой — ничего
    let (a, b) = (center(&layout, Target::Reset), center(&layout, Target::Save));
    send(hwnd, WM_LBUTTONDOWN, 1, lparam(a));
    send(hwnd, WM_LBUTTONUP, 0, lparam(b));
    assert!(dialog.is_open());
    assert_eq!(dialog.values().unwrap().opacity, 0.21);
    // «По умолчанию», затем «Сохранить»
    click(hwnd, &mirror, Target::Reset);
    assert_eq!(dialog.values(), Some(DEFAULT_COLORS));
    click(hwnd, &ColorModel::new(DEFAULT_COLORS), Target::Save);
    assert!(!dialog.is_open());
    assert!(!exists(hwnd));
    assert_eq!(*saved.borrow(), [DEFAULT_COLORS]);
}

#[test]
fn layout_targets_hit_themselves() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let fonts = DialogFonts::new(scale);
        let mut m = ColorModel::new(config());
        m.set(Field::Accent, rgb("#123456"));
        let layout = Layout::new(&fonts, &m);
        let mut targets = vec![Target::Slider, Target::Reset, Target::Cancel, Target::Save, Target::Custom(Field::Accent)];
        for field in [Field::Fill, Field::Accent] {
            targets.extend((0..8).map(|i| Target::Swatch(field, i)));
            targets.extend([Target::Other(field), Target::Pipette(field)]);
        }
        targets.extend(LabelChoice::ALL.map(Target::Label));
        for t in &targets {
            assert_eq!(layout.hit(center(&layout, *t)), Some(*t), "{t:?} при {scale}");
            let r = layout.rect(*t).unwrap();
            assert!(r.l >= 0 && r.t >= 0 && r.r <= layout.size.0 && r.b <= layout.size.1, "{t:?} при {scale}");
        }
        assert_eq!(layout.rect(Target::Custom(Field::Fill)), None);
        assert!(layout.size.0 >= px(460.0, scale), "{:?}", layout.size);
        // размер окна не зависит от того, есть ли свой образец
        assert_eq!(Layout::new(&fonts, &ColorModel::new(config())).size, layout.size);
        // ползунок: края и середина
        assert_eq!(layout.slider_value(layout.trough.l - 50), 20);
        assert_eq!(layout.slider_value(layout.trough.r + 50), 60);
        let k = layout.knob(40);
        assert_eq!(layout.slider_value((k.l + k.r) / 2), 40);
        assert_eq!(layout.knob(20).l, layout.trough.l);
        assert_eq!(layout.knob(60).r, layout.trough.r);
    }
}

#[test]
fn render_draws_preview_and_selection() {
    let fonts = DialogFonts::new(1.0);
    let m = ColorModel::new(config());
    let layout = Layout::new(&fonts, &m);
    let (w, h) = layout.size;
    for light in [true, false] {
        let p = palette(light);
        let mut c = Canvas::new(w, h);
        render(&mut c, &m, &layout, &fonts, &p, Some(Target::Save), None);
        assert_eq!(c.pixel(2, 2), Some(p.bg));
        // светлая полоса образца белая, тёмная — #1E1E1E
        assert_eq!(c.pixel(layout.preview.l + 10, layout.preview.t + 26), Some(rgb("#FFFFFF")));
        assert_eq!(c.pixel(layout.preview.l + 10, layout.preview.t + 60 + 26), Some(rgb("#1E1E1E")));
        // движок ползунка на 30 % — цвет primary
        let k = layout.knob(30);
        assert_eq!(c.pixel((k.l + k.r) / 2, (k.t + k.b) / 2), Some(p.primary));
        // середина образца «Жёлтый» — жёлтый
        let r = layout.fill.presets[0];
        assert_eq!(c.pixel((r.l + r.r) / 2, (r.t + r.b) / 2), Some(rgb("#FFD400")));
    }
    // лупа: рамка, клетки, образец
    let env = FakeEnv::new();
    let mut picker = PickerModel::new(env.now(), 1.0);
    assert!(matches!(picker.tick(&*env), Step::Frame { redraw: true, .. }));
    let font = Font::new("Segoe UI", 13, false);
    let loupe = render_loupe(&picker, &font);
    assert_eq!(loupe.size(), picker.size());
    assert_eq!(loupe.pixel(0, 0), Some(rgb("#5A5A5A")));
    assert_eq!(loupe.pixel(5, 5), Some(PIXEL));
    let m = 1 + 4 * 11;
    assert_eq!(loupe.pixel(m, m + 5), Some(rgb("#FFFFFF"))); // белая рамка средней клетки
    assert_eq!(loupe.pixel(m - 1, m + 5), Some(rgb("#000000"))); // чёрная снаружи
    assert_eq!(loupe.pixel(13, 1 + 99 + 13), Some(PIXEL)); // образец в строке
}

#[test]
fn no_gdi_or_window_leaks() {
    let env = FakeEnv::new();
    let cycle = |env: &Arc<FakeEnv>| {
        let (dialog, _) = recording_dialog(env.clone(), config());
        send(dialog.hwnd(), WM_KEYDOWN, VK_RETURN, LPARAM(0));
        let (mut dialog, _) = recording_dialog(env.clone(), config());
        click(dialog.hwnd(), &ColorModel::new(config()), Target::Pipette(Field::Fill));
        let overlay = env.overlay();
        send(overlay, WM_TIMER, 1, LPARAM(0));
        send(overlay, WM_LBUTTONDOWN, 1, LPARAM(0));
        send(overlay, WM_LBUTTONUP, 0, LPARAM(0));
        dialog.close();
        let (mut picker, _) = start_picker(env);
        picker.cancel();
        let font = Font::new("Segoe UI", 13, false);
        drop(render_loupe(&PickerModel::new(0.0, 1.25), &font));
    };
    for _ in 0..3 {
        cycle(&env); // прогрев: классы, шрифты системы
    }
    let before = gui_objects();
    for _ in 0..30 {
        cycle(&env);
        env.shows.borrow_mut().clear();
    }
    let after = gui_objects();
    eprintln!("GDI {} -> {}, USER {} -> {}", before.0, after.0, before.1, after.1);
    // другие тесты идут параллельно и тоже создают объекты; утечка дала бы сотни
    assert!(after.0 <= before.0 + 30, "GDI {} -> {}", before.0, after.0);
    assert!(after.1 <= before.1 + 10, "USER {} -> {}", before.1, after.1);
}

/// Картинки окна цветов и лупы для сверки вида со снимком Python-версии:
/// `cargo test --test colors -- --ignored --nocapture` → %TEMP%\zakladka-part8\*.bmp
#[test]
#[ignore]
fn colors_bmp() {
    let dir = std::env::temp_dir().join("zakladka-part8");
    std::fs::create_dir_all(&dir).unwrap();
    let custom = Colors {
        color: rgb("#34C759"),
        opacity: 0.45,
        bar_color: rgb("#123456"),
        label_color: LabelColor::Custom(rgb("#FFFFFF")),
    };
    for (name, light, scale, colors, hover) in [
        ("light", true, 1.0, config(), None),
        ("dark", false, 1.0, config(), None),
        ("light-custom", true, 1.0, custom, Some(Target::Pipette(Field::Accent))),
        ("dark-custom-150", false, 1.5, custom, Some(Target::Save)),
    ] {
        let fonts = DialogFonts::new(scale);
        let m = ColorModel::new(colors);
        let layout = Layout::new(&fonts, &m);
        let mut c = Canvas::new(layout.size.0, layout.size.1);
        render(&mut c, &m, &layout, &fonts, &palette(light), hover, None);
        let path = dir.join(format!("colors-{name}.bmp"));
        c.save_bmp(&path).unwrap();
        println!("{} ({}x{})", path.display(), layout.size.0, layout.size.1);
    }
    let env = FakeEnv::new();
    env.pixel.set(rgb("#FF7A00"));
    for scale in [1.0, 1.5] {
        let mut p = PickerModel::new(env.now(), scale);
        p.tick(&*env);
        let font = Font::new("Segoe UI", px(13.0, scale), false);
        let path = dir.join(format!("loupe-{}.bmp", (scale * 100.0) as i32));
        render_loupe(&p, &font).save_bmp(&path).unwrap();
        println!("{}", path.display());
    }
}
