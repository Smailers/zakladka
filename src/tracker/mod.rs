//! Поток tracker (часть 4): цикл, команды, помощники Worker (P 1563–1736, 1818–1829, 1979–1997).
//! state.rs — Claude; follow.rs, overlay.rs — часть 4; mark.rs, locator.rs — часть 5.

pub mod follow;
pub mod locator;
pub mod mark;
pub mod overlay;
pub mod state;

pub use locator::{spawn_locator, LocatorHandle, Request, ScanResult};
pub use overlay::Overlay;
pub use state::{FlatEntry, Item, Scope, Seg, Track, Worker};

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::core::flatten::{has_cyrillic, to_flat, FlatDoc};
use crate::core::geometry::{merge_lines, shift};
use crate::core::rules::{page_key, same_title};
use crate::ocr::Ocr;
use crate::store::{load_bookmarks, log_error, log_timing, now_local, prune, save_bookmarks};
use crate::tracker::follow::{tick_wait, wait_left, STEP_IDLE};
use crate::types::{
    Bookmark, Command, CommandSender, Config, EventSink, Hwnd, ItemRow, Mode, OverlayWindows, PieceBindings, Point,
    Rect, Shared, UiEvent,
};
use crate::uia::{Element, ScopeKey, Segment, TextPattern, Uia};
use crate::win::{monotonic, Api, Wake};

/// «Вернуть» помнит не больше стольких пачек.
const UNDO_LIMIT: usize = 20;
/// `Ocr` отпускается, если столько секунд не был нужен (раздел 5.5 плана).
const OCR_KEEP_S: f64 = 60.0;
/// Тик дольше — `SLOW_TICK` в timing.log.
const SLOW_TICK_S: f64 = 0.3;
/// `Overlay::guard()` — раз в столько секунд.
const GUARD_S: f64 = 0.5;
/// `TICKS` при debug — раз в секунду.
const TICKS_S: f64 = 1.0;
/// Мигнуть при появлении: после перехода из панели и после «Вернуть».
const FLASH_GOTO_S: f64 = 3.0;
const FLASH_UNDO_S: f64 = 5.0;
/// Сколько символов подписи показывает «Возвращена: «…»».
const UNDO_LABEL: usize = 28;
/// Зазор `merge_lines` (в Python — значение по умолчанию).
pub(crate) const MERGE_GAP: i32 = 14;

/// Item из закладки: плоские quote/prefix/suffix разбираются один раз.
pub fn item_of(b: Bookmark) -> Item {
    Item { quote: to_flat(&b.quote), prefix: to_flat(&b.prefix), suffix: to_flat(&b.suffix), b }
}

