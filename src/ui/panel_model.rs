//! Модель панели без окон (часть 7; N 632–1128): строки, прокрутка, наведение, клавиши, раскладка.
//! `PanelModel` — на усмотрение части 7 (в разделе 5 его нет).
//!
//! Модель не создаёт окон и не читает экран: всё внешнее (рабочая область, масштаб, тема, время,
//! кнопки мыши, активное окно) ей передают. Вид — список команд рисования `scene()`, его рисует
//! `panel::paint` в `Canvas`. Координаты — пиксели окна панели (0, 0 — левый верхний угол).

use crate::core::text::CLOSED_HINT;
use crate::types::{Point, Rect, Rgb, Status};
use crate::ui::gfx::{faint, fit_text, mix, palette, px, tag_fill, Align};

#[derive(Clone, PartialEq, Debug)]
pub struct PanelRow {
    pub id: String,
    pub program: String,
    pub text: String,
    pub status: Status,
    pub color: Rgb,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PanelState {
    pub hotkey: String,
    pub extra_hotkey: String,
    pub undo: Option<String>,
    pub marks_visible: bool,
    pub hints: bool,
    pub accent: Rgb,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PanelAction {
    Go(String),
    Delete(String),
    Undo,
    DeleteAll,
    ToggleMarks,
    ToggleHints,
    Hotkeys,
    Colors,
    Help,
    Exit,
    Closed,
}

/// Размеры при 96 DPI (N 640–644).
pub const WIDTH: i32 = 380;
pub const ROW_HEIGHT: i32 = 32;
pub const ITEM_HEIGHT: i32 = 30;
pub const MAX_ROWS: usize = 10;
/// Колёсико: деление (delta 120) прокручивает три строки.
pub const WHEEL_PER_ROW: i32 = 40;
/// Опрос активного окна и кнопок мыши, пока панель открыта (N 1108–1127).
pub const WATCH_MS: u32 = 50;
/// Сколько ждёт второй щелчок «Удалить все» (N 1051).
pub const CONFIRM_MS: u32 = 4000;
/// Щелчок по трею, который только что закрыл панель, не открывает её снова (N 676–681).
pub const REOPEN_GUARD_S: f64 = 0.6;
pub const REOPEN_GUARD_PX: f64 = 40.0;
/// Светло-красный ✕ строки и он же при наведении (N 28).
pub const DELETE_COLOR: Rgb = Rgb(0xF2, 0x8B, 0x82);
pub const DELETE_HOVER: Rgb = Rgb(0xE8, 0x45, 0x3C);

/// Шрифты панели (N 742–747).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextStyle {
    Title,
    Name,
    Text,
    Small,
    Tag,
    Symbol,
}

impl TextStyle {
    pub const ALL: [TextStyle; 6] =
        [TextStyle::Title, TextStyle::Name, TextStyle::Text, TextStyle::Small, TextStyle::Tag, TextStyle::Symbol];

    /// (пиксели при 96 DPI, полужирный, семейство).
    pub fn spec(self) -> (f64, bool, &'static str) {
        match self {
            TextStyle::Title => (14.0, true, "Segoe UI"),
            TextStyle::Name => (13.0, true, "Segoe UI"),
            TextStyle::Text => (13.0, false, "Segoe UI"),
            TextStyle::Small => (12.0, false, "Segoe UI"),
            TextStyle::Tag => (11.0, false, "Segoe UI"),
            TextStyle::Symbol => (12.0, false, "Segoe UI Symbol"),
        }
    }
}

/// Измерение текста шрифтами панели текущего масштаба (`panel::PanelFonts` — GDI).
pub trait Measure {
    fn width(&self, style: TextStyle, s: &str) -> i32;
    /// Высота строки.
    fn height(&self, style: TextStyle) -> i32;
    /// (ширина, высота) с переносом по словам при ширине `width`.
    fn wrapped(&self, style: TextStyle, s: &str, width: i32) -> (i32, i32);
}

/// Часть строки: сама строка (переход) или ✕ (удаление).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Row,
    Delete,
}

/// Пункты подвала.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Undo,
    DeleteAll,
    Marks,
    Hints,
    Hotkeys,
    Colors,
    Help,
    Exit,
}

const GRID: [Item; 6] = [Item::Marks, Item::Hints, Item::Hotkeys, Item::Colors, Item::Help, Item::Exit];

/// Что под точкой окна.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Row(usize, Zone),
    Item(Item),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Delete,
    Escape,
}

/// Активное окно при опросе: нет, панель, другое.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    None,
    Panel,
    Other,
}

