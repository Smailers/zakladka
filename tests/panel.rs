//! Тесты панели (часть 7): `PanelTests` из test_panel.py на `PanelModel`, замер открытия,
//! окно панели (скрытое, подставное окружение), утечки GDI/USER.
//!
//! Окна только скрытые и за экраном (рабочая область подставная: -32000…-30000), фокус не
//! берётся, буфер обмена и клавиатура не трогаются.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsWindowVisible, SendMessageW, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_TIMER,
};

use zakladka::types::{to_hwnd, Hwnd, Point, Rect, Rgb, Status};
use zakladka::ui::gfx::Canvas;
use zakladka::ui::panel::{paint, Panel, PanelEnv, PanelFonts};
use zakladka::ui::panel_model::{
    Draw, Focus, Item, Key, Op, PanelAction, PanelModel, PanelRow, PanelState, ShowContext, Tag, Zone, ROW_HEIGHT,
};

const OFF_SCREEN: Rect = Rect { l: -32000, t: -32000, r: -30000, b: -30000 };
const ANCHOR: Point = Point { x: -30100, y: -30050 };
const PROGRAMS: [&str; 5] = ["claude.exe", "chatgpt.exe", "notepad.exe", "msedge.exe", "editors.exe"];
const CLOSED_HINT: &str = zakladka::core::text::CLOSED_HINT;

fn state() -> PanelState {
    PanelState {
        hotkey: "Ctrl+Alt+Z".into(),
        extra_hotkey: "Ctrl+Alt+Shift+Z".into(),
        undo: None,
        marks_visible: true,
        hints: true,
        accent: Rgb(0xFF, 0x7A, 0x00),
    }
}

fn make_rows(count: usize) -> Vec<PanelRow> {
    (0..count)
        .map(|i| PanelRow {
            id: format!("id{i}"),
            program: PROGRAMS[i % PROGRAMS.len()].into(),
            text: format!("Закладка {i}: ") + &"довольно длинный текст закладки ".repeat(4),
            status: Status::Shown,
            color: Rgb(0xFF, 0xD4, 0x00),
        })
        .collect()
}

/// Модель + шрифты GDI (без окон) + записанные действия, как `PanelTests` в Python.
struct Fixture {
    model: PanelModel,
    fonts: PanelFonts,
    calls: Vec<PanelAction>,
    now: f64,
    work: Rect,
    anchor: Point,
    cursor: Point,
    buttons: [bool; 3],
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            model: PanelModel::new(),
            fonts: PanelFonts::new(1.0),
            calls: Vec::new(),
            now: 100.0,
            work: OFF_SCREEN,
            anchor: ANCHOR,
            cursor: Point { x: -30500, y: -30500 },
            buttons: [false; 3],
        }
    }

    fn record(&mut self, action: Option<PanelAction>) {
        if let Some(action) = action {
            self.calls.push(action);
        }
    }

    fn show(&mut self, rows: Vec<PanelRow>, state: PanelState) -> bool {
        let ctx = ShowContext {
            now: self.now,
            anchor: self.anchor,
            work_area: self.work,
            scale: 1.0,
            light: true,
            rounded: true,
            buttons: self.buttons,
        };
        let (shown, action) = self.model.show(&ctx, rows, state, &self.fonts);
        self.record(action);
        shown
    }

    fn open(&mut self, rows: Vec<PanelRow>, state: PanelState) {
        assert!(self.show(rows, state));
        assert!(self.model.is_open());
    }

    fn scene(&self) -> Vec<Draw> {
        self.model.scene(&self.fonts)
    }

    /// Тексты команд с меткой `tag` (как `canvas_texts(tag)`).
    fn canvas_texts(&self, tag: Tag) -> Vec<String> {
        self.scene()
            .into_iter()
            .filter(|d| d.tag == tag)
            .filter_map(|d| match d.op {
                Op::Text { s, .. } => Some(s),
                _ => None,
            })
            .collect()
    }

    fn count(&self, tag: Tag) -> usize {
        self.scene().iter().filter(|d| d.tag == tag).count()
    }

    /// Надписи заголовка, пустой панели и кнопок (как `widget_texts`).
    fn widget_texts(&self) -> Vec<String> {
        self.scene()
            .into_iter()
            .filter_map(|d| match (d.tag, d.op) {
                (Tag::Title | Tag::Hotkey | Tag::Empty, Op::Text { s, .. }) => Some(s),
                (_, Op::Button { caption, .. }) => Some(caption),
                _ => None,
            })
            .collect()
    }

    fn row_point(&self, slot: i32, zone: Zone) -> Point {
        let area = self.model.rows_rect().expect("rows");
        let width = area.r - area.l;
        let right = width - if self.model.rows().len() > self.model.visible_rows() { 8 } else { 0 };
        let x = if zone == Zone::Delete { right - 15 } else { 60 };
        Point { x: area.l + x, y: area.t + slot * ROW_HEIGHT + ROW_HEIGHT / 2 }
    }

    fn click_at(&mut self, p: Point) {
        self.model.press(p);
        let action = self.model.release(p);
        self.record(action);
    }

    fn click_row(&mut self, slot: i32, zone: Zone) {
        let p = self.row_point(slot, zone);
        self.click_at(p);
    }

    /// Щелчок по кнопке с надписью `caption` (как `find_button(...).on_click()`).
    fn click_button(&mut self, caption: &str) {
        let rect = self
            .scene()
            .into_iter()
            .find_map(|d| match d.op {
                Op::Button { r, caption: c, .. } if c == caption => Some(r),
                _ => None,
            })
            .unwrap_or_else(|| panic!("нет кнопки {caption:?}"));
        self.click_at(Point { x: (rect.l + rect.r) / 2, y: (rect.t + rect.b) / 2 });
    }

    fn watch(&mut self) {
        let action = self.model.watch(self.now, Focus::Other, self.buttons, self.cursor);
        self.record(action);
    }
}