/// Копия настроек (замок держится только на время копирования).
pub(crate) fn read_config(shared: &Shared) -> Config {
    shared.config.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Миллисекунды `monotonic()` — для `heartbeat` (UI сравнивает по тем же часам).
fn now_ms() -> u64 {
    (monotonic() * 1000.0).max(0.0) as u64
}

/// Тело цикла в `catch_unwind`: паника -> `PANIC` в error.log (только место, не текст) и
/// `reset_window()`, как `except Exception` в Python.
fn guarded(worker: &mut Worker, body: impl FnOnce(&mut Worker)) {
    if catch_unwind(AssertUnwindSafe(|| body(&mut *worker))).is_err() {
        log_error(&worker.shared, "PANIC");
        let _ = catch_unwind(AssertUnwindSafe(|| worker.reset_window()));
    }
}

pub struct TrackerHandle {
    pub commands: CommandSender,
    pub heartbeat: Arc<AtomicU64>,
    pub join: JoinHandle<()>,
}

pub fn spawn(
    shared: Arc<Shared>,
    api: Arc<Api>,
    events: EventSink,
    windows: OverlayWindows,
    bindings: Arc<Mutex<PieceBindings>>,
    owner: Hwnd,
) -> TrackerHandle {
    let wake = Arc::new(Wake::new());
    let (commands, receiver) = CommandSender::channel(wake);
    let heartbeat = Arc::new(AtomicU64::new(now_ms()));
    let overlay = Overlay::new(windows, bindings, read_config(&shared).opacity);
    let (own_commands, own_heartbeat) = (commands.clone(), heartbeat.clone());
    let join = std::thread::Builder::new()
        .name("tracker".to_string())
        .spawn(move || run_thread(shared, api, events, own_commands, receiver, overlay, owner, own_heartbeat))
        .expect("поток tracker");
    TrackerHandle { commands, heartbeat, join }
}

/// Весь поток tracker: MTA -> `Uia::new()` (ошибка -> `UiEvent::Fatal`) -> locator -> закладки ->
/// `UiEvent::Items` -> цикл до `Command::Stop`.
#[allow(clippy::too_many_arguments)]
fn run_thread(
    shared: Arc<Shared>,
    api: Arc<Api>,
    events: EventSink,
    commands: CommandSender,
    receiver: Receiver<Command>,
    overlay: Overlay,
    owner: Hwnd,
    heartbeat: Arc<AtomicU64>,
) {
    // SAFETY: инициализация COM своего потока (MTA); парный CoUninitialize — в конце потока.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let uia = match Uia::new() {
        Ok(uia) => Arc::new(uia),
        Err(()) => {
            log_error(&shared, "UIA_INIT");
            events.send(UiEvent::Fatal("Не удалось запустить UI Automation.".to_string()));
            if com {
                // SAFETY: парный вызов к успешному CoInitializeEx этого потока.
                unsafe { CoUninitialize() };
            }
            return;
        }
    };
    let locator =
        spawn_locator(shared.clone(), api.clone(), uia.clone(), events.clone(), commands.clone(), commands.wake().clone());
    let mut worker = Worker::new(shared, api, uia, events, commands, owner, overlay, locator, heartbeat);
    guarded(&mut worker, |w| {
        w.load_items();
        w.publish_items();
    });
    worker.run_loop(&receiver);
    // Сначала отпустить все объекты COM (и отправитель заявок — locator завершится), потом COM.
    drop(worker);
    if com {
        // SAFETY: парный вызов к успешному CoInitializeEx этого потока; объекты COM уже отпущены.
        unsafe { CoUninitialize() };
    }
}

