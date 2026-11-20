//! Тесты части 4 (tracker): `Overlay::apply`, шаг ожидания, ветка `self_heal` в `apply_scan`.
//!
//! Не мешают человеку за компьютером: окна меток — свой класс, слоёные, без кнопки на панели
//! задач, не активируются и ставятся только далеко за экраном (около −21000); фокус, буфер обмена
//! и клавиши не трогаются. UIA только создаётся (для `Worker`), дерево не читается.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{mpsc, Arc, Mutex, Once, RwLock};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetLayeredWindowAttributes, GetMessageW,
    GetWindowRect, IsWindowVisible, PostThreadMessageW, RegisterClassExW, ShowWindow, TranslateMessage, MSG,
    SW_HIDE, SW_SHOWNOACTIVATE, WM_QUIT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use zakladka::app_id::DEV;
use zakladka::store::{default_config, load_bookmarks, now_local};
use zakladka::tracker::follow::{
    tick_wait, wait_left, STEP_FAST, STEP_FLASH, STEP_IDLE, STEP_REST, STEP_SCREEN_ACTIVE, STEP_SHOWN,
};
use zakladka::tracker::{item_of, LocatorHandle, Overlay, Track, Worker};
use zakladka::types::{
    from_hwnd, to_hwnd, Bookmark, CommandSender, Dir, EventSink, Hwnd, Kind, Mode, OverlayWindows, PieceBindings,
    Rect, Shared, UiEvent, BARS, FLAGS, HIGHLIGHTS,
};
use zakladka::uia::{ScopeKey, Uia};
use zakladka::win::{Api, Wake};

// ---------------------------------------------------------------- подставные окна меток

const CLASS: PCWSTR = w!("ZakladkaTrackerTestPiece");
const FLAG: i32 = 16;
const PILL: (i32, i32) = (90, 24);
/// Область просмотра далеко за экраном (левее и выше любого монитора).
const VIEW: Rect = Rect { l: -21000, t: -21000, r: -20000, b: -20000 };

unsafe extern "system" fn piece_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn instance() -> HINSTANCE {
    // SAFETY: дескриптор своего модуля, без побочных действий.
    unsafe { GetModuleHandleW(None) }.map(|module| HINSTANCE(module.0)).unwrap_or_default()
}

/// Класс окон один на процесс; там же DPI (координаты — физические пиксели, без виртуализации).
fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        Api::enable_dpi();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(piece_proc),
            hInstance: instance(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // SAFETY: структура заполнена, имя класса — статическая строка.
        unsafe { RegisterClassExW(&class) };
    });
}

/// Скрытое слоёное окно своего класса за экраном (как окна меток части 6, но без рисования).
fn piece() -> Hwnd {
    register_class();
    // SAFETY: класс зарегистрирован; окно не показывается.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST,
            CLASS,
            w!(""),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            None,
            None,
            instance(),
            None,
        )
    }
    .expect("окно метки");
    from_hwnd(hwnd)
}

/// 24 подсветки, 6 полос, 6 флажков, 2 плашки; уничтожаются в Drop (в своём потоке).
struct Pieces {
    highlights: Vec<Hwnd>,
    bars: Vec<Hwnd>,
    flags: Vec<Hwnd>,
    pill_up: Hwnd,
    pill_down: Hwnd,
}

impl Pieces {
    fn new() -> Pieces {
        Pieces {
            highlights: (0..HIGHLIGHTS).map(|_| piece()).collect(),
            bars: (0..BARS).map(|_| piece()).collect(),
            flags: (0..FLAGS).map(|_| piece()).collect(),
            pill_up: piece(),
            pill_down: piece(),
        }
    }

    fn windows(&self) -> OverlayWindows {
        OverlayWindows {
            highlights: self.highlights.clone(),
            bars: self.bars.clone(),
            flags: self.flags.clone(),
            pill_up: self.pill_up,
            pill_down: self.pill_down,
            pill_size: PILL,
            flag_size: FLAG,
        }
    }