/// Всё внешнее, что нужно для открытия.
#[derive(Clone, Copy, Debug)]
pub struct ShowContext {
    /// Секунды монотонных часов (для правила «0,6 с»).
    pub now: f64,
    /// Точка у значка трея.
    pub anchor: Point,
    /// Рабочая область монитора якоря.
    pub work_area: Rect,
    /// Масштаб DPI монитора якоря, 1…4.
    pub scale: f64,
    pub light: bool,
    /// Windows 11 рисует рамку и углы сама; иначе панель рисует рамку 1 px.
    pub rounded: bool,
    /// Кнопки мыши (левая, правая, средняя) в момент открытия.
    pub buttons: [bool; 3],
}

/// Команда рисования.
#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    Fill { r: Rect, c: Rgb },
    Round { r: Rect, radius: i32, fill: Rgb, outline: Option<Rgb> },
    Polygon { points: Vec<Point>, fill: Rgb },
    Text { r: Rect, s: String, style: TextStyle, c: Rgb, align: Align, wrap: bool },
    Button { r: Rect, caption: String, hover: bool, pressed: bool, fg: Rgb, hover_fill: Rgb },
}

/// Метка команды (как теги холста Tk в N 892–953 и тексты виджетов в тестах).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Background,
    Icon,
    Title,
    Hotkey,
    Empty,
    Hover,
    Dot,
    StatusTag,
    Name,
    Text,
    Delete,
    Scrollbar,
    Line,
    Item(Item),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Draw {
    pub tag: Tag,
    pub op: Op,
}

#[derive(Clone, PartialEq, Debug)]
enum Pressed {
    Row(String, Zone),
    Item(Item),
}

#[derive(Clone, Debug, Default)]
struct Layout {
    width: i32,
    height: i32,
    border: i32,
    icon: Rect,
    title: Rect,
    hotkey: Rect,
    rows: Option<Rect>,
    empty: Option<Rect>,
    line: Rect,
    items: Vec<(Item, Rect)>,
}

/// Состояние панели (Python `Panel` без окна).
pub struct PanelModel {
    open: bool,
    rows: Vec<PanelRow>,
    state: PanelState,
    anchor: Point,
    work_area: Rect,
    scale: f64,
    light: bool,
    rounded: bool,
    top_row: usize,
    visible_rows: usize,
    hover: Option<(usize, Zone)>,
    item_hover: Option<Item>,
    in_rows: bool,
    pressed: Option<Pressed>,
    confirm_all: bool,
    wheel: i32,
    had_focus: bool,
    buttons: [bool; 3],
    dismissed: Option<(f64, Point)>,
    layout: Layout,
    layout_key: (usize, bool),
    geometry: Option<Rect>,
}

impl Default for PanelModel {
    fn default() -> Self {
        PanelModel::new()
    }
}

impl PanelModel {
    pub fn new() -> PanelModel {
        PanelModel {
            open: false,
            rows: Vec::new(),
            state: PanelState {
                hotkey: String::new(),
                extra_hotkey: String::new(),
                undo: None,
                marks_visible: true,
                hints: true,
                accent: Rgb(0xFF, 0x7A, 0x00),
            },
            anchor: Point::default(),
            work_area: Rect::default(),
            scale: 1.0,
            light: true,
            rounded: true,
            top_row: 0,
            visible_rows: 0,
            hover: None,
            item_hover: None,
            in_rows: false,
            pressed: None,
            confirm_all: false,
            wheel: 0,
            had_focus: false,
            buttons: [false; 3],
            dismissed: None,
            layout: Layout::default(),
            layout_key: (0, false),
            geometry: None,
        }
    }

    // ---- состояние

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn rows(&self) -> &[PanelRow] {
        &self.rows
    }

