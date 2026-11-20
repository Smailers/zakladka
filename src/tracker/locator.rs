//! Поток locator (часть 5; P 1906–1977, 2432–2465): сканирование документа, «закрыто», переход.
//!
//! Медленная работа UI Automation вне потока tracker: сканирование документа (50–80 мс),
//! статусы «закрыто» для панели и переход к закладке. Поток живёт, пока жив `LocatorHandle`
//! (отправитель заявок у `Worker`): отправитель закрыт — цикл кончается.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::core::flatten::{to_flat, FlatDoc};
use crate::core::matching::{best_occurrence, similarity};
use crate::core::rules::{page_key, same_title, title_key, BROWSERS};
use crate::store::log_error;
use crate::types::{Bookmark, Command, CommandSender, EventSink, Hwnd, Mode, Shared, Status, UiEvent};
use crate::uia::{Element, ScopeKey, Segment, Uia};
use crate::win::{monotonic, Api, Wake};

/// Предел узлов одного сканирования (`segments(limit=30000)` в Python).
const SEGMENT_LIMIT: usize = 30_000;
/// Сколько ждать документ с нужной страницей после перехода, и шаг ожидания (P 1961–1974).
const GOTO_WAIT_S: f64 = 2.5;
const GOTO_STEP: Duration = Duration::from_millis(150);
/// `best_occurrence(min_score=0.6)` по умолчанию в Python.
const MIN_SCORE: f64 = 0.6;

/// tracker -> locator.
pub enum Request {
    Scan { key: ScopeKey, element: Element },
    Status(Vec<Bookmark>),
    Goto(Bookmark),
}

/// locator -> tracker (+ `wake.signal()`).
pub struct ScanResult {
    pub key: ScopeKey,
    pub segs: Vec<Segment>,
    pub cost_s: f64,
}

pub struct LocatorHandle {
    pub requests: mpsc::Sender<Request>,
    pub results: mpsc::Receiver<ScanResult>,
}

/// Всё, чем пользуется поток locator.
struct Locator {
    shared: Arc<Shared>,
    api: Arc<Api>,
    uia: Arc<Uia>,
    events: EventSink,
    commands: CommandSender,
    wake: Arc<Wake>,
    results: mpsc::Sender<ScanResult>,
}

pub fn spawn_locator(
    shared: Arc<Shared>,
    api: Arc<Api>,
    uia: Arc<Uia>,
    events: EventSink,
    commands: CommandSender,
    wake: Arc<Wake>,
) -> LocatorHandle {
    let (requests, inbox) = mpsc::channel::<Request>();
    let (results_tx, results) = mpsc::channel::<ScanResult>();
    let log = shared.clone();
    let locator = Locator { shared, api, uia, events, commands, wake, results: results_tx };
    let spawned = std::thread::Builder::new()
        .name("zakladka-locator".into())
        .spawn(move || locator.run(inbox));
    if spawned.is_err() {
        // Заявки уйдут в закрытый канал и молча потеряются; слежение за найденным продолжится.
        log_error(&log, "LOCATOR_SPAWN");
    }
    LocatorHandle { requests, results }
}

impl Locator {
    fn run(self, inbox: mpsc::Receiver<Request>) {
        // SAFETY: обычный вызов COM в начале собственного потока.
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        // Отправитель у Worker закрыт (поток tracker кончился) — выход.
        while let Ok(request) = inbox.recv() {
            // Ключ сканирования — чтобы после паники снять его из `scanning` пустым результатом,
            // иначе документ больше не сканировался бы до смены окна.
            let scan_key = match &request {
                Request::Scan { key, .. } => Some(key.clone()),
                _ => None,
            };
            if catch_unwind(AssertUnwindSafe(|| self.handle(request))).is_err() {
                log_error(&self.shared, "LOCATOR_PANIC");
                if let Some(key) = scan_key {
                    let _ = self.results.send(ScanResult { key, segs: Vec::new(), cost_s: 0.0 });
                    self.wake.signal();
                }
            }
        }
        // Объекты COM (непрочитанные заявки, свои ссылки) отпускаются до CoUninitialize.
        drop(inbox);
        drop(self);
        if com {
            // SAFETY: парный вызов к успешному CoInitializeEx этого потока.
            unsafe { CoUninitialize() };
        }
    }

    fn handle(&self, request: Request) {
        match request {
            Request::Scan { key, element } => {
                let started = monotonic();
                let segs = match self.uia.segments(&element, SEGMENT_LIMIT) {
                    Ok(segs) => segs,
                    Err(()) => {
                        log_error(&self.shared, "SCAN");
                        Vec::new()
                    }
                };
                let cost_s = monotonic() - started;
                if self.results.send(ScanResult { key, segs, cost_s }).is_ok() {
                    self.wake.signal();
                }
            }
            Request::Status(items) => self.events.send(UiEvent::Statuses(self.statuses(&items))),
            Request::Goto(item) => self.goto(&item),
        }
    }

    fn toast(&self, text: String) {
        self.events.send(UiEvent::Toast { text, at: None, important: true });
    }

    fn closed(&self, id: &str) {
        self.events.send(UiEvent::Statuses(vec![(id.to_string(), Status::Closed)]));
    }