/// Создание, загрузка и цикл (P 1563–1629).
impl Worker {
    /// Состояние потока tracker. Закладки не читаются (`load_items` в потоке); `ocr_available` =
    /// `config.ocr && Ocr::supported()` — один раз.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared: Arc<Shared>,
        api: Arc<Api>,
        uia: Arc<Uia>,
        events: EventSink,
        commands: CommandSender,
        owner: Hwnd,
        overlay: Overlay,
        locator: LocatorHandle,
        heartbeat: Arc<AtomicU64>,
    ) -> Worker {
        let ocr_available = read_config(&shared).ocr && Ocr::supported();
        let store_path = shared.data_dir.join("bookmarks.json");
        Worker {
            shared,
            api,
            uia,
            events,
            commands,
            owner,
            store_path,
            ocr: None,
            ocr_available,
            ocr_last_use: 0.0,
            ocr_visible: HashMap::new(),
            ocr_thumb: None,
            ocr_window: None,
            ocr_stable: 0,
            ocr_next: 0.0,
            ocr_dirty: true,
            items: Vec::new(),
            undo: Vec::new(),
            tracks: HashMap::new(),
            flats: HashMap::new(),
            scopes: Vec::new(),
            target: 0,
            pid: 0,
            exe: String::new(),
            window: None,
            target_chromium: false,
            next_scopes: 0.0,
            wait: STEP_IDLE,
            paused: false,
            jump_cycle: 0,
            scanning: HashSet::new(),
            bands: Vec::new(),
            pending_flash: HashMap::new(),
            last_render: None,
            overlay,
            locator,
            heartbeat,
        }
    }

    /// bookmarks.json -> `items` (битый файл `load_bookmarks` откладывает в `*.bad` сам).
    fn load_items(&mut self) {
        let config = read_config(&self.shared);
        let loaded =
            load_bookmarks(&self.store_path, &now_local(), config.max_age_days, config.max_bookmarks as usize);
        self.items = loaded.into_iter().map(item_of).collect();
    }

    /// Ожидание `Wake::wait(remaining)`: команда или результат сканирования будят сразу; затем
    /// все команды по очереди и тик. `Stop` — выход.
    fn run_loop(&mut self, receiver: &Receiver<Command>) {
        let wake = self.commands.wake().clone();
        let (mut last_tick, mut last_guard) = (0.0f64, 0.0f64);
        let (mut ticks, mut ticks_since) = (0u32, monotonic());
        loop {
            self.heartbeat.store(now_ms(), Ordering::Relaxed);
            let woken = wake.wait(wait_left(last_tick, self.wait, monotonic()));
            let mut handled = false;
            while let Ok(command) = receiver.try_recv() {
                if matches!(command, Command::Stop) {
                    return;
                }
                handled = true;
                guarded(self, |w| w.handle(command));
            }
            if !(woken || handled || monotonic() - last_tick >= self.wait) {
                continue;
            }
            let started = monotonic();
            guarded(self, |w| w.tick());
            last_tick = monotonic();
            if last_tick - started > SLOW_TICK_S {
                log_timing(&self.shared, "SLOW_TICK", (last_tick - started) * 1000.0);
            }
            if self.shared.debug.load(Ordering::Relaxed) {
                ticks += 1;
                if last_tick - ticks_since >= TICKS_S {
                    if !self.tracks.is_empty() {
                        log_timing(&self.shared, "TICKS", f64::from(ticks));
                    }
                    (ticks, ticks_since) = (0, last_tick);
                }
            }
            if self.ocr.is_some() && last_tick - self.ocr_last_use > OCR_KEEP_S {
                self.ocr = None; // ~25 МБ движков возвращаются системе
            }
            if last_tick - last_guard > GUARD_S {
                last_guard = last_tick;
                guarded(self, |w| w.overlay.guard());
            }
        }
    }

    /// Команды UI (P 1699–1736) и `ConfigChanged`.
    fn handle(&mut self, command: Command) {
        match command {
            Command::Mark { hwnd, cursor, extra } => self.do_mark(hwnd, cursor, extra),
            Command::JumpTo(id) => {
                let failed = match self.tracks.get(&id) {
                    Some(track) => match track.segs.first() {
                        Some(seg) => !self.uia.scroll_to(&seg.element, track.pattern.as_ref()),
                        None => false,
                    },
                    None => false,
                };
                if failed {
                    self.toast("Не получилось прокрутить к закладке".to_string(), None, false);
                }
            }
            Command::Delete(id) => {
                self.delete(&[id], true);
            }
            Command::DeleteAll => {
                let ids: Vec<String> = self.items.iter().map(|item| item.b.id.clone()).collect();
                self.delete(&ids, true);
            }
            Command::Undo => self.undo_last(),
            Command::Pause(paused) => self.paused = paused,
            Command::Rerender => {
                self.last_render = None; // UI спрятал метки сам: поставить их заново
                self.overlay.forget_shown();
            }
            Command::Flash(id) => {
                self.pending_flash.insert(id, monotonic() + FLASH_GOTO_S);
            }
            Command::Goto(id) => {
                if let Some(item) = self.item_by_id(&id) {
                    let _ = self.locator.requests.send(Request::Goto(item.b.clone()));
                }
            }
            Command::Status => {
                let all: Vec<Bookmark> = self.items.iter().map(|item| item.b.clone()).collect();
                let _ = self.locator.requests.send(Request::Status(all));
            }
            Command::ConfigChanged => {
                let opacity = read_config(&self.shared).opacity;
                self.overlay.set_opacity(opacity);
            }
            Command::Stop => {} // обрабатывается в цикле
        }
    }

    /// «Вернуть»: последняя пачка удалённых обратно, сразу искать и мигнуть при появлении.
    fn undo_last(&mut self) {
        let Some(batch) = self.undo.pop() else { return };
        let known: HashSet<String> = self.items.iter().map(|item| item.b.id.clone()).collect();
        self.items.extend(batch.iter().filter(|b| !known.contains(&b.id)).cloned().map(item_of));
        for entry in self.flats.values_mut() {
            entry.dirty = true;
        }
        let until = monotonic() + FLASH_UNDO_S;
        for b in &batch {
            self.pending_flash.insert(b.id.clone(), until);
        }
        self.save();
        if let Some(first) = batch.first() {
            let short: String = first.label.chars().take(UNDO_LABEL).collect();
            let more = if first.label.chars().count() > UNDO_LABEL { "…" } else { "" };
            self.toast(format!("Возвращена: «{short}{more}»"), None, false);
        }
    }

    pub(crate) fn item_by_id(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.b.id == id)
    }
}