// ------------------------------------------------------------ PanelTests (11)

#[test]
fn test_empty() {
    let mut f = Fixture::new();
    f.open(vec![], state());
    let texts = f.widget_texts();
    for text in ["Закладки · 0", "Ctrl+Alt+Z", "Выделите текст и нажмите Ctrl+Alt+Z"] {
        assert!(texts.iter().any(|t| t == text), "{text}: {texts:?}");
    }
    assert!(f.model.rows_rect().is_none());
    assert!(!texts.iter().any(|t| t == "Удалить все…"));
    assert!(!texts.iter().any(|t| t.starts_with("Вернуть")));
    for item in ["Метки: показаны", "Подсказки: вкл", "Клавиши: Ctrl+Alt+Z ›", "Цвет ›", "Справка", "Выход"] {
        assert!(texts.iter().any(|t| t == item), "{item}");
    }
    let g = f.model.geometry().unwrap();
    assert!(OFF_SCREEN.l <= g.l && g.r <= OFF_SCREEN.r && OFF_SCREEN.t <= g.t && g.b <= OFF_SCREEN.b, "{g:?}");
    f.model.close();
    assert!(!f.model.is_open());
    assert!(f.calls.is_empty());
}

#[test]
fn test_three_rows_with_statuses() {
    let mut f = Fixture::new();
    let mut rows = make_rows(3);
    rows[1].status = Status::Closed;
    rows[2].status = Status::NotFound;
    rows[2].text = "короткий".into();
    f.open(rows, PanelState { undo: Some("удалённая закладка".into()), marks_visible: false, hints: false, ..state() });
    let texts = f.widget_texts();
    assert!(texts.iter().any(|t| t == "Закладки · 3"));
    assert_eq!(f.canvas_texts(Tag::Name), ["Claude", "ChatGPT", "Блокнот"]);
    assert_eq!(f.canvas_texts(Tag::Delete).len(), 3);
    let mut tags = f.canvas_texts(Tag::StatusTag);
    tags.sort();
    assert_eq!(tags, ["закрыто", "текст не найден"]);
    let shown = f.canvas_texts(Tag::Text);
    assert!(shown[0].starts_with("Закладка 0:") && shown[0].ends_with('…'), "{:?}", shown[0]);
    assert_eq!(shown[2], "короткий");
    assert!(texts.iter().any(|t| t == "Вернуть: «удалённая закладка»"), "{texts:?}");
    for item in ["Удалить все…", "Метки: скрыты", "Подсказки: выкл", "Клавиши: Ctrl+Alt+Z ›"] {
        assert!(texts.iter().any(|t| t == item), "{item}");
    }
    assert_eq!(f.count(Tag::Scrollbar), 0);
}