    fn all(&self) -> Vec<Hwnd> {
        let mut all = self.highlights.clone();
        all.extend(&self.bars);
        all.extend(&self.flags);
        all.push(self.pill_up);
        all.push(self.pill_down);
        all
    }
}

impl Drop for Pieces {
    fn drop(&mut self) {
        for hwnd in self.all() {
            // SAFETY: свои окна, уничтожаются потоком, который их создал.
            unsafe {
                let _ = DestroyWindow(to_hwnd(hwnd));
            }
        }
    }
}

fn rect_of(hwnd: Hwnd) -> Rect {
    let mut rect = RECT::default();
    // SAFETY: запись в свою структуру.
    unsafe { GetWindowRect(to_hwnd(hwnd), &mut rect) }.expect("GetWindowRect");
    rect.into()
}

fn visible(hwnd: Hwnd) -> bool {
    // SAFETY: только чтение состояния своего окна.
    unsafe { IsWindowVisible(to_hwnd(hwnd)) }.as_bool()
}

fn alpha_of(hwnd: Hwnd) -> u8 {
    let mut alpha = 0u8;
    // SAFETY: запись в свою переменную.
    unsafe { GetLayeredWindowAttributes(to_hwnd(hwnd), None, Some(&mut alpha), None) }.expect("альфа");
    alpha
}