/// Помощники, которыми пользуется часть 5 — сигнатуры неизменны.
impl Worker {
    /// Короткое сообщение у курсора; неважные UI не показывает при выключенных «Подсказках».
    pub fn toast(&self, text: String, at: Option<Point>, important: bool) {
        self.events.send(UiEvent::Toast { text, at, important });
    }

    pub fn publish_items(&self) {
        let rows = self
            .items
            .iter()
            .map(|item| ItemRow {
                id: item.b.id.clone(),
                exe: item.b.exe.clone(),
                label: item.b.label.clone(),
                created: item.b.created.clone(),
                kind: item.b.kind,
            })
            .collect();
        let undo = self.undo.last().and_then(|batch| batch.first()).map(|b| b.label.clone());
        self.events.send(UiEvent::Items { rows, undo });
    }

    /// Срок и предел (`prune`), убрать слежение за исчезнувшими, записать файл, сообщить UI.
    pub fn save(&mut self) {
        let config = read_config(&self.shared);
        let all: Vec<Bookmark> = self.items.iter().map(|item| item.b.clone()).collect();
        let kept = prune(all, &now_local(), config.max_age_days, config.max_bookmarks as usize);
        let alive: HashSet<String> = kept.iter().map(|b| b.id.clone()).collect();
        self.tracks.retain(|id, _| alive.contains(id));
        self.ocr_visible.retain(|id, _| alive.contains(id));
        if save_bookmarks(&self.store_path, &kept).is_err() {
            log_error(&self.shared, "SAVE");
        }
        self.items = kept.into_iter().map(item_of).collect();
        self.publish_items();
    }

    /// `false` — ни одной такой закладки нет. `undoable` — пачка попадает в «Вернуть» (до 20).
    pub fn delete(&mut self, ids: &[String], undoable: bool) -> bool {
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let removed: Vec<Bookmark> =
            self.items.iter().filter(|item| ids.contains(item.b.id.as_str())).map(|item| item.b.clone()).collect();
        if removed.is_empty() {
            return false;
        }
        self.items.retain(|item| !ids.contains(item.b.id.as_str()));
        if undoable {
            self.undo.push(removed);
            if self.undo.len() > UNDO_LIMIT {
                let extra = self.undo.len() - UNDO_LIMIT;
                self.undo.drain(..extra);
            }
        }
        for id in &ids {
            self.tracks.remove(*id);
            self.ocr_visible.remove(*id);
        }
        self.save();
        true
    }

    /// Забыть всё о текущем окне (новое окно, паника тика).
    pub fn reset_window(&mut self) {
        self.tracks.clear();
        self.flats.clear();
        self.scopes.clear();
        self.scanning.clear();
        self.bands.clear();
        self.target = 0;
        self.pid = 0;
        self.exe.clear();
        self.window = None;
        self.next_scopes = 0.0;
        self.jump_cycle = 0;
        self.ocr_visible.clear();
        self.ocr_thumb = None;
        self.ocr_window = None;
        self.ocr_stable = 0;
        self.ocr_next = 0.0;
        self.ocr_dirty = true;
    }