#[test]
fn test_fifteen_rows_scroll_and_update() {
    let mut f = Fixture::new();
    let mut rows = make_rows(15);
    f.open(rows.clone(), state());
    assert_eq!(f.model.visible_rows(), 10);
    assert_eq!(f.canvas_texts(Tag::Name).len(), 10);
    assert_eq!(f.count(Tag::Scrollbar), 1);
    f.model.wheel(-120, None);
    assert_eq!(f.model.top_row(), 3);
    for _ in 0..4 {
        f.model.wheel(-30, None); // мелкие шаги тачпада складываются
    }
    assert_eq!(f.model.top_row(), 5);
    f.model.wheel(-1200, None);
    assert_eq!(f.model.top_row(), 5); // предел: 15 строк, видно 10
    f.model.wheel(240, None);
    assert_eq!(f.model.top_row(), 0);
    f.model.wheel(-120, None);
    rows[4].status = Status::Closed;
    f.model.update(rows.clone(), state(), &f.fonts);
    assert_eq!(f.model.top_row(), 3);
    assert_eq!(f.canvas_texts(Tag::StatusTag), ["закрыто"]);
    let moved = f.model.update(rows, PanelState { undo: Some("текст".into()), ..state() }, &f.fonts);
    assert!(moved); // новый пункт подвала: окно выше, прокрутка сохранена
    assert_eq!(f.model.top_row(), 3);
    assert!(f.widget_texts().iter().any(|t| t == "Вернуть: «текст»"));
    assert_eq!(f.canvas_texts(Tag::Name).len(), 10);
}

#[test]
fn test_keyboard() {
    let mut f = Fixture::new();
    f.open(make_rows(15), state());
    for _ in 0..12 {
        f.model.key(Key::Down);
    }
    assert_eq!(f.model.hover(), Some((11, Zone::Row)));
    assert_eq!(f.model.top_row(), 2);
    let action = f.model.key(Key::Enter);
    f.record(action);
    assert_eq!(f.calls, [PanelAction::Go("id11".into())]);
    assert!(!f.model.is_open());
}

#[test]
fn test_click_row_goes_and_cross_deletes() {
    let mut f = Fixture::new();
    let mut rows = make_rows(3);
    rows[1].status = Status::Closed;
    f.open(rows.clone(), state());
    f.click_row(1, Zone::Row);
    assert_eq!(f.calls, [PanelAction::Go("id1".into())]); // «закрытая» строка тоже ведёт: программа объяснит
    assert!(!f.model.is_open());
    f.open(rows, state());
    f.click_row(2, Zone::Delete);
    assert_eq!(f.calls.last(), Some(&PanelAction::Delete("id2".into())));
    assert!(!f.model.is_open());
}

#[test]
fn test_press_and_release_on_different_rows_do_nothing() {
    let mut f = Fixture::new();
    f.open(make_rows(3), state());
    let (a, b) = (f.row_point(0, Zone::Row), f.row_point(1, Zone::Row));
    f.model.press(a);
    let action = f.model.release(b);
    f.record(action);
    assert!(f.calls.is_empty());
    assert!(f.model.is_open());
}

#[test]
fn test_row_tooltips() {
    let mut f = Fixture::new();
    let mut rows = make_rows(2);
    rows[1].status = Status::Closed;
    f.open(rows, state());
    f.model.set_hover(Some((0, Zone::Row)));
    assert_eq!(f.model.row_tip_text(), "Перейти к закладке");
    f.model.set_hover(Some((0, Zone::Delete)));
    assert_eq!(f.model.row_tip_text(), "Удалить");
    f.model.set_hover(Some((1, Zone::Row)));
    assert_eq!(f.model.row_tip_text(), CLOSED_HINT);
}