    pub fn state(&self) -> &PanelState {
        &self.state
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn light(&self) -> bool {
        self.light
    }

    pub fn top_row(&self) -> usize {
        self.top_row
    }

    pub fn visible_rows(&self) -> usize {
        self.visible_rows
    }

    /// Выделенная строка (мышью или клавишами).
    pub fn hover(&self) -> Option<(usize, Zone)> {
        self.hover
    }

    pub fn set_hover(&mut self, hover: Option<(usize, Zone)>) {
        self.hover = hover;
    }

    pub fn item_hover(&self) -> Option<Item> {
        self.item_hover
    }

    /// Ждёт второго щелчка «Удалить все».
    pub fn confirm_all(&self) -> bool {
        self.confirm_all
    }

    /// Прямоугольник окна на экране.
    pub fn geometry(&self) -> Option<Rect> {
        if self.open {
            self.geometry
        } else {
            None
        }
    }

    /// Область строк в координатах окна (нет закладок — `None`).
    pub fn rows_rect(&self) -> Option<Rect> {
        if self.open {
            self.layout.rows
        } else {
            None
        }
    }

    /// Пункты подвала по порядку и их прямоугольники в координатах окна.
    pub fn items(&self) -> Vec<(Item, Rect)> {
        if self.open {
            self.layout.items.clone()
        } else {
            Vec::new()
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        self.geometry().is_some_and(|g| inside(&g, p))
    }

    // ---- открытие, обновление, закрытие

    /// Открыть у `ctx.anchor` (N 670–697). Возвращает (открыта, действие): если панель уже была
    /// открыта — закрывает её и отдаёт `Closed` (второй щелчок по трею); щелчок, который только
    /// что закрыл панель (0,6 с и 40 px), — (false, None).
    pub fn show(
        &mut self,
        ctx: &ShowContext,
        rows: Vec<PanelRow>,
        state: PanelState,
        m: &dyn Measure,
    ) -> (bool, Option<PanelAction>) {
        if self.open {
            return (false, self.dismiss(None));
        }
        if let Some((when, point)) = self.dismissed.take() {
            let distance = (point.x - ctx.anchor.x).abs().max((point.y - ctx.anchor.y).abs());
            if distance <= self.s(REOPEN_GUARD_PX) && ctx.now - when < REOPEN_GUARD_S {
                return (false, None);
            }
        }
        self.anchor = ctx.anchor;
        self.work_area = ctx.work_area;
        self.scale = ctx.scale;
        self.light = ctx.light;
        self.rounded = ctx.rounded;
        self.rows = rows;
        self.state = state;
        self.top_row = 0;
        self.confirm_all = false;
        self.wheel = 0;
        self.build(m);
        self.place(m);
        self.open = true;
        self.had_focus = false;
        self.buttons = ctx.buttons;
        (true, None)
    }

    /// Обновить строки и состояние, сохранив прокрутку (N 699–709). `true` — у окна новый
    /// прямоугольник (изменилось число строк или появилось «Вернуть»).
    pub fn update(&mut self, rows: Vec<PanelRow>, state: PanelState, m: &dyn Measure) -> bool {
        self.rows = rows;
        self.state = state;
        if !self.open {
            return false;
        }
        if self.current_key() != self.layout_key {
            let (top, confirm, before) = (self.top_row, self.confirm_all, self.geometry);
            self.build(m);
            self.top_row = top;
            self.confirm_all = confirm;
            self.place(m);
            self.clamp_scroll();
            return self.geometry != before;
        }
        self.clamp_scroll();
        false
    }

    /// Закрыть без действия (`hide`, N 711–713, 1076–1084).
    pub fn close(&mut self) {
        self.open = false;
        self.confirm_all = false;
        self.hover = None;
        self.pressed = None;
        self.item_hover = None;
        self.in_rows = false;
    }

    /// Закрыть по Esc, щелчку вне панели или потере фокуса и сообщить `Closed` (N 1065–1074).
    /// `at` = (время, точка щелчка) — для правила «0,6 с и 40 px».
    pub fn dismiss(&mut self, at: Option<(f64, Point)>) -> Option<PanelAction> {
        if !self.open {
            return None;
        }
        self.close();
        if at.is_some() {
            self.dismissed = at;
        }
        Some(PanelAction::Closed)
    }

    /// Конец ожидания второго щелчка «Удалить все» (через `CONFIRM_MS`).
    pub fn end_confirm(&mut self) {
        self.confirm_all = false;
    }

    // ---- мышь

    /// Что под точкой окна (N 955–966 для строк).
    pub fn hit(&self, p: Point) -> Option<Target> {
        if !self.open {
            return None;
        }
        if let Some((index, zone)) = self.row_hit(p) {
            return Some(Target::Row(index, zone));
        }
        self.item_at(p).map(Target::Item)
    }

    /// Движение мыши: наведение строки (пока указатель в области строк) и пункта подвала.
    pub fn mouse_move(&mut self, p: Point) {
        if !self.open {
            return;
        }
        if self.layout.rows.is_some_and(|r| inside(&r, p)) {
            self.in_rows = true;
            self.hover = self.row_hit(p);
        } else if self.in_rows {
            self.in_rows = false;
            self.hover = None;
        }
        self.item_hover = self.item_at(p);
    }

    /// Указатель ушёл из окна.
    pub fn mouse_leave(&mut self) {
        if self.in_rows {
            self.hover = None;
        }
        self.in_rows = false;
        self.item_hover = None;
    }

    /// Часть под указателем мыши — для пояснения.
    pub fn pointer_target(&self) -> Option<Target> {
        if self.in_rows {
            self.hover.map(|(index, zone)| Target::Row(index, zone))
        } else {
            self.item_hover.map(Target::Item)
        }
    }

    pub fn press(&mut self, p: Point) {
        self.pressed = match self.hit(p) {
            Some(Target::Row(index, zone)) => Some(Pressed::Row(self.rows[index].id.clone(), zone)),
            Some(Target::Item(item)) => Some(Pressed::Item(item)),
            None => None,
        };
    }

    /// Отпускание: действие, только если нажатие и отпускание пришлись на одну и ту же строку
    /// (по id) и её часть или на один пункт (N 994–1002, 577–587).
    pub fn release(&mut self, p: Point) -> Option<PanelAction> {
        let pressed = self.pressed.take()?;
        match (pressed, self.hit(p)?) {
            (Pressed::Row(id, zone), Target::Row(index, at)) if at == zone && self.rows[index].id == id => {
                self.act(if zone == Zone::Delete { PanelAction::Delete(id) } else { PanelAction::Go(id) })
            }
            (Pressed::Item(a), Target::Item(b)) if a == b => self.click(a),
            _ => None,
        }
    }

    /// Щелчок по пункту подвала. «Удалить все» — только вторым щелчком (N 1045–1051).
    pub fn click(&mut self, item: Item) -> Option<PanelAction> {
        if !self.open {
            return None;
        }
        let action = match item {
            Item::Undo => PanelAction::Undo,
            Item::DeleteAll => {
                if !self.confirm_all {
                    self.confirm_all = true;
                    return None;
                }
                PanelAction::DeleteAll
            }
            Item::Marks => PanelAction::ToggleMarks,
            Item::Hints => PanelAction::ToggleHints,
            Item::Hotkeys => PanelAction::Hotkeys,
            Item::Colors => PanelAction::Colors,
            Item::Help => PanelAction::Help,
            Item::Exit => PanelAction::Exit,
        };
        self.act(action)
    }

    /// Колёсико (N 1021–1029): `delta` как в WM_MOUSEWHEEL; `pointer` — указатель в координатах
    /// окна. `true` — прокрутилось.
    pub fn wheel(&mut self, delta: i32, pointer: Option<Point>) -> bool {
        if !self.open || self.visible_rows == 0 || self.rows.len() <= self.visible_rows {
            return false;
        }
        self.wheel = self.wheel.saturating_sub(delta);
        let rows = self.wheel / WHEEL_PER_ROW;
        if rows == 0 {
            return false;
        }
        self.wheel -= rows * WHEEL_PER_ROW;
        self.scroll_to(self.top_row as i64 + rows as i64, pointer)
    }

    // ---- клавиши

    pub fn key(&mut self, key: Key) -> Option<PanelAction> {
        if !self.open {
            return None;
        }
        let page = self.visible_rows.max(1) as i32;
        match key {
            Key::Up => self.move_hover(-1),
            Key::Down => self.move_hover(1),
            Key::PageUp => self.move_hover(-page),
            Key::PageDown => self.move_hover(page),
            Key::Enter => return self.keyboard_action(false),
            Key::Delete => return self.keyboard_action(true),
            Key::Escape => return self.dismiss(None),
        }
        None
    }

    /// Стрелки и страницы (N 1031–1042).
    pub fn move_hover(&mut self, step: i32) {
        if !self.open || self.rows.is_empty() || self.visible_rows == 0 {
            return;
        }
        let len = self.rows.len() as i64;
        let current = match self.hover {
            Some((index, _)) => index as i64,
            None if step > 0 => -1,
            None => len,
        };
        let index = (current + step as i64).clamp(0, len - 1) as usize;
        if index < self.top_row {
            self.top_row = index;
        } else if index >= self.top_row + self.visible_rows {
            self.top_row = index + 1 - self.visible_rows;
        }
        self.hover = Some((index, Zone::Row));
    }

    // ---- опрос

    /// Раз в `WATCH_MS` (N 1108–1127): закрыть при потере фокуса (если он был) или при нажатии
    /// кнопки мыши вне панели (в том числе когда фокуса не было вовсе).
    pub fn watch(&mut self, now: f64, focus: Focus, buttons: [bool; 3], cursor: Point) -> Option<PanelAction> {
        if !self.open {
            return None;
        }
        match focus {
            Focus::Panel => self.had_focus = true,
            Focus::Other if self.had_focus => return self.dismiss(Some((now, cursor))),
            _ => {}
        }
        let newly = buttons.iter().zip(self.buttons.iter()).any(|(now, before)| *now && !*before);
        if newly && !self.contains(cursor) {
            return self.dismiss(Some((now, cursor)));
        }
        self.buttons = buttons;
        None
    }

    // ---- тексты

    /// Пояснение к выделенной строке (N 968–974).
    pub fn row_tip_text(&self) -> String {
        match self.hover {
            Some((index, zone)) => self.tip_text(Target::Row(index, zone)),
            None => String::new(),
        }
    }

    /// Пояснение к части панели (N 802–826).
    pub fn tip_text(&self, target: Target) -> String {
        let text = match target {
            Target::Row(index, _) if index >= self.rows.len() => "",
            Target::Row(_, Zone::Delete) => "Удалить",
            Target::Row(index, Zone::Row) => {
                if self.rows[index].status == Status::Closed {
                    CLOSED_HINT
                } else {
                    "Перейти к закладке"
                }
            }
            Target::Item(Item::Undo) => "Вернуть последнюю удалённую закладку",
            Target::Item(Item::DeleteAll) => {
                if self.confirm_all {
                    "Щёлкните ещё раз — удалятся все закладки (их можно вернуть)"
                } else {
                    "Удалить все закладки; потом их можно вернуть"
                }
            }
            Target::Item(Item::Marks) => "Временно скрыть все отметки на экране; закладки сохраняются",
            Target::Item(Item::Hints) => "Короткие сообщения у курсора (Поставлена, Снята…)",
            Target::Item(Item::Hotkeys) => {
                return format!(
                    "Поставить: {} · Дополнительная: {}. Щёлкните, чтобы изменить",
                    self.state.hotkey, self.state.extra_hotkey
                )
            }
            Target::Item(Item::Colors) => "Цвет подсветки, полосы и надписей; пипетка",
            Target::Item(Item::Help) => "Как пользоваться закладками",
            Target::Item(Item::Exit) => "Закрыть программу; закладки сохранятся",
        };
        text.to_string()
    }

    /// Надпись пункта подвала (N 817–826, 883–889).
    pub fn caption(&self, item: Item, m: &dyn Measure) -> String {
        let text = match item {
            Item::Undo => {
                let prefix = "Вернуть: «";
                let room = self.s(WIDTH as f64) - self.s(40.0) - m.width(TextStyle::Text, "Вернуть: «»");
                let undo = one_line(self.state.undo.as_deref().unwrap_or(""));
                let fitted = fit_text(&|t| m.width(TextStyle::Text, t), &undo, room);
                return format!("{prefix}{fitted}»");
            }
            Item::DeleteAll => {
                if self.confirm_all {
                    "Точно удалить все?"
                } else {
                    "Удалить все…"
                }
            }
            Item::Marks => {
                if self.state.marks_visible {
                    "Метки: показаны"
                } else {
                    "Метки: скрыты"
                }
            }
            Item::Hints => {
                if self.state.hints {
                    "Подсказки: вкл"
                } else {
                    "Подсказки: выкл"
                }
            }
            Item::Hotkeys => return format!("Клавиши: {} ›", self.state.hotkey),
            Item::Colors => "Цвет ›",
            Item::Help => "Справка",
            Item::Exit => "Выход",
        };
        text.to_string()
    }

    /// Вид открытой панели — команды рисования по порядку (N 749–832, 892–953).
    pub fn scene(&self, m: &dyn Measure) -> Vec<Draw> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let p = palette(self.light);
        let l = &self.layout;
        let full = Rect { l: 0, t: 0, r: l.width, b: l.height };
        let push = |out: &mut Vec<Draw>, tag: Tag, op: Op| out.push(Draw { tag, op });
        if l.border > 0 {
            push(&mut out, Tag::Background, Op::Fill { r: full, c: p.border });
            let b = l.border;
            push(&mut out, Tag::Background, Op::Fill { r: Rect { l: b, t: b, r: l.width - b, b: l.height - b }, c: p.bg });
        } else {
            push(&mut out, Tag::Background, Op::Fill { r: full, c: p.bg });
        }
        let i = l.icon;
        let points = vec![
            Point { x: i.l, y: i.t },
            Point { x: i.r, y: i.t },
            Point { x: i.r, y: i.b },
            Point { x: (i.l + i.r) / 2, y: i.b - self.s(4.0) },
            Point { x: i.l, y: i.b },
        ];
        push(&mut out, Tag::Icon, Op::Polygon { points, fill: self.state.accent });
        let title = format!("Закладки · {}", self.rows.len());
        push(&mut out, Tag::Title, text_op(l.title, title, TextStyle::Title, p.fg, Align::Left));
        let hotkey = self.state.hotkey.clone();
        push(&mut out, Tag::Hotkey, text_op(l.hotkey, hotkey, TextStyle::Small, p.muted, Align::Right));
        if let Some(rows) = l.rows {
            self.draw_rows(&mut out, rows, m);
        } else if let Some(r) = l.empty {
            let text = self.empty_text();
            out.push(Draw { tag: Tag::Empty, op: Op::Text { r, s: text, style: TextStyle::Text, c: p.muted, align: Align::Left, wrap: true } });
        }
        push(&mut out, Tag::Line, Op::Fill { r: l.line, c: p.line });
        for &(item, r) in &l.items {
            let fg = if item == Item::DeleteAll && self.confirm_all { p.danger } else { p.fg };
            let hover = self.item_hover == Some(item);
            let pressed = hover && self.pressed == Some(Pressed::Item(item));
            let caption = self.caption(item, m);
            push(&mut out, Tag::Item(item), Op::Button { r, caption, hover, pressed, fg, hover_fill: p.hover });
        }
        out
    }

    // ---- внутреннее

    fn s(&self, value: f64) -> i32 {
        px(value, self.scale)
    }

    fn has_undo(&self) -> bool {
        self.state.undo.as_deref().is_some_and(|u| !u.is_empty())
    }

    /// N 739–740: число видимых строк и наличие «Вернуть» — при их смене раскладка строится заново.
    fn current_key(&self) -> (usize, bool) {
        (self.rows.len().min(MAX_ROWS), self.has_undo())
    }

    fn scrolling(&self) -> bool {
        self.rows.len() > self.visible_rows
    }

    fn empty_text(&self) -> String {
        let hotkey = if self.state.hotkey.is_empty() { "Ctrl+Alt+Z" } else { &self.state.hotkey };
        format!("Выделите текст и нажмите {hotkey}")
    }

    fn act(&mut self, action: PanelAction) -> Option<PanelAction> {
        self.close();
        Some(action)
    }

    fn keyboard_action(&mut self, delete: bool) -> Option<PanelAction> {
        let (index, _) = self.hover?;
        let id = self.rows.get(index)?.id.clone();
        self.act(if delete { PanelAction::Delete(id) } else { PanelAction::Go(id) })
    }

    fn max_top(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows)
    }