fn show_quietly(hwnd: Hwnd, shown: bool) {
    // SAFETY: своё окно за экраном; без активации.
    unsafe {
        let _ = ShowWindow(to_hwnd(hwnd), if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

fn r(l: i32, t: i32, r: i32, b: i32) -> Rect {
    Rect { l, t, r, b }
}

fn bindings_snapshot(b: &Mutex<PieceBindings>) -> (Vec<Option<String>>, Option<String>, Option<String>) {
    let b = b.lock().unwrap();
    (b.flags.to_vec(), b.pill_up.clone(), b.pill_down.clone())
}

// ---------------------------------------------------------------- Overlay::apply

#[test]
fn overlay_places_lines_bars_flags_and_pills() {
    let pieces = Pieces::new();
    let bindings = Arc::new(Mutex::new(PieceBindings::default()));
    let mut overlay = Overlay::new(pieces.windows(), bindings.clone(), 0.35);
    let a = vec![r(-20900, -20800, -20700, -20780), r(-20900, -20770, -20750, -20750)];
    let b = vec![r(-20998, -20995, -20900, -20980)]; // у левого верхнего угла области просмотра
    let marks = vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())];
    let pills = vec![(Dir::Up, "c".to_string()), (Dir::Down, "d".to_string())];
    let now = 100.0;
    overlay.flash("b", now + 0.75);
    assert!(overlay.flashing(now));
    overlay.apply(&marks, &pills, Some(VIEW), now);

    // строки
    assert_eq!(rect_of(pieces.highlights[0]), a[0]);
    assert_eq!(rect_of(pieces.highlights[1]), a[1]);
    assert_eq!(rect_of(pieces.highlights[2]), b[0]);
    // полоса: x = max(view.l, left - 7), ширина 4, от верха первой до низа последней строки
    assert_eq!(rect_of(pieces.bars[0]), r(-20907, -20800, -20903, -20750));
    assert_eq!(rect_of(pieces.bars[1]), r(-21000, -20995, -20996, -20980));
    // флажок: (bar_x + 2 - size/2, top - size + 4), не левее и не выше области просмотра
    assert_eq!(rect_of(pieces.flags[0]), r(-20913, -20812, -20897, -20796));
    assert_eq!(rect_of(pieces.flags[1]), r(-21000, -21000, -20984, -20984));
    // плашки: x = view.r - w - 28; вверх y = view.t + 90; вниз y = view.b - h - 170
    assert_eq!(rect_of(pieces.pill_up), r(-20118, -20910, -20028, -20886));
    assert_eq!(rect_of(pieces.pill_down), r(-20118, -20194, -20028, -20170));

    for (index, &hwnd) in pieces.highlights.iter().enumerate() {
        assert_eq!(visible(hwnd), index < 3, "подсветка {index}");
    }
    for index in 0..BARS {
        assert_eq!(visible(pieces.bars[index]), index < 2, "полоса {index}");
        assert_eq!(visible(pieces.flags[index]), index < 2, "флажок {index}");
    }
    assert!(visible(pieces.pill_up) && visible(pieces.pill_down));

    // альфа: подсветка = opacity (0,35 -> 89), мигающая закладка в «вспышке» — 0,75 (191)
    assert_eq!(alpha_of(pieces.highlights[0]), 89);
    assert_eq!(alpha_of(pieces.highlights[2]), 191);
    // через 0,12 с — вторая половина периода: обычная альфа
    overlay.apply(&marks, &pills, Some(VIEW), now + 0.12);
    assert_eq!(alpha_of(pieces.highlights[2]), 89);
    assert!(!overlay.flashing(now + 1.0));

    // привязки флажков и плашек
    let (flags, up, down) = bindings_snapshot(&bindings);
    assert_eq!(flags[0].as_deref(), Some("a"));
    assert_eq!(flags[1].as_deref(), Some("b"));
    assert!(flags[2..].iter().all(Option::is_none));
    assert_eq!(up.as_deref(), Some("c"));
    assert_eq!(down.as_deref(), Some("d"));

    // без области просмотра плашки не ставятся; строка нулевой ширины — окно шириной 1
    overlay.apply(&[("z".to_string(), vec![r(-20500, -20500, -20500, -20490)])], &pills, None, now);
    assert_eq!(rect_of(pieces.highlights[0]), r(-20500, -20500, -20499, -20490));
    assert_eq!(rect_of(pieces.bars[0]), r(-20507, -20500, -20503, -20490));
    assert!(!visible(pieces.pill_up) && !visible(pieces.pill_down));
    assert!(!visible(pieces.highlights[1]) && !visible(pieces.bars[1]) && !visible(pieces.flags[1]));

    // пусто — всё спрятано; привязки остаются (спрятанный флажок нажать нельзя), как в Python
    overlay.apply(&[], &[], None, now);
    assert!(pieces.all().into_iter().all(|hwnd| !visible(hwnd)));
    assert_eq!(bindings_snapshot(&bindings).0[0].as_deref(), Some("z"));
}

#[test]
fn overlay_limits_24_lines_and_6_marks() {
    let pieces = Pieces::new();
    let bindings = Arc::new(Mutex::new(PieceBindings::default()));
    let mut overlay = Overlay::new(pieces.windows(), bindings.clone(), 0.3);
    let line = |n: i32| r(-20900, -20990 + n * 20, -20800, -20980 + n * 20);

    // 7 закладок по 5 строк: 24 строки кончаются на пятой закладке (5+5+5+5+4)
    let marks: Vec<(String, Vec<Rect>)> =
        (0..7).map(|m| (format!("m{m}"), (0..5).map(|k| line(m * 5 + k)).collect())).collect();
    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    assert!(pieces.highlights.iter().all(|&hwnd| visible(hwnd)));
    assert_eq!(rect_of(pieces.highlights[23]), line(4 * 5 + 3));
    for index in 0..BARS {
        assert_eq!(visible(pieces.bars[index]), index < 5, "полоса {index}");
        assert_eq!(visible(pieces.flags[index]), index < 5, "флажок {index}");
    }
    // полоса пятой закладки — по её четырём поставленным строкам
    assert_eq!(rect_of(pieces.bars[4]), r(-20907, line(20).t, -20903, line(23).b));
    let (flags, _, _) = bindings_snapshot(&bindings);
    for m in 0..5 {
        assert_eq!(flags[m].as_deref(), Some(format!("m{m}").as_str()));
    }
    assert!(flags[5].is_none());

    // 7 закладок по одной строке: закладок не больше 6
    let marks: Vec<(String, Vec<Rect>)> = (0..7).map(|m| (format!("m{m}"), vec![line(m)])).collect();
    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    for (index, &hwnd) in pieces.highlights.iter().enumerate() {
        assert_eq!(visible(hwnd), index < 6, "подсветка {index}");
    }
    assert!(pieces.bars.iter().chain(&pieces.flags).all(|&hwnd| visible(hwnd)));
    let (flags, _, _) = bindings_snapshot(&bindings);
    assert_eq!(flags[5].as_deref(), Some("m5"));
    assert!(flags.iter().all(|id| id.as_deref() != Some("m6")));
}

#[test]
fn overlay_forget_shown_guard_and_hide_all() {
    let pieces = Pieces::new();
    let bindings = Arc::new(Mutex::new(PieceBindings::default()));
    let mut overlay = Overlay::new(pieces.windows(), bindings, 0.3);
    let marks = vec![("a".to_string(), vec![r(-20900, -20900, -20800, -20880)])];
    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    assert!(visible(pieces.highlights[0]));

    // UI спрятал метки сам: без Rerender окна считаются поставленными и не трогаются
    show_quietly(pieces.highlights[0], false);
    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    assert!(!visible(pieces.highlights[0]));
    // Rerender -> forget_shown: следующий apply ставит заново
    overlay.forget_shown();
    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    assert!(visible(pieces.highlights[0]));

    // чужая программа показала спрятанное окно — guard прячет его снова
    overlay.apply(&[], &[], None, 0.0);
    assert!(!visible(pieces.highlights[0]));
    show_quietly(pieces.highlights[0], true);
    assert!(visible(pieces.highlights[0]));
    overlay.guard();
    assert!(!visible(pieces.highlights[0]));

    overlay.apply(&marks, &[], Some(VIEW), 0.0);
    assert!(visible(pieces.bars[0]));
    overlay.hide_all();
    assert!(pieces.all().into_iter().all(|hwnd| !visible(hwnd)));
}

/// Как в программе: окна принадлежат другому потоку (UI) с циклом сообщений; `SWP_ASYNCWINDOWPOS`
/// не ждёт его — положение применяется, когда тот поток разберёт очередь.
#[test]
fn overlay_moves_windows_of_another_thread() {
    let (sender, receiver) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        let pieces = Pieces::new();
        // SAFETY: без аргументов.
        let thread = unsafe { GetCurrentThreadId() };
        sender.send((pieces.windows(), thread)).unwrap();
        let mut msg = MSG::default();
        // SAFETY: обычный цикл сообщений своего потока до WM_QUIT.
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        drop(pieces);
    });
    let (windows, thread) = receiver.recv().unwrap();
    let first = windows.highlights[0];
    let bar = windows.bars[0];
    let mut overlay = Overlay::new(windows, Arc::new(Mutex::new(PieceBindings::default())), 0.3);
    let line = r(-20600, -20600, -20500, -20580);
    overlay.apply(&[("a".to_string(), vec![line])], &[], Some(VIEW), 0.0);
    let deadline = Instant::now() + Duration::from_secs(3);
    while (rect_of(first) != line || !visible(bar)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(rect_of(first), line);
    assert_eq!(rect_of(bar), r(-20607, -20600, -20603, -20580));
    overlay.hide_all();
    let deadline = Instant::now() + Duration::from_secs(3);
    while visible(first) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!visible(first));
    // SAFETY: поток-владелец ждёт WM_QUIT в GetMessageW.
    unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) }.unwrap();
    owner.join().unwrap();
}