#[test]
fn test_delete_all_needs_second_click() {
    let mut f = Fixture::new();
    f.open(make_rows(3), state());
    f.click_button("Удалить все…");
    assert!(f.widget_texts().iter().any(|t| t == "Точно удалить все?"));
    assert!(f.model.confirm_all());
    assert!(f.model.is_open());
    assert!(f.calls.is_empty());
    f.click_button("Точно удалить все?");
    assert_eq!(f.calls, [PanelAction::DeleteAll]);
    assert!(!f.model.is_open());
}

#[test]
fn test_footer_actions() {
    let mut f = Fixture::new();
    let undo = || PanelState { undo: Some("x".into()), ..state() };
    for (caption, action) in [
        ("Метки: показаны", PanelAction::ToggleMarks),
        ("Подсказки: вкл", PanelAction::ToggleHints),
        ("Клавиши: Ctrl+Alt+Z ›", PanelAction::Hotkeys),
        ("Цвет ›", PanelAction::Colors),
        ("Справка", PanelAction::Help),
        ("Выход", PanelAction::Exit),
    ] {
        f.open(make_rows(1), undo());
        f.click_button(caption);
        assert_eq!(f.calls.last(), Some(&action));
        assert!(!f.model.is_open());
    }
    f.open(make_rows(1), undo());
    f.click_button("Вернуть: «x»");
    assert_eq!(f.calls.last(), Some(&PanelAction::Undo));
}

#[test]
fn test_escape_outside_click_and_toggle() {
    let mut f = Fixture::new();
    f.open(make_rows(2), state());
    let action = f.model.key(Key::Escape);
    f.record(action);
    assert_eq!(f.calls, [PanelAction::Closed]);
    assert!(!f.model.is_open());
    f.open(make_rows(2), state());
    f.cursor = f.anchor; // значок трея — вне панели
    f.buttons[0] = true;
    f.watch();
    assert_eq!(f.calls.last(), Some(&PanelAction::Closed));
    assert!(!f.model.is_open());
    assert!(!f.show(make_rows(2), state())); // этот щелчок её и закрыл
    f.buttons = [false; 3];
    f.open(make_rows(2), state());
    assert!(!f.show(make_rows(2), state())); // второй вызов закрывает
    assert_eq!(f.calls.iter().filter(|a| **a == PanelAction::Closed).count(), 3);
}

#[test]
fn test_click_inside_keeps_open() {
    let mut f = Fixture::new();
    f.open(make_rows(2), state());
    let g = f.model.geometry().unwrap();
    f.cursor = Point { x: g.l + 5, y: g.t + 5 };
    f.buttons[0] = true;
    f.watch();
    assert!(f.model.is_open());
}

// ------------------------------------------------------------ сверх Python-тестов

#[test]
fn reopen_rule_only_near_and_soon() {
    let mut f = Fixture::new();
    f.open(make_rows(2), state());
    f.cursor = f.anchor;
    f.buttons[0] = true;
    f.watch();
    f.buttons = [false; 3];
    f.anchor = Point { x: ANCHOR.x + 41, y: ANCHOR.y }; // дальше 40 px — открывается
    assert!(f.show(make_rows(2), state()));
    f.model.close();
    f.open(make_rows(2), state());
    f.anchor = ANCHOR;
    f.cursor = ANCHOR;
    f.buttons[0] = true;
    f.watch();
    f.buttons = [false; 3];
    f.now += 0.61; // позже 0,6 с — открывается
    assert!(f.show(make_rows(2), state()));
}

#[test]
fn focus_loss_closes_only_after_focus() {
    let mut f = Fixture::new();
    f.open(make_rows(2), state());
    let other = f.model.watch(f.now, Focus::Other, [false; 3], f.cursor);
    assert_eq!(other, None); // фокуса ещё не было — не закрывать
    assert_eq!(f.model.watch(f.now, Focus::Panel, [false; 3], f.cursor), None);
    assert_eq!(f.model.watch(f.now, Focus::Other, [false; 3], f.cursor), Some(PanelAction::Closed));
}