    fn clamp_scroll(&mut self) {
        self.top_row = self.top_row.min(self.max_top());
    }

    fn scroll_to(&mut self, top: i64, pointer: Option<Point>) -> bool {
        let before = self.top_row;
        self.top_row = top.clamp(0, self.max_top() as i64) as usize;
        if self.top_row == before {
            return false;
        }
        self.hover = pointer.and_then(|p| self.row_hit(p));
        true
    }

    fn row_hit(&self, p: Point) -> Option<(usize, Zone)> {
        let area = self.layout.rows?;
        let (x, y) = (p.x - area.l, p.y - area.t);
        let width = area.r - area.l;
        let row_height = self.s(ROW_HEIGHT as f64);
        if x < 0 || x >= width || y < 0 || y >= self.visible_rows as i32 * row_height {
            return None;
        }
        let index = self.top_row + (y / row_height) as usize;
        let right = width - if self.scrolling() { self.s(8.0) } else { 0 };
        if index >= self.rows.len() || x >= right {
            return None;
        }
        Some((index, if x >= right - self.s(30.0) { Zone::Delete } else { Zone::Row }))
    }

    fn item_at(&self, p: Point) -> Option<Item> {
        self.layout.items.iter().find(|(_, r)| inside(r, p)).map(|(item, _)| *item)
    }