// ---------------------------------------------------------------- шаг ожидания

#[test]
fn tick_wait_follows_motion_input_and_visibility() {
    let now = 50.0;
    let calm_hidden = [(now - 3.0, false)];
    let calm_shown = [(now - 3.0, false), (now - 3.0, true)];
    let moving = [(now - 0.1, false)];
    // ничего не найдено
    assert_eq!(tick_wait(now, 9.0, &[], false, false), STEP_REST);
    // текст движется (< 0,5 с) или пользователь работает (< 0,3 с) — каждый кадр
    assert_eq!(tick_wait(now, 9.0, &moving, false, false), STEP_FAST);
    assert_eq!(tick_wait(now, 0.1, &calm_hidden, false, false), STEP_FAST);
    assert_eq!(tick_wait(now, 9.0, &[(now - 0.49, false)], false, false), STEP_FAST);
    assert_eq!(tick_wait(now, 9.0, &[(now - 0.51, false)], false, false), STEP_REST);
    // покой: метка видна — 0,1; не видна — 0,25
    assert_eq!(tick_wait(now, 9.0, &calm_shown, false, false), STEP_SHOWN);
    assert_eq!(tick_wait(now, 0.31, &calm_hidden, false, false), STEP_REST);
    // OCR-закладки: 0,1 при простое < 2 с, иначе 1,0 (меньше 0,25 не становится)
    assert_eq!(tick_wait(now, 1.0, &[], true, false), STEP_SCREEN_ACTIVE);
    assert_eq!(tick_wait(now, 3.0, &[], true, false), STEP_REST);
    // мигание — 0,05, но не медленнее движения
    assert_eq!(tick_wait(now, 9.0, &calm_shown, false, true), STEP_FLASH);
    assert_eq!(tick_wait(now, 9.0, &moving, true, true), STEP_FAST);
    // числа плана
    assert_eq!((STEP_FAST, STEP_SHOWN, STEP_REST, STEP_IDLE, STEP_FLASH), (0.016, 0.1, 0.25, 0.3, 0.05));
    // остаток ожидания отсчитывается от конца прошлого тика
    assert!((wait_left(10.0, 0.1, 10.04) - 0.06).abs() < 1e-9);
    assert_eq!(wait_left(10.0, 0.1, 10.2), 0.0);
    assert_eq!(wait_left(0.0, STEP_IDLE, 1000.0), 0.0);
}