#[test]
fn small_screen_shows_fewer_rows_and_taskbar_on_top() {
    let mut f = Fixture::new();
    f.work = Rect { l: -32000, t: -32000, r: -30000, b: -31700 }; // 300 px высоты
    f.anchor = Point { x: -30100, y: -31990 }; // у верхнего края: панель задач сверху
    f.open(make_rows(15), state());
    let rows = f.model.visible_rows();
    assert!((3..10).contains(&rows), "{rows}");
    let g = f.model.geometry().unwrap();
    assert!(g.t >= f.work.t + 8 && g.b <= f.work.b + 8, "{g:?}");
    assert!(g.t > f.anchor.y, "под якорем: {g:?}");
    // совсем низкий экран — не меньше трёх строк
    f.model.close();
    f.work.b = f.work.t + 120;
    f.open(make_rows(15), state());
    assert_eq!(f.model.visible_rows(), 3);
}

#[test]
fn mouse_hover_tracks_rows_and_items() {
    let mut f = Fixture::new();
    f.open(make_rows(3), state());
    let p = f.row_point(1, Zone::Delete);
    f.model.mouse_move(p);
    assert_eq!(f.model.hover(), Some((1, Zone::Delete)));
    assert_eq!(f.model.tip_text(f.model.pointer_target().unwrap()), "Удалить");
    assert!(f.scene().iter().any(|d| d.tag == Tag::Hover));
    let (_, exit) = f.model.items().into_iter().find(|(i, _)| *i == Item::Exit).unwrap();
    f.model.mouse_move(Point { x: exit.l + 3, y: exit.t + 3 });
    assert_eq!(f.model.hover(), None);
    assert_eq!(f.model.item_hover(), Some(Item::Exit));
    assert_eq!(f.model.tip_text(f.model.pointer_target().unwrap()), "Закрыть программу; закладки сохранятся");
    f.model.mouse_leave();
    assert_eq!(f.model.pointer_target(), None);
}

// ------------------------------------------------------------ окно панели (скрытое)

struct FakeEnv {
    buttons: Cell<[bool; 3]>,
    cursor: Cell<Point>,
    reveals: Cell<u32>,
}

impl FakeEnv {
    fn new() -> Arc<FakeEnv> {
        Arc::new(FakeEnv { buttons: Cell::new([false; 3]), cursor: Cell::new(ANCHOR), reveals: Cell::new(0) })
    }
}

impl PanelEnv for FakeEnv {
    fn work_area(&self, _: Point) -> Rect {
        OFF_SCREEN
    }
    fn scale_at(&self, _: Point) -> f64 {
        1.0
    }
    fn cursor(&self) -> Point {
        self.cursor.get()
    }
    fn mouse_buttons(&self) -> [bool; 3] {
        self.buttons.get()
    }
    fn foreground(&self) -> Hwnd {
        0
    }
    fn light(&self) -> bool {
        true
    }
    fn reveal(&self, _: Hwnd, _: Rect) {
        self.reveals.set(self.reveals.get() + 1); // окно не показывать, фокус не брать
    }
}

fn recording_panel(env: Arc<FakeEnv>) -> (Panel, Rc<RefCell<Vec<PanelAction>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let sink = calls.clone();
    (Panel::with_env(env, Box::new(move |a| sink.borrow_mut().push(a))), calls)
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

fn gui_objects() -> (u32, u32) {
    // SAFETY: только чтение счётчиков своего процесса.
    unsafe { (GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS)) }
}