    /// (id, "" | закрыто) для панели: у программы нет окна, или (браузеры) нет вкладки с названием
    /// страницы, или (закладки по тексту экрана) нет окна с заголовком документа.
    fn statuses(&self, items: &[Bookmark]) -> Vec<(String, Status)> {
        // Группы по exe в порядке первого появления (как dict в Python).
        let mut groups: Vec<(&str, Vec<&Bookmark>)> = Vec::new();
        for item in items {
            match groups.iter_mut().find(|(exe, _)| *exe == item.exe) {
                Some((_, group)) => group.push(item),
                None => groups.push((&item.exe, vec![item])),
            }
        }
        let mut result = Vec::with_capacity(items.len());
        for (exe, group) in groups {
            let windows = self.api.windows_of(exe);
            let titles: Vec<String> = windows.iter().map(|&h| self.api.title(h)).collect();
            let tab_names: Option<Vec<String>> = (!windows.is_empty() && BROWSERS.contains(&exe)).then(|| {
                windows
                    .iter()
                    .flat_map(|&h| self.uia.tabs(h))
                    .filter_map(|tab| self.uia.current_name(&tab))
                    .map(|name| title_key(&name))
                    .collect()
            });
            for item in group {
                let doc = title_key(&item.doc);
                let status = if windows.is_empty() {
                    Status::Closed
                } else if tab_names
                    .as_ref()
                    .is_some_and(|names| !doc.is_empty() && !names.iter().any(|name| name.contains(&doc)))
                {
                    Status::Closed
                } else if item.mode == Mode::Ocr && !titles.iter().any(|t| same_title(&item.title, t)) {
                    Status::Closed
                } else {
                    Status::Shown
                };
                result.push((item.id.clone(), status));
            }
        }
        result
    }

    /// Поднять окно закладки (развёрнутым), её вкладку браузера или чат, найти текст,
    /// прокрутить к нему и попросить tracker мигнуть меткой.
    fn goto(&self, item: &Bookmark) {
        let windows = self.api.windows_of(&item.exe);
        if windows.is_empty() {
            self.closed(&item.id);
            return self.toast("Закрыто: окно с этой закладкой не открыто".to_string());
        }
        let (doc, title) = (item.doc.as_str(), item.title.as_str());
        let doc_key = title_key(doc);
        let mut chosen: Option<Hwnd> = None;
        let mut tab: Option<Element> = None;
        if !doc.is_empty() {
            for &hwnd in &windows {
                tab = self.uia.tabs(hwnd).into_iter().find(|t| {
                    self.uia.current_name(t).is_some_and(|name| title_key(&name).contains(&doc_key))
                });
                if tab.is_some() {
                    chosen = Some(hwnd);
                    break;
                }
            }
        }
        let chosen = match chosen {
            Some(hwnd) => hwnd,
            None => {
                if BROWSERS.contains(&item.exe.as_str()) && !doc.is_empty() {
                    self.closed(&item.id);
                    return self.toast(format!("Закрыто: вкладка «{}» не открыта", head(doc, 40)));
                }
                self.closest_window(&windows, title, doc)
            }
        };
        self.api.bring_up(chosen);
        if let Some(tab) = &tab {
            self.uia.select(tab);
        } else if !doc.is_empty() && item.mode != Mode::Ocr {
            self.uia.invoke_link(chosen, doc); // пункт списка чатов с точно таким именем, если есть
        }
        if item.mode == Mode::Ocr {
            self.commands.send(Command::Flash(item.id.clone()));
            return;
        }
        let pid = self.api.process(chosen).0;
        let (quote, prefix, suffix) = (to_flat(&item.quote), to_flat(&item.prefix), to_flat(&item.suffix));
        let deadline = monotonic() + GOTO_WAIT_S;
        while monotonic() < deadline {
            for (_, scope) in self.uia.shown_scopes(chosen, pid, self.api.window_rect(chosen)) {
                if !item.page.is_empty() {
                    let (url, name) = self.uia.document_info(&scope);
                    if page_key(&url, &name) != item.page {
                        continue;
                    }
                }
                // Python: исключение здесь обрывало переход без подсказки; здесь — следующий документ.
                let Ok(segs) = self.uia.segments(&scope, SEGMENT_LIMIT) else { continue };
                let flat = FlatDoc::new(&segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
                let Some(start) = best_occurrence(&flat.text, &quote, &prefix, &suffix, MIN_SCORE) else {
                    continue;
                };
                let Some(element) = flat
                    .spans(start as isize, start as isize + 1)
                    .first()
                    .and_then(|&(segment, _, _)| segs.get(segment))
                    .map(|s| s.element.clone())
                else {
                    continue;
                };
                self.uia.scroll_to(&element, self.uia.text_pattern(&scope).as_ref());
                self.commands.send(Command::Flash(item.id.clone()));
                return;
            }
            std::thread::sleep(GOTO_STEP);
        }
        if !doc.is_empty() && tab.is_none() {
            return self.toast(format!("Откройте «{}» — там закладка", head(doc, 40)));
        }
        self.toast("Текст закладки на странице не найден".to_string());
    }

    /// Окно с заголовком, больше всего похожим на заголовок закладки или название документа
    /// (`max` Python: первое из наибольших).
    fn closest_window(&self, windows: &[Hwnd], title: &str, doc: &str) -> Hwnd {
        let title: Vec<char> = title_key(title).chars().collect();
        let doc: Vec<char> = title_key(doc).chars().collect();
        let mut best: Option<(f64, Hwnd)> = None;
        for &hwnd in windows {
            let own: Vec<char> = title_key(&self.api.title(hwnd)).chars().collect();
            let score = similarity(&own, &title).max(similarity(&own, &doc));
            if best.map_or(true, |(top, _)| score > top) {
                best = Some((score, hwnd));
            }
        }
        best.map_or(windows[0], |(_, hwnd)| hwnd)
    }
}

/// `s[:n]` Python — первые `n` кодовых точек.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