    fn build(&mut self, m: &dyn Measure) {
        self.hover = None;
        self.pressed = None;
        self.item_hover = None;
        self.in_rows = false;
        self.visible_rows = self.rows.len().min(MAX_ROWS);
        self.layout = self.compute_layout(self.visible_rows, m);
        self.layout_key = self.current_key();
        self.clamp_scroll();
    }

    /// Место у якоря (N 852–875): над значком, правый край у якоря; на низком экране — меньше
    /// строк (не меньше 3); панель задач сверху — под якорем; поля 8.
    fn place(&mut self, m: &dyn Measure) {
        let width = self.s(WIDTH as f64);
        let mut height = self.layout.height;
        let work = self.work_area;
        let margin = self.s(8.0);
        let room = work.b - work.t - 2 * margin;
        if self.visible_rows > 0 && height > room {
            let row = self.s(ROW_HEIGHT as f64);
            let over = (height - room + row - 1) / row;
            let fewer = (self.visible_rows as i32 - over).max(3) as usize;
            if fewer < self.visible_rows {
                self.visible_rows = fewer;
                self.layout = self.compute_layout(fewer, m);
                self.clamp_scroll();
                height = self.layout.height;
            }
        }
        let mut x = self.anchor.x - width + self.s(24.0);
        let mut y = self.anchor.y - height - self.s(12.0);
        if y < work.t + margin {
            y = self.anchor.y + self.s(12.0);
        }
        x = (work.l + margin).max(x.min(work.r - margin - width));
        y = (work.t + margin).max(y.min(work.b - margin - height));
        self.geometry = Some(Rect { l: x, t: y, r: x + width, b: y + height });
    }

