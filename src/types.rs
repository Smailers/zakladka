//! Общие типы и сообщения между потоками (раздел 5.1 плана). Пишет и меняет только Claude.
//!
//! Все смещения в тексте — в кодовых точках (`char`), как в Python.

use std::sync::{mpsc, Arc};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::win::Wake;

/// Дескриптор окна как число: `Send`, без `HWND` из crate (там это указатель).
pub type Hwnd = isize;

/// `Hwnd` -> `HWND` для вызовов Win32.
#[inline]
pub fn to_hwnd(h: Hwnd) -> HWND {
    HWND(h as *mut std::ffi::c_void)
}

/// `HWND` -> `Hwnd`.
#[inline]
pub fn from_hwnd(h: HWND) -> Hwnd {
    h.0 as isize
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl From<POINT> for Point {
    fn from(p: POINT) -> Point {
        Point { x: p.x, y: p.y }
    }
}

impl From<Point> for POINT {
    fn from(p: Point) -> POINT {
        POINT { x: p.x, y: p.y }
    }
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Rect {
        Rect { l: r.left, t: r.top, r: r.right, b: r.bottom }
    }
}

impl From<Rect> for RECT {
    fn from(r: Rect) -> RECT {
        RECT { left: r.l, top: r.t, right: r.r, bottom: r.b }
    }
}

/// Цвет: `parse("#RRGGBB")`, `hex() -> "#RRGGBB"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Строго `#` и шесть шестнадцатеричных цифр (любой регистр), как
    /// `re.fullmatch(r"#[0-9A-Fa-f]{6}")` в `parse_config`. Мягкий разбор («#rgb», без «#») —
    /// `core::color::normalize_color`.
    pub fn parse(s: &str) -> Option<Rgb> {
        let digits = s.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        Some(Rgb(channel(0)?, channel(2)?, channel(4)?))
    }

    /// `"#RRGGBB"`, заглавные буквы.
    pub fn hex(&self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// `COLORREF` для GDI: `0x00BBGGRR`.
    pub fn colorref(&self) -> u32 {
        (self.0 as u32) | ((self.1 as u32) << 8) | ((self.2 as u32) << 16)
    }
}

/// `"pos"` | `"extra"`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Pos,
    Extra,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Pos => "pos",
            Kind::Extra => "extra",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "pos" => Some(Kind::Pos),
            "extra" => Some(Kind::Extra),
            _ => None,
        }
    }
}

/// `"uia"` | `"ocr"`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Uia,
    Ocr,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Uia => "uia",
            Mode::Ocr => "ocr",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "uia" => Some(Mode::Uia),
            "ocr" => Some(Mode::Ocr),
            _ => None,
        }
    }
}