    /// Над `r` нет окна другой программы: проверяются левый край, середина и правый край
    /// (наши метки не считаются).
    pub fn uncovered(&self, r: Rect) -> bool {
        let middle_y = half_sum(r.t, r.b);
        let xs = [r.l.saturating_add(3).min(r.r), half_sum(r.l, r.r), r.r.saturating_sub(3).max(r.l)];
        xs.into_iter().all(|x| match self.api.top_window_at(Point { x, y: middle_y }) {
            Some(hwnd) => hwnd == self.target || self.api.process(hwnd).0 == self.api.own_pid(),
            None => false,
        })
    }

    /// Видимые сейчас метки: и по элементам, и по тексту экрана (OCR).
    pub fn visible_marks(&self) -> Vec<(String, Vec<Rect>)> {
        let mut marks: Vec<(String, Vec<Rect>)> = self
            .tracks
            .values()
            .filter(|track| !track.visible.is_empty())
            .map(|track| (track.id.clone(), track.visible.clone()))
            .collect();
        for (id, lines) in &self.ocr_visible {
            match marks.iter_mut().find(|(known, _)| known == id) {
                Some(entry) => entry.1 = lines.clone(),
                None => marks.push((id.clone(), lines.clone())),
            }
        }
        marks
    }

    pub fn render_empty(&mut self) {
        self.render(true, monotonic());
    }