    /// Раскладка как у Tk-панели (N 749–832): рамка (только без скруглений Windows 11), заголовок,
    /// строки или «Выделите текст…», линия, подвал. Метки Tk добавляют к тексту 3 px с каждой
    /// стороны (рамка 2 + поле 1; эти пиксели Tk не масштабирует).
    fn compute_layout(&self, visible: usize, m: &dyn Measure) -> Layout {
        const LABEL_INSET: i32 = 3;
        let s = |v: f64| self.s(v);
        let border = if self.rounded { 0 } else { 1 };
        let width = s(WIDTH as f64);
        let content = width - 2 * border - 2 * s(6.0);
        let label = |style: TextStyle| m.height(style) + 2 * LABEL_INSET;

        let header_top = border + s(12.0);
        let header_height = s(14.0).max(label(TextStyle::Title)).max(label(TextStyle::Small));
        let header_bottom = header_top + header_height;
        let (header_left, header_right) = (border + s(14.0), width - border - s(14.0));
        let icon_top = header_top + (header_height - s(14.0)) / 2;
        let icon = Rect { l: header_left, t: icon_top, r: header_left + s(10.0), b: icon_top + s(14.0) };
        let title = Rect { l: icon.r + s(8.0) + LABEL_INSET, t: header_top, r: header_right - LABEL_INSET, b: header_bottom };
        let hotkey = Rect { l: header_left, t: header_top, r: header_right - LABEL_INSET, b: header_bottom };

        let mut y = header_bottom + s(6.0);
        let (rows, empty) = if visible > 0 {
            let area = Rect {
                l: border + s(6.0),
                t: y,
                r: border + s(6.0) + content,
                b: y + visible as i32 * s(ROW_HEIGHT as f64),
            };
            y = area.b + s(4.0);
            (Some(area), None)
        } else {
            let wrap = content - s(16.0);
            let (_, text_height) = m.wrapped(TextStyle::Text, &self.empty_text(), wrap);
            let top = y + s(2.0) + LABEL_INSET;
            let left = border + s(14.0) + LABEL_INSET;
            let area = Rect { l: left, t: top, r: left + wrap, b: top + text_height };
            y = area.b + LABEL_INSET + s(12.0);
            (None, Some(area))
        };
        let line = Rect { l: border + s(10.0), t: y, r: width - border - s(10.0), b: y + 1 };
        y = line.b + s(4.0);

        let left = border + s(6.0);
        let item_height = s(ITEM_HEIGHT as f64);
        let mut items = Vec::new();
        let mut full_row = |item: Item, y: &mut i32| {
            items.push((item, Rect { l: left, t: *y, r: left + content, b: *y + item_height }));
            *y += item_height;
        };
        if self.has_undo() {
            full_row(Item::Undo, &mut y);
        }
        if !self.rows.is_empty() {
            full_row(Item::DeleteAll, &mut y);
        }
        let half = content / 2;
        for (k, &item) in GRID.iter().enumerate() {
            let (row, column) = ((k / 2) as i32, (k % 2) as i32);
            let l = left + column * half;
            let r = if column == 0 { left + half } else { left + content };
            let t = y + row * item_height;
            items.push((item, Rect { l, t, r, b: t + item_height }));
        }
        y += 3 * item_height;
        let height = y + s(6.0) + border;
        Layout { width, height, border, icon, title, hotkey, rows, empty, line, items }
    }