/// `""` | `"closed"` | `"notfound"`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Shown,
    Closed,
    NotFound,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Shown => "",
            Status::Closed => "closed",
            Status::NotFound => "notfound",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "" => Some(Status::Shown),
            "closed" => Some(Status::Closed),
            "notfound" => Some(Status::NotFound),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Up,
    Down,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Bookmark {
    pub id: String,
    pub exe: String,
    pub kind: Kind,
    pub mode: Mode,
    pub quote: String, // плоский текст
    pub prefix: String,
    pub suffix: String,
    pub label: String,
    pub title: String,
    pub created: String, // "2026-11-19T22:40:07", местное время
    pub page: String,    // необязательные, "" если нет
    pub doc: String,
    pub url: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LabelColor {
    Auto,
    Custom(Rgb),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Config {
    pub hotkey_mark: String,
    pub hotkey_extra: String,
    pub color: Rgb,
    pub opacity: f64,
    pub bar_color: Rgb,
    pub label_color: LabelColor,
    pub max_age_days: u32,
    pub max_bookmarks: u32,
    pub hints: bool,
    pub ocr: bool,
    pub copy_fallback: bool,
    pub debug: bool,
}

/// `Arc<Shared>` у всех потоков.
pub struct Shared {
    pub config: std::sync::RwLock<Config>,
    pub profile: &'static crate::app_id::Profile,
    pub data_dir: std::path::PathBuf,
    pub debug: std::sync::atomic::AtomicBool,
}

/// UI -> tracker.
pub enum Command {
    Mark { hwnd: Hwnd, cursor: Point, extra: bool },
    JumpTo(String),
    Delete(String),
    DeleteAll,
    Undo,
    Pause(bool),
    Rerender,
    Flash(String),
    Goto(String),
    Status,
    ConfigChanged,
    Stop,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ItemRow {
    pub id: String,
    pub exe: String,
    pub label: String,
    pub created: String,
    pub kind: Kind,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct RenderState {
    pub marks: Vec<(String, Vec<Rect>)>,
    pub pills: Vec<(Dir, String)>,
    pub viewport: Option<Rect>,
}

/// tracker/locator -> UI.
pub enum UiEvent {
    Toast { text: String, at: Option<Point>, important: bool },
    Items { rows: Vec<ItemRow>, undo: Option<String> }, // undo = подпись последней удалённой
    Statuses(Vec<(String, Status)>),
    Render(RenderState), // только при debug
    Fatal(String),
}

/// UI -> tracker: `mpsc::Sender<Command>` + `Wake::signal()` (поток просыпается сразу).
#[derive(Clone)]
pub struct CommandSender {
    tx: mpsc::Sender<Command>,
    wake: Arc<Wake>,
}

impl CommandSender {
    pub fn new(tx: mpsc::Sender<Command>, wake: Arc<Wake>) -> CommandSender {
        CommandSender { tx, wake }
    }

    /// Новый канал: отправитель и приёмник (приёмник — потоку tracker).
    pub fn channel(wake: Arc<Wake>) -> (CommandSender, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel();
        (CommandSender::new(tx, wake), rx)
    }

    /// Событие, которое будит tracker (оно же нужно `spawn_locator`).
    pub fn wake(&self) -> &Arc<Wake> {
        &self.wake
    }

    /// Отправить и разбудить. Если tracker уже завершился, команда молча теряется.
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
        self.wake.signal();
    }
}

/// tracker/locator -> UI: `mpsc::Sender<UiEvent>` + `PostMessageW(owner, WM_APP_EVENTS, 0, 0)`.
/// UI-поток на `WM_APP_EVENTS` вычитывает приёмник целиком (`try_recv` до пустого).
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<UiEvent>,
    owner: Hwnd,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<UiEvent>, owner: Hwnd) -> EventSink {
        EventSink { tx, owner }
    }

    /// Новый канал для окна-владельца `owner`: отправитель и приёмник (приёмник — UI-потоку).
    pub fn channel(owner: Hwnd) -> (EventSink, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel();
        (EventSink::new(tx, owner), rx)
    }

    /// Отправить и разбудить UI-поток. Если UI уже закрыт, событие молча теряется.
    pub fn send(&self, e: UiEvent) {
        if self.tx.send(e).is_ok() {
            // SAFETY: PostMessageW с любым числом-дескриптором безопасен; ошибка (окна уже нет)
            // не важна — тогда и читать некому.
            unsafe {
                let _ = PostMessageW(to_hwnd(self.owner), WM_APP_EVENTS, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Сколько окон меток создаёт UI заранее (раздел 4) и какие пределы у tracker (часть 4).
pub const HIGHLIGHTS: usize = 24;
pub const BARS: usize = 6;
pub const FLAGS: usize = 6;

/// Создаёт UI, получает tracker.
pub struct OverlayWindows {
    pub highlights: Vec<Hwnd>, // HIGHLIGHTS = 24
    pub bars: Vec<Hwnd>,       // BARS = 6
    pub flags: Vec<Hwnd>,      // FLAGS = 6
    pub pill_up: Hwnd,
    pub pill_down: Hwnd,
    pub pill_size: (i32, i32),
    pub flag_size: i32,
}

/// Привязка «флажок/плашка -> id закладки»: пишет tracker (`Overlay::apply`), читает UI при щелчке.
#[derive(Default)]
pub struct PieceBindings {
    pub flags: [Option<String>; 6],
    pub pill_up: Option<String>,
    pub pill_down: Option<String>,
}

pub const WM_APP_TRAY: u32 = 0x8001; // как в Python
pub const WM_APP_EVENTS: u32 = 0x8002;