    /// Один шаг слежения (P 2000–2056): цель = активное окно; документы -> поиск -> следование ->
    /// метки по тексту экрана -> отрисовка; затем шаг ожидания.
    pub fn tick(&mut self) {
        let hwnd = self.api.foreground();
        if self.paused || hwnd == 0 {
            self.wait = STEP_IDLE;
            return self.render_empty();
        }
        let (mut pid, mut exe) = self.api.process(hwnd);
        if pid == self.api.own_pid() {
            return self.render_empty(); // наша панель или подсказка: состояние не трогать
        }
        if hwnd != self.target {
            self.api.forget_pid(pid); // повторно выданный pid не должен сохранить старое имя
            (pid, exe) = self.api.process(hwnd);
            self.reset_window();
            self.target = hwnd;
            self.pid = pid;
            self.exe = exe.clone();
            self.target_chromium = self.api.is_chromium(hwnd);
        }
        if !self.items.iter().any(|item| item.b.exe == exe) || self.api.is_iconic(hwnd) {
            for track in self.tracks.values_mut() {
                // после разворачивания — дождаться, пока окно успокоится
                track.el_rects = None;
                track.stable = 0;
                track.visible.clear();
            }
            self.ocr_visible.clear();
            self.ocr_thumb = None;
            self.wait = STEP_IDLE;
            return self.render_empty();
        }
        let now = monotonic();
        // P 2023–2027 по замыслу: окно сдвинулось или изменило размер — его полосы перекрытия
        // уехали вместе с ним, их надо выучить заново.
        let window = self.api.window_rect(hwnd);
        if window != self.window {
            self.bands.clear();
        }
        self.window = window;
        let element_marks: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.b.exe == exe && item.b.mode == Mode::Uia)
            .map(|item| item.b.id.clone())
            .collect();
        let screen_marks: Vec<String> = if self.ocr_available {
            let title = self.api.title(hwnd);
            self.items
                .iter()
                .filter(|item| item.b.exe == exe && item.b.mode == Mode::Ocr && same_title(&item.b.title, &title))
                .map(|item| item.b.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        let idle = self.api.idle_seconds();
        if !element_marks.is_empty() {
            if now >= self.next_scopes {
                self.refresh_scopes();
                self.next_scopes = now + if idle < 1.0 { 0.5 } else { 1.0 };
            }
            self.locate(&element_marks, now);
            self.follow(now);
        } else {
            self.tracks.clear();
        }
        if !screen_marks.is_empty() {
            self.ocr_last_use = now; // OCR-закладки в активном окне держат движки
        }
        self.follow_screen(&screen_marks, now);
        self.render(false, now);
        let tracks: Vec<(f64, bool)> =
            self.tracks.values().map(|track| (track.last_move, !track.visible.is_empty())).collect();
        let flashing = !self.pending_flash.is_empty() || self.overlay.flashing(now);
        self.wait = tick_wait(now, idle, &tracks, !screen_marks.is_empty(), flashing);
    }

    /// TextPattern документа; новый документ добавляется в `scopes`.
    pub fn text_pattern_for(&mut self, key: &ScopeKey, scope: &Element) -> Option<TextPattern> {
        if let Some(existing) = self.scopes.iter().find(|s| &s.key == key) {
            return existing.pattern.clone();
        }
        let pattern = self.uia.text_pattern(scope);
        let rect = self.uia.current_rect(scope);
        let (url, name) = self.uia.document_info(scope);
        self.scopes.push(Scope {
            key: key.clone(),
            element: scope.clone(),
            pattern: pattern.clone(),
            rect,
            page: page_key(&url, &name),
        });
        pattern
    }

    pub fn make_track(&self, item: &Item, key: &ScopeKey, pattern: Option<TextPattern>, flat: &FlatDoc, segs: &[Segment], start: usize) -> Track {
        let end = start.saturating_add(item.quote.len());
        let parts = flat
            .spans(start as isize, end as isize)
            .into_iter()
            .filter_map(|(segment, first, last)| {
                let s = segs.get(segment)?;
                Some(Seg { element: s.element.clone(), rid: self.uia.rid(&s.element), name: s.name.clone(), first, last })
            })
            .collect();
        Track {
            id: item.b.id.clone(),
            kind: item.b.kind,
            created: item.b.created.clone(),
            scope_key: key.clone(),
            pattern,
            segs: parts,
            strong: true,
            allowed: None,
            outer: HashSet::new(),
            el_rects: None,
            lines: Vec::new(),
            stable: 0,
            visible: Vec::new(),
            direction: None,
            last_move: 0.0,
            next_check: 0.0,
            next_names: 0.0,
            probe: None,
            sample: 0,
        }
    }

    /// Создать Ocr по требованию, обновить ocr_last_use.
    /// `false` — OCR недоступен (выключен, нет в Windows, не создался — тогда больше не пробуем).
    pub fn ensure_ocr(&mut self) -> bool {
        if !self.ocr_available {
            return false;
        }
        if self.ocr.is_none() {
            match Ocr::new() {
                Ok(ocr) => self.ocr = Some(ocr),
                Err(()) => {
                    log_error(&self.shared, "OCR_INIT");
                    self.ocr_available = false;
                    return false;
                }
            }
        }
        self.ocr_last_use = monotonic();
        true
    }

    /// OCR окна: (FlatDoc слов, прямоугольники слов в пикселях экрана). Нет прямоугольника окна —
    /// пустой результат; `None` — ошибка (OCR недоступен, снимок или распознавание не удались).
    pub fn screen_words(&mut self, hwnd: Hwnd, hint: &str) -> Option<(FlatDoc, Vec<Rect>)> {
        let Some(rect) = self.api.window_rect(hwnd) else {
            return Some((FlatDoc::new::<&str>(&[]), Vec::new()));
        };
        if !self.ensure_ocr() {
            return None;
        }
        let bitmap = self.api.capture(rect, None)?;
        let words = self.ocr.as_ref()?.words(&bitmap, has_cyrillic(hint)).ok()?;
        let rects = words.iter().map(|word| shift(word.rect, rect.l, rect.t)).collect();
        let names: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        Some((FlatDoc::new(&names), rects))
    }

    /// Строки экрана для куска `[start, start + length)` плоского текста слов.
    pub fn screen_lines(&self, flat: &FlatDoc, rects: &[Rect], start: usize, length: usize) -> Vec<Rect> {
        let end = start.saturating_add(length);
        let words: Vec<Rect> = flat
            .spans(start as isize, end as isize)
            .into_iter()
            .filter_map(|(segment, _, _)| rects.get(segment).copied())
            .collect();
        merge_lines(&words, MERGE_GAP)
    }
}

/// `(a + b) // 2` Python (округление вниз) без переполнения.
pub(crate) fn half_sum(a: i32, b: i32) -> i32 {
    (i64::from(a) + i64::from(b)).div_euclid(2) as i32
}
