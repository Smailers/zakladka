//! Состояние потока tracker (раздел 5.6 плана): Worker, Track, FlatEntry и др. — полностью.
//! Пишет и меняет только Claude; части 4 и 5 пользуются полями напрямую.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::core::flatten::FlatDoc;
use crate::ocr::Ocr;
use crate::tracker::locator::LocatorHandle;
use crate::tracker::overlay::Overlay;
use crate::types::{Bookmark, CommandSender, Dir, EventSink, Hwnd, Kind, Point, Rect, RenderState, Shared};
use crate::uia::{Element, RuntimeId, ScopeKey, Segment, TextPattern};
use crate::win::Api;

/// Сегмент документа, через который проходит цитата закладки.
pub struct Seg {
    pub element: Element,
    pub rid: Option<RuntimeId>,
    pub name: String,
    pub first: usize, // позиции в имени сегмента, в char
    pub last: usize,
}

/// Где одна закладка в текущем окне (только во время работы).
pub struct Track {
    pub id: String,
    pub kind: Kind,
    pub created: String,
    pub scope_key: ScopeKey,
    pub pattern: Option<TextPattern>,
    pub segs: Vec<Seg>,
    pub strong: bool,                          // найдена вместе со своим контекстом
    pub allowed: Option<HashSet<RuntimeId>>,   // id элементов цитаты и их контейнеров
    pub outer: HashSet<RuntimeId>,             // id документа и выше
    pub el_rects: Option<Vec<Rect>>,           // прямоугольники элементов, для которых считались lines
    pub lines: Vec<Rect>,                      // прямоугольники строк цитаты (пиксели экрана)
    pub stable: u8,
    pub visible: Vec<Rect>,
    pub direction: Option<Dir>,
    pub last_move: f64,
    pub next_check: f64,                       // следующая проверка перекрытия (hit test)
    pub next_names: f64,                       // следующая проверка, что текст не изменился
    pub probe: Option<(Point, f64)>,           // (точка, время) подготовленного hit test Chromium
    pub sample: usize,                         // какую строку смотрит следующий hit test
}

impl Track {
    /// Python `signature`: (rid, name, first, last) всех сегментов совпадают по порядку.
    pub fn signature_eq(&self, other: &Track) -> bool {
        self.segs.len() == other.segs.len()
            && self.segs.iter().zip(&other.segs).all(|(a, b)| {
                a.rid == b.rid && a.name == b.name && a.first == b.first && a.last == b.last
            })
    }
}

/// Плоский текст документа и его сегменты (кэш сканирования).
pub struct FlatEntry {
    pub flat: FlatDoc,
    pub segs: Vec<Segment>,
    pub next: f64,
    pub dirty: bool,
    pub delay: f64, // растёт, пока пересканирование не находит нового
}

/// Документ (или корень окна), показанный сейчас в целевом окне.
pub struct Scope {
    pub key: ScopeKey,
    pub element: Element,
    pub pattern: Option<TextPattern>,
    pub rect: Option<Rect>,
    pub page: String,
}

/// Закладка с плоскими полями, разобранными один раз.
pub struct Item {
    pub b: Bookmark,
    pub quote: Vec<char>,
    pub prefix: Vec<char>,
    pub suffix: Vec<char>,
}

/// Всё состояние потока tracker. Создаётся внутри потока (после `Uia::new()`), между потоками не ходит.
pub struct Worker {
    pub shared: Arc<Shared>,
    pub api: Arc<Api>,
    pub uia: Arc<crate::uia::Uia>,
    pub events: EventSink,
    pub commands: CommandSender, // свой же отправитель: `commands.wake()` — событие пробуждения
    pub owner: Hwnd,             // окно-владелец буфера обмена для Ctrl+C
    pub store_path: PathBuf,

    pub ocr: Option<Ocr>,
    pub ocr_available: bool, // config.ocr && Ocr::supported(), один раз при запуске
    pub ocr_last_use: f64,
    pub ocr_visible: HashMap<String, Vec<Rect>>,
    pub ocr_thumb: Option<Vec<u8>>,
    pub ocr_window: Option<Rect>,
    pub ocr_stable: u32,
    pub ocr_next: f64,
    pub ocr_dirty: bool,

    pub items: Vec<Item>,
    pub undo: Vec<Vec<Bookmark>>, // не более 20 пачек
    pub tracks: HashMap<String, Track>,
    pub flats: HashMap<ScopeKey, FlatEntry>,
    pub scopes: Vec<Scope>,

    pub target: Hwnd,
    pub pid: u32,
    pub exe: String,
    pub window: Option<Rect>,
    pub target_chromium: bool,

    pub next_scopes: f64,
    pub wait: f64,
    pub paused: bool,
    pub jump_cycle: usize,
    pub scanning: HashSet<ScopeKey>,          // документы, сканирование которых идёт
    pub bands: Vec<(i32, i32)>,               // полосы y, где программа рисует поверх своего текста
    pub pending_flash: HashMap<String, f64>,  // id -> срок, до которого мигнуть при появлении
    pub last_render: Option<RenderState>,
    pub overlay: Overlay,
    pub locator: LocatorHandle,
    pub heartbeat: Arc<AtomicU64>, // мс monotonic
}