    /// Строки (N 892–953), координаты холста строк сдвинуты на `area.l`, `area.t`.
    fn draw_rows(&self, out: &mut Vec<Draw>, area: Rect, m: &dyn Measure) {
        let s = |v: f64| self.s(v);
        let p = palette(self.light);
        let faint = faint(self.light);
        let (ox, oy) = (area.l, area.t);
        let width = area.r - area.l;
        let row_height = s(ROW_HEIGHT as f64);
        let scrolling = self.scrolling();
        let right = width - if scrolling { s(8.0) } else { 0 };
        let delete_left = right - s(30.0);
        let at = |l: i32, t: i32, r: i32, b: i32| Rect { l: ox + l, t: oy + t, r: ox + r, b: oy + b };
        for slot in 0..self.visible_rows {
            let index = self.top_row + slot;
            let Some(row) = self.rows.get(index) else { break };
            let top = slot as i32 * row_height;
            let middle = top + row_height / 2;
            let closed = row.status == Status::Closed;
            let hovered = self.hover.is_some_and(|h| h.0 == index);
            if hovered {
                let r = at(0, top + s(1.0), right, top + row_height - s(1.0));
                out.push(Draw { tag: Tag::Hover, op: Op::Round { r, radius: s(5.0), fill: p.hover, outline: None } });
            }
            let dot = if closed { mix(row.color, p.bg, 0.55) } else { row.color };
            let (radius, center) = (s(4.0), s(14.0));
            out.push(Draw {
                tag: Tag::Dot,
                op: Op::Round {
                    r: at(center - radius, middle - radius, center + radius, middle + radius),
                    radius,
                    fill: dot,
                    outline: Some(mix(dot, Rgb(0, 0, 0), 0.25)),
                },
            });
            let mut tag_left = delete_left;
            let tag = status_tag(row.status);
            if !tag.is_empty() {
                let tag_width = m.width(TextStyle::Tag, tag) + 2 * s(7.0);
                tag_left = delete_left - s(2.0) - tag_width;
                let r = at(tag_left, middle - s(9.0), tag_left + tag_width, middle + s(9.0));
                out.push(Draw { tag: Tag::StatusTag, op: Op::Round { r, radius: s(9.0), fill: tag_fill(self.light), outline: None } });
                out.push(Draw { tag: Tag::StatusTag, op: text_op(r, tag.to_string(), TextStyle::Tag, p.muted, Align::Center) });
            }
            let name_x = s(28.0);
            let name_room = ((tag_left - name_x) as f64 * 0.45) as i32;
            let name = fit_text(&|t| m.width(TextStyle::Name, t), &program_name(&row.program), name_room);
            let r = at(name_x, top, tag_left, top + row_height);
            let name_width = if name.is_empty() { 0 } else { m.width(TextStyle::Name, &name) + s(8.0) };
            out.push(Draw { tag: Tag::Name, op: text_op(r, name, TextStyle::Name, if closed { faint } else { p.fg }, Align::Left) });
            let text_x = name_x + name_width;
            let text = fit_text(&|t| m.width(TextStyle::Text, t), &one_line(&row.text), tag_left - s(8.0) - text_x);
            let r = at(text_x, top, tag_left, top + row_height);
            out.push(Draw { tag: Tag::Text, op: text_op(r, text, TextStyle::Text, if closed { faint } else { p.muted }, Align::Left) });
            let on_delete = hovered && self.hover.is_some_and(|h| h.1 == Zone::Delete);
            let cross = (delete_left + right) / 2;
            if on_delete {
                let r11 = s(11.0);
                let r = at(cross - r11, middle - r11, cross + r11, middle + r11);
                out.push(Draw { tag: Tag::Delete, op: Op::Round { r, radius: r11, fill: mix(p.bg, DELETE_COLOR, 0.22), outline: None } });
            }
            let r = at(delete_left, top, right, top + row_height);
            let color = if on_delete { DELETE_HOVER } else { DELETE_COLOR };
            out.push(Draw { tag: Tag::Delete, op: text_op(r, "✕".to_string(), TextStyle::Symbol, color, Align::Center) });
        }
        if scrolling {
            let track = self.visible_rows as i32 * row_height;
            let total = self.rows.len() as i32;
            let thumb = s(24.0).max(track * self.visible_rows as i32 / total);
            let offset = (track - thumb) * self.top_row as i32 / (total - self.visible_rows as i32).max(1);
            let r = at(width - s(5.0), offset + s(2.0), width - s(2.0), offset + thumb - s(2.0));
            out.push(Draw { tag: Tag::Scrollbar, op: Op::Round { r, radius: s(2.0), fill: faint, outline: None } });
        }
    }
}