/// Готово, когда: построение и отрисовка 15 строк в Canvas + создание окна скрытым < 30 мс.
#[test]
fn open_fifteen_rows_under_30_ms() {
    let mut cold = None;
    let mut best = f64::MAX;
    for run in 0..5 {
        let started = Instant::now();
        // 1) модель + вид + отрисовка в холст (в памяти)
        let fonts = PanelFonts::new(1.0);
        let mut model = PanelModel::new();
        let ctx = ShowContext {
            now: 0.0,
            anchor: ANCHOR,
            work_area: OFF_SCREEN,
            scale: 1.0,
            light: true,
            rounded: true,
            buttons: [false; 3],
        };
        assert!(model.show(&ctx, make_rows(15), state(), &fonts).0);
        let g = model.geometry().unwrap();
        let mut canvas = Canvas::new(g.r - g.l, g.b - g.t);
        paint(&mut canvas, &model.scene(&fonts), &fonts);
        let model_ms = started.elapsed().as_secs_f64() * 1000.0;
        // 2) настоящая панель: шрифты, модель, скрытое окно, задний буфер
        let env = FakeEnv::new();
        let (mut panel, _) = recording_panel(env.clone());
        let window_started = Instant::now();
        assert!(panel.show(ANCHOR, make_rows(15), state()));
        let window_ms = window_started.elapsed().as_secs_f64() * 1000.0;
        let hwnd = panel.hwnd();
        // SAFETY: проверка своего окна.
        unsafe {
            assert!(IsWindow(to_hwnd(hwnd)).as_bool());
            assert!(!IsWindowVisible(to_hwnd(hwnd)).as_bool());
        }
        assert_eq!(env.reveals.get(), 1);
        panel.hide();
        let total = model_ms + window_ms;
        eprintln!("run {run}: canvas {model_ms:.2} ms + hidden window {window_ms:.2} ms = {total:.2} ms");
        cold.get_or_insert(total);
        best = best.min(total);
    }
    // Первый раз в процессе — загрузка шрифтов GDI и регистрация класса; при параллельных тестах
    // (все создают шрифты одновременно) он нестабилен, поэтому проверяется лучший прогон.
    // Один этот тест: `cargo test --test panel open_fifteen -- --nocapture`.
    eprintln!("cold {:.2} ms, best {best:.2} ms", cold.unwrap_or(0.0));
    assert!(best < 30.0, "best {best:.2} ms");
}

#[test]
fn hidden_window_clicks_keys_and_watch() {
    let env = FakeEnv::new();
    let (mut panel, calls) = recording_panel(env.clone());
    let rows = make_rows(3);
    // щелчок по строке 1 — переход; окно уничтожено до вызова действия
    assert!(panel.show(ANCHOR, rows.clone(), state()));
    let hwnd = panel.hwnd();
    let (fonts, mut model) = (PanelFonts::new(1.0), PanelModel::new());
    let ctx = ShowContext { now: 0.0, anchor: ANCHOR, work_area: OFF_SCREEN, scale: 1.0, light: true, rounded: true, buttons: [false; 3] };
    model.show(&ctx, rows.clone(), state(), &fonts);
    let area = model.rows_rect().unwrap();
    let p = Point { x: area.l + 60, y: area.t + ROW_HEIGHT + ROW_HEIGHT / 2 };
    send(hwnd, WM_MOUSEMOVE, 0, lparam(p));
    send(hwnd, WM_LBUTTONDOWN, 1, lparam(p));
    send(hwnd, WM_LBUTTONUP, 0, lparam(p));
    assert_eq!(*calls.borrow(), [PanelAction::Go("id1".into())]);
    assert!(!panel.is_open());
    // SAFETY: проверка дескриптора.
    assert!(!unsafe { IsWindow(to_hwnd(hwnd)).as_bool() });
    // Esc
    assert!(panel.show(ANCHOR, rows.clone(), state()));
    send(panel.hwnd(), WM_KEYDOWN, 0x1B, LPARAM(0));
    assert_eq!(calls.borrow().last(), Some(&PanelAction::Closed));
    assert!(!panel.is_open());
    // опрос: нажатие вне панели закрывает, тот же щелчок не открывает снова
    assert!(panel.show(ANCHOR, rows.clone(), state()));
    env.buttons.set([true, false, false]);
    send(panel.hwnd(), WM_TIMER, 1, LPARAM(0));
    assert!(!panel.is_open());
    assert_eq!(calls.borrow().iter().filter(|a| **a == PanelAction::Closed).count(), 2);
    env.buttons.set([false; 3]);
    assert!(!panel.show(ANCHOR, rows.clone(), state()));
    // повторный show закрывает (запись о закрывшем щелчке уже израсходована)
    assert!(panel.show(ANCHOR, rows.clone(), state()));
    assert!(!panel.show(ANCHOR, rows.clone(), state()));
    assert_eq!(calls.borrow().iter().filter(|a| **a == PanelAction::Closed).count(), 3);
    // update закрытой панели только запоминает данные
    panel.update(make_rows(1), state());
    assert!(!panel.is_open());
    // update открытой: новое число строк — окно меняет размер
    assert!(panel.show(ANCHOR, rows, state()));
    panel.update(make_rows(12), state());
    assert!(panel.is_open());
    drop(panel); // Drop закрывает окно
}