// ---------------------------------------------------------------- apply_scan: self_heal

fn bookmark(id: &str, kind: Kind, created: &str) -> Bookmark {
    Bookmark {
        id: id.to_string(),
        exe: "test.exe".to_string(),
        kind,
        mode: Mode::Uia,
        quote: "цитата закладки для проверки".to_string(),
        prefix: String::new(),
        suffix: String::new(),
        label: format!("подпись {id}"),
        title: String::new(),
        created: created.to_string(),
        page: String::new(),
        doc: String::new(),
        url: String::new(),
    }
}

fn track(id: &str, kind: Kind, created: &str, scope: &ScopeKey, strong: bool) -> Track {
    Track {
        id: id.to_string(),
        kind,
        created: created.to_string(),
        scope_key: scope.clone(),
        pattern: None,
        segs: Vec::new(),
        strong,
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

fn ids<'a>(list: impl Iterator<Item = &'a String>) -> HashSet<String> {
    list.cloned().collect()
}

#[test]
fn apply_scan_self_heal_drops_older_position_in_same_document() {
    // SAFETY: COM своего потока теста (MTA, как у tracker).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let uia = Arc::new(Uia::new().expect("UI Automation"));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let data_dir = std::env::temp_dir().join(format!("zakladka-part4-{}-{stamp}", std::process::id()));
    let mut config = default_config(&DEV);
    config.max_age_days = 36500; // даты закладок ниже не устаревают
    let shared = Arc::new(Shared {
        config: RwLock::new(config),
        profile: &DEV,
        data_dir: data_dir.clone(),
        debug: AtomicBool::new(false),
    });
    let (events, ui_events) = EventSink::channel(0);
    let (commands, _commands_rx) = CommandSender::channel(Arc::new(Wake::new()));
    let (requests, _requests_rx) = mpsc::channel();
    let (_results_tx, results) = mpsc::channel();
    let windows = OverlayWindows {
        highlights: Vec::new(),
        bars: Vec::new(),
        flags: Vec::new(),
        pill_up: 0,
        pill_down: 0,
        pill_size: PILL,
        flag_size: FLAG,
    };
    let overlay = Overlay::new(windows, Arc::new(Mutex::new(PieceBindings::default())), 0.3);
    let mut worker = Worker::new(
        shared,
        Arc::new(Api::new()),
        uia,
        events,
        commands,
        0,
        overlay,
        LocatorHandle { requests, results },
        Arc::new(AtomicU64::new(0)),
    );

    let scanned = ScopeKey::Doc(vec![42, 1].into_boxed_slice()); // сканируется сейчас, пустой
    let other = ScopeKey::Doc(vec![42, 2].into_boxed_slice()); // документ B
    let root = ScopeKey::Root(vec![42, 2].into_boxed_slice()); // тот же rid, но корень окна — другой ключ
    let (t1, t2, t3) = ("2026-01-01T10:00:00", "2026-01-01T11:00:00", "2026-01-01T12:00:00");
    worker.items = vec![
        bookmark("old", Kind::Pos, t1),   // B, найдена с контекстом — устарела
        bookmark("new", Kind::Pos, t3),   // B, самая новая позиция
        bookmark("gone", Kind::Pos, t1),  // была в сканируемом документе, там её больше нет
        bookmark("extra", Kind::Extra, t1), // B, дополнительная — не трогается
        bookmark("weak", Kind::Pos, t2),  // B, без своего контекста — не трогается
        bookmark("root", Kind::Pos, t1),  // корень окна — свой ключ
    ]
    .into_iter()
    .map(item_of)
    .collect();
    for (id, kind, created, scope, strong) in [
        ("old", Kind::Pos, t1, &other, true),
        ("new", Kind::Pos, t3, &other, true),
        ("gone", Kind::Pos, t1, &scanned, true),
        ("extra", Kind::Extra, t1, &other, true),
        ("weak", Kind::Pos, t2, &other, false),
        ("root", Kind::Pos, t1, &root, true),
    ] {
        worker.tracks.insert(id.to_string(), track(id, kind, created, scope, strong));
    }
    let marks: Vec<String> = worker.items.iter().map(|item| item.b.id.clone()).collect();

    worker.apply_scan(&scanned, None, Vec::new(), 0.01, &marks);

    // «gone» не найдена в своём документе — слежение снято, закладка осталась
    let expected_tracks: HashSet<String> = ["new", "extra", "weak", "root"].iter().map(|s| s.to_string()).collect();
    assert_eq!(ids(worker.tracks.keys()), expected_tracks);
    // «old» — более старая позиция в документе, где есть более новая: удалена
    let expected_items: HashSet<String> =
        ["new", "gone", "extra", "weak", "root"].iter().map(|s| s.to_string()).collect();
    assert_eq!(ids(worker.items.iter().map(|item| &item.b.id)), expected_items);
    // без «Вернуть» (это перенос позиции, а не удаление)
    assert!(worker.undo.is_empty());
    // файл записан
    let saved = load_bookmarks(&data_dir.join("bookmarks.json"), &now_local(), 36500, 300);
    assert_eq!(ids(saved.iter().map(|b| &b.id)), expected_items);
    // UI получил новый список
    let mut published = None;
    while let Ok(event) = ui_events.try_recv() {
        if let UiEvent::Items { rows, undo } = event {
            published = Some((ids(rows.iter().map(|row| &row.id)), undo));
        }
    }
    let (rows, undo) = published.expect("UiEvent::Items");
    assert_eq!(rows, expected_items);
    assert!(undo.is_none());
    // следующее сканирование: мало сегментов -> 0,25 с
    let entry = worker.flats.get(&scanned).expect("FlatEntry");
    assert!(!entry.dirty);
    assert_eq!(entry.delay, 0.25);

    // повтор: устаревших больше нет — ничего не удаляется и UI не дёргается
    worker.apply_scan(&scanned, None, Vec::new(), 0.01, &marks);
    assert_eq!(worker.items.len(), 5);
    assert!(ui_events.try_recv().is_err());

    drop(worker);
    let _ = std::fs::remove_dir_all(&data_dir);
}