fn text_op(r: Rect, s: String, style: TextStyle, c: Rgb, align: Align) -> Op {
    Op::Text { r, s, style, c, align, wrap: false }
}

fn inside(r: &Rect, p: Point) -> bool {
    p.x >= r.l && p.x < r.r && p.y >= r.t && p.y < r.b
}

// Приватные копии простых помощников части 1 (`core::text`): тесты панели не должны зависеть
// от её готовности. При интеграции можно заменить на `crate::core::text::*` — поведение то же.

/// Пробелы в одну строку (N 138–139): все пробельные символы Python `str.split()`.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Читаемое имя программы (N 130–135): 'msedge.exe' -> 'Edge'.
fn program_name(program: &str) -> String {
    const NAMES: [(&str, &str); 16] = [
        ("claude", "Claude"),
        ("chatgpt", "ChatGPT"),
        ("codex", "Codex"),
        ("antigravity", "Antigravity"),
        ("brave", "Brave"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("browser", "Яндекс"),
        ("editors", "ONLYOFFICE"),
        ("winword", "Word"),
        ("excel", "Excel"),
        ("powerpnt", "PowerPoint"),
        ("notepad", "Блокнот"),
        ("code", "VS Code"),
        ("anydesk", "AnyDesk"),
    ];
    let path = program.replace('/', "\\");
    let mut name = path.rsplit('\\').next().unwrap_or("").trim().to_string();
    if name.to_lowercase().ends_with(".exe") {
        let cut = name.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
        name.truncate(cut);
    }
    let lower = name.to_lowercase();
    if let Some((_, readable)) = NAMES.iter().find(|(key, _)| *key == lower) {
        return readable.to_string();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "закрыто" / "текст не найден" / "" (N 48).
fn status_tag(status: Status) -> &'static str {
    match status {
        Status::Shown => "",
        Status::Closed => "закрыто",
        Status::NotFound => "текст не найден",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_names_like_python() {
        let cases = [
            ("claude.exe", "Claude"),
            ("ChatGPT.exe", "ChatGPT"),
            ("antigravity.exe", "Antigravity"),
            ("msedge.exe", "Edge"),
            ("editors.exe", "ONLYOFFICE"),
            ("WINWORD.EXE", "Word"),
            ("notepad.exe", "Блокнот"),
            ("C:\\Apps\\telegram.exe", "Telegram"),
            ("C:/Apps/telegram.exe", "Telegram"),
            ("codex", "Codex"),
            ("", ""),
            (".exe", ""),
        ];
        for (exe, name) in cases {
            assert_eq!(program_name(exe), name, "{exe}");
        }
    }

    #[test]
    fn one_line_like_python() {
        assert_eq!(one_line("  a\n\tb  c "), "a b c");
        assert_eq!(one_line(""), "");
        assert_eq!(one_line("a\u{a0}b\x1fc"), "a b c");
    }
}