#[test]
fn no_gdi_or_window_leaks() {
    let env = FakeEnv::new();
    let (mut panel, _) = recording_panel(env);
    // прогрев: класс, шрифты системы, окно пояснений
    for _ in 0..3 {
        assert!(panel.show(ANCHOR, make_rows(15), state()));
        panel.hide();
    }
    let before = gui_objects();
    for i in 0..40 {
        assert!(panel.show(ANCHOR, make_rows(15), state()));
        if i % 2 == 0 {
            panel.update(make_rows(5), PanelState { undo: Some("x".into()), ..state() });
        }
        panel.hide();
        let mut c = Canvas::new(50, 50);
        c.fill(Rect { l: 0, t: 0, r: 50, b: 50 }, Rgb(1, 2, 3));
        drop(PanelFonts::new(1.25));
    }
    let after = gui_objects();
    eprintln!("GDI {} -> {}, USER {} -> {}", before.0, after.0, before.1, after.1);
    // другие тесты идут параллельно и тоже создают объекты; утечка дала бы сотни
    assert!(after.0 <= before.0 + 30, "GDI {} -> {}", before.0, after.0);
    assert!(after.1 <= before.1 + 10, "USER {} -> {}", before.1, after.1);
}

/// Картинки панели для сверки вида со снимком Python-панели:
/// `.\build.ps1 test --test panel -- --ignored --nocapture` → %TEMP%\zakladka-part7\panel-*.bmp
#[test]
#[ignore]
fn panel_bmp() {
    let dir = std::env::temp_dir().join("zakladka-part7");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, light, scale, count, rounded) in [
        ("light-3", true, 1.0, 3, true),
        ("dark-3", false, 1.0, 3, true),
        ("light-15", true, 1.0, 15, true),
        ("light-empty", true, 1.0, 0, false),
        ("light-3-150", true, 1.5, 3, true),
    ] {
        let fonts = PanelFonts::new(scale);
        let mut model = PanelModel::new();
        let mut rows = make_rows(count);
        if count >= 3 {
            rows[1].status = Status::Closed;
            rows[2].status = Status::NotFound;
            rows[2].text = "короткий".into();
        }
        let ctx = ShowContext { now: 0.0, anchor: ANCHOR, work_area: OFF_SCREEN, scale, light, rounded, buttons: [false; 3] };
        let undo = if count > 0 { Some("удалённая закладка".to_string()) } else { None };
        assert!(model.show(&ctx, rows, PanelState { undo, ..state() }, &fonts).0);
        if count > 0 {
            let area = model.rows_rect().unwrap();
            let y = area.t + (ROW_HEIGHT as f64 * scale) as i32 / 2;
            model.mouse_move(Point { x: area.r - (20.0 * scale) as i32, y }); // наведение на ✕ первой строки
        }
        let g = model.geometry().unwrap();
        let mut canvas = Canvas::new(g.r - g.l, g.b - g.t);
        paint(&mut canvas, &model.scene(&fonts), &fonts);
        let path = dir.join(format!("panel-{name}.bmp"));
        canvas.save_bmp(&path).unwrap();
        println!("{} ({}x{})", path.display(), g.r - g.l, g.b - g.t);
    }
}
