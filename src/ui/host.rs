//! Владелец состояния UI (часть 6; P 2519–2589, 2932–3132): items, statuses, paused, panel_focus,
//! tray, timers; обработка сообщений окна-владельца.
//!
//! `Host` живёт в `Rc<RefCell<…>>` UI-потока; оконная процедура владельца берёт его на время
//! одного сообщения. Чтобы ничего не приходило «внутрь» занятого `Host`:
//! - действия панели, окна цветов и окна клавиш кладутся в почтовый ящик (`Mailbox`) и
//!   разбираются по `WM_APP_DEFERRED` — обработчики частей 7 и 8 могут звать нас откуда угодно;
//! - модальные окна (`MessageBoxW`: справка, «занято», OCR, ошибка) встают в очередь и
//!   показываются, когда `Host` уже отпущен (`run_modals`): их вложенный цикл сообщений снова
//!   приходит в оконную процедуру владельца.
//!
//! Метки UI прячет сам (`pieces::hide_all`: панель, справка, сторож); кэш `Overlay` в tracker об
//! этом не знает, поэтому после — всегда `Command::Rerender` (раздел 4, сценарии 4, 7).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, IsWindowVisible, KillTimer, MessageBoxW, PostMessageW, PostQuitMessage,
    SetTimer, ShowWindow, HMENU, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MESSAGEBOX_STYLE, SW_HIDE,
    WM_CLOSE, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_QUERYENDSESSION, WM_TIMER, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::core::color::Colors;
use crate::core::hotkey::parse_hotkey;
use crate::core::text::{help_text, APP_TITLE};
use crate::store;
use crate::types::{
    from_hwnd, to_hwnd, Command, CommandSender, Config, Dir, Hwnd, ItemRow, LabelColor, OverlayWindows,
    PieceBindings, Point, Rgb, Shared, Status, UiEvent, WM_APP_EVENTS, WM_APP_TRAY,
};
use crate::ui::color_dialog::ColorDialog;
use crate::ui::gfx::{instance, register_class};
use crate::ui::hotkeys_dialog;
use crate::ui::panel::Panel;
use crate::ui::panel_model::{PanelAction, PanelRow, PanelState};
use crate::ui::pieces::{self, PieceEvent, WM_APP_PIECE};
use crate::ui::toast::Toast;
use crate::ui::tray::{self, Tray};
use crate::win::{monotonic, Api};

/// Отложенные действия из обработчиков других окон (почтовый ящик).
pub(crate) const WM_APP_DEFERRED: u32 = WM_APP_EVENTS + 1;

const OWNER_CLASS: &str = "ZakladkaOwner";

/// Таймеры окна-владельца (SetTimer, только пока нужны).
const TIMER_FOCUS_EARLY: usize = 1; // 50 мс после запуска: вернуть фокус (P 3138–3139)
const TIMER_FOCUS_LATE: usize = 2; // 400 мс
const TIMER_WATCH: usize = 3; // сторож heartbeat, 1 с — пока могут быть видны метки
const TIMER_STATUS: usize = 4; // «закрыто» каждые 2 с — пока открыта панель
const WATCH_MS: u32 = 1000;
const STATUS_MS: u32 = 2000;
/// heartbeat старше — tracker застрял в медленном окне: метки спрятать (P 2982–2984).
const STALE_MS: u64 = 3000;
/// Сколько ждать tracker при выходе (P 2998).
const STOP_WAIT: Duration = Duration::from_secs(3);

const OCR_MISSING: &str = "В этой Windows нет распознавания текста (OCR). Закладки в программах, \
которые не отдают текст напрямую (например ONLYOFFICE), ставиться не будут; остальное работает.\n\n\
Как добавить: Параметры → Время и язык → Язык → «Распознавание текста».\n\
Или в PowerShell от имени администратора:\n\
Add-WindowsCapability -Online -Name \"Language.OCR~~~ru-RU~0.0.1.0\"\n\n\
Чтобы это сообщение не появлялось, поставьте \"ocr\": false в config.json.";

/// Что пришло от панели, окна цветов или окна клавиш.
pub(crate) enum Deferred {
    Panel(PanelAction),
    Colors(Colors),
    Hotkeys(Option<(String, String)>),
}

pub(crate) type Mailbox = Rc<RefCell<VecDeque<Deferred>>>;

/// Положить в ящик и разбудить владельца.
fn post(mailbox: &Mailbox, owner: Hwnd, item: Deferred) {
    if let Ok(mut queue) = mailbox.try_borrow_mut() {
        queue.push_back(item);
    }
    // SAFETY: окно-владелец своё; ошибка (окна уже нет) не важна.
    unsafe {
        let _ = PostMessageW(to_hwnd(owner), WM_APP_DEFERRED, WPARAM(0), LPARAM(0));
    }
}

/// Что сделать после модального окна.
enum Then {
    Rerender,
    Close,
}

struct Modal {
    owner: Hwnd,
    text: String,
    flags: MESSAGEBOX_STYLE,
    then: Then,
}

/// Всё, из чего собирается `Host` (`ui::run` или тест).
pub(crate) struct Parts {
    pub shared: Arc<Shared>,
    pub api: Arc<Api>,
    pub config_path: PathBuf,
    pub owner: Hwnd,
    pub windows: OverlayWindows, // своя копия дескрипторов окон меток
    pub bindings: Arc<Mutex<PieceBindings>>,
    pub commands: CommandSender,
    pub heartbeat: Arc<AtomicU64>,
    pub join: Option<JoinHandle<()>>,
    pub events: Receiver<UiEvent>,
    pub mailbox: Mailbox,
}

pub struct Host {
    shared: Arc<Shared>,
    api: Arc<Api>,
    config_path: PathBuf,
    owner: Hwnd,
    windows: OverlayWindows,
    bindings: Arc<Mutex<PieceBindings>>,
    commands: CommandSender,
    heartbeat: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
    events: Receiver<UiEvent>,
    mailbox: Mailbox,
    toast: Toast,
    toast_is_hint: bool, // показано пояснение к флажку/плашке (его прячет уход указателя)
    tray: Tray,
    panel: Option<Panel>,
    color_dialog: Option<ColorDialog>,
    items: Vec<ItemRow>,
    undo: Option<String>,
    statuses: HashMap<String, Status>,
    paused: bool,
    panel_focus: Hwnd,
    start_foreground: Hwnd,
    greeted: bool,
    registered: [bool; 2],
    watching: bool,
    status_timer: bool,
    modals: VecDeque<Modal>,
    stopping: bool,
}

/// Строки панели: новые сверху (P 3008–3012; сортировка устойчивая, как `sorted` в Python).
pub(crate) fn panel_rows(items: &[ItemRow], statuses: &HashMap<String, Status>, color: Rgb) -> Vec<PanelRow> {
    let mut rows: Vec<&ItemRow> = items.iter().collect();
    rows.sort_by(|a, b| b.created.cmp(&a.created));
    rows.into_iter()
        .map(|row| PanelRow {
            id: row.id.clone(),
            program: row.exe.clone(),
            text: row.label.clone(),
            status: statuses.get(&row.id).copied().unwrap_or(Status::Shown),
            color,
        })
        .collect()
}

/// Состояние подвала панели (P 3014–3017).
pub(crate) fn panel_state(config: &Config, undo: &Option<String>, paused: bool) -> PanelState {
    PanelState {
        hotkey: config.hotkey_mark.clone(),
        extra_hotkey: config.hotkey_extra.clone(),
        undo: undo.clone().filter(|u| !u.is_empty()),
        marks_visible: !paused,
        hints: config.hints,
        accent: config.bar_color,
    }
}

/// Вернуть ли клавиатуру окну `previous` (P 3126–3132): оно есть и видно, активно не оно, а
/// активного нет или оно наше.
pub(crate) fn should_give_back(previous: Hwnd, current: Hwnd, previous_visible: bool, current_is_ours: bool) -> bool {
    previous != 0 && current != previous && previous_visible && (current == 0 || current_is_ours)
}

/// heartbeat (мс `monotonic`) устарел.
pub(crate) fn stale(now_ms: u64, beat_ms: u64) -> bool {
    now_ms.saturating_sub(beat_ms) > STALE_MS
}

/// Текст «занято» (P 2961–2964; пункт меню заменён на «Клавиши» в панели).
pub(crate) fn busy_text(hotkey: &str) -> String {
    format!(
        "Сочетание {hotkey} занято другой программой.\n\
         Выберите другое: щёлкните значок «Закладка» в трее → «Клавиши»."
    )
}

fn label_value(c: LabelColor) -> Value {
    match c {
        LabelColor::Auto => Value::from("auto"),
        LabelColor::Custom(rgb) => Value::from(rgb.hex()),
    }
}

impl Host {
    /// Ничего не показывает и не регистрирует (это делает `start`).
    pub(crate) fn new(p: Parts) -> Host {
        let config = read_config(&p.shared);
        let tip = tray::tip_text(p.shared.profile.tray_suffix, &config.hotkey_mark);
        let tray = Tray::new(p.owner, p.api.make_icon(32, config.bar_color), &tip);
        Host {
            toast: Toast::new(p.api.clone()),
            toast_is_hint: false,
            tray,
            shared: p.shared,
            api: p.api,
            config_path: p.config_path,
            owner: p.owner,
            windows: p.windows,
            bindings: p.bindings,
            commands: p.commands,
            heartbeat: p.heartbeat,
            join: p.join,
            events: p.events,
            mailbox: p.mailbox,
            panel: None,
            color_dialog: None,
            items: Vec::new(),
            undo: None,
            statuses: HashMap::new(),
            paused: false,
            panel_focus: 0,
            start_foreground: 0,
            greeted: false,
            registered: [false; 2],
            watching: false,
            status_timer: false,
            modals: VecDeque::new(),
            stopping: false,
        }
    }

    /// Запуск (раздел 4, сценарий 1): клавиши (занято — сообщение), значок в трее, приветствие,
    /// проверка OCR, возврат фокуса окну, активному до запуска.
    pub(crate) fn start(&mut self, previous_foreground: Hwnd) {
        self.start_foreground = previous_foreground;
        self.start_hotkeys();
        let config = self.config();
        if config.ocr && !crate::ocr::Ocr::supported() {
            self.modal(0, OCR_MISSING.to_string(), MB_ICONINFORMATION, Then::Rerender);
        }
        self.set_timer(TIMER_FOCUS_EARLY, 50);
        self.set_timer(TIMER_FOCUS_LATE, 400);
    }

    fn config(&self) -> Config {
        read_config(&self.shared)
    }

    fn send(&self, c: Command) {
        self.commands.send(c);
    }

    fn modal(&mut self, owner: Hwnd, text: String, flags: MESSAGEBOX_STYLE, then: Then) {
        self.modals.push_back(Modal { owner, text, flags, then });
    }

    fn set_timer(&self, id: usize, ms: u32) {
        // SAFETY: окно своё; повторный SetTimer с тем же номером перезапускает отсчёт.
        unsafe {
            SetTimer(to_hwnd(self.owner), id, ms, None);
        }
    }

    fn kill_timer(&self, id: usize) {
        // SAFETY: окно своё.
        unsafe {
            let _ = KillTimer(to_hwnd(self.owner), id);
        }
    }

    /// Сторож нужен, только пока метки могут быть видны.
    fn update_watch(&mut self) {
        let want = !self.stopping && !self.paused && !self.items.is_empty();
        if want != self.watching {
            if want {
                self.set_timer(TIMER_WATCH, WATCH_MS);
            } else {
                self.kill_timer(TIMER_WATCH);
            }
            self.watching = want;
        }
    }

    fn stop_status_timer(&mut self) {
        if self.status_timer {
            self.kill_timer(TIMER_STATUS);
            self.status_timer = false;
        }
    }

    /// Спрятать метки из UI (P 2724–2726); вызывающий потом шлёт `Rerender`.
    fn hide_marks(&self) {
        pieces::hide_all(&self.windows);
    }

    /// Короткое сообщение у точки (P 2737–2755). Неважные молчат при «Подсказки: выкл».
    fn show_toast(&mut self, text: &str, at: Option<Point>, seconds: f64, important: bool, small: bool) {
        if !important && !self.config().hints {
            return;
        }
        self.toast.show(text, at, seconds, small);
        self.toast_is_hint = false;
    }

    // ---- сообщения окна-владельца

    fn handle(&mut self, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
        if self.stopping {
            return None;
        }
        match msg {
            WM_APP_EVENTS => {
                self.drain_events();
                Some(0)
            }
            WM_APP_DEFERRED => {
                self.drain_mailbox();
                Some(0)
            }
            WM_APP_PIECE => {
                if let Some(event) = PieceEvent::decode(wparam, lparam) {
                    self.on_piece(event);
                }
                Some(0)
            }
            WM_APP_TRAY => {
                if tray::opens_panel(lparam) {
                    let at = self.api.cursor();
                    self.open_panel(at);
                }
                Some(0)
            }
            WM_HOTKEY => {
                if wparam == 1 || wparam == 2 {
                    let (hwnd, cursor) = (self.api.foreground(), self.api.cursor());
                    self.send(Command::Mark { hwnd, cursor, extra: wparam == 2 });
                }
                Some(0)
            }
            WM_TIMER => {
                self.on_timer(wparam);
                Some(0)
            }
            WM_CLOSE => {
                self.close();
                Some(0)
            }
            WM_ENDSESSION => {
                if wparam != 0 {
                    self.close();
                }
                Some(0)
            }
            m if m != 0 && m == self.tray.taskbar_created() => {
                self.add_tray(); // Проводник перезапущен: значок заново (P 2580–2581, 2973–2974)
                Some(0)
            }
            _ => None,
        }
    }

    /// События tracker и locator (P 2932–2978).
    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                UiEvent::Toast { text, at, important } => {
                    self.show_toast(&text, at, if important { 2.5 } else { 1.4 }, important, false)
                }
                UiEvent::Items { rows, undo } => {
                    let ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
                    self.statuses.retain(|id, _| ids.contains(id.as_str()));
                    self.items = rows;
                    self.undo = undo.filter(|u| !u.is_empty());
                    self.update_watch();
                    self.refresh_panel();
                }
                UiEvent::Statuses(list) => {
                    self.statuses.extend(list);
                    self.refresh_panel();
                }
                UiEvent::Render(state) => {
                    if !self.paused {
                        store::log_render(&self.shared, &state);
                    }
                }
                UiEvent::Fatal(text) => {
                    self.modal(0, text, MB_ICONERROR, Then::Close);
                    return;
                }
            }
        }
    }

    fn drain_mailbox(&mut self) {
        loop {
            let next = self.mailbox.try_borrow_mut().ok().and_then(|mut q| q.pop_front());
            let Some(item) = next else { break };
            match item {
                Deferred::Panel(action) => self.on_panel_action(action),
                Deferred::Colors(colors) => self.apply_colors(colors),
                Deferred::Hotkeys(Some((mark, extra))) => self.save_hotkeys(mark, extra),
                Deferred::Hotkeys(None) => self.start_hotkeys(),
            }
            if self.stopping {
                break;
            }
        }
    }

    /// Мышь над флажком ✕ и плашкой ▲/▼ (P 2645–2654, 2658–2664, 2728–2735).
    fn on_piece(&mut self, event: PieceEvent) {
        let cursor = Some(self.api.cursor());
        match event {
            PieceEvent::FlagClick(index) => {
                let id = lock(&self.bindings).flags.get(index).cloned().flatten();
                if let Some(id) = id {
                    self.send(Command::Delete(id));
                    self.show_toast("Снята", cursor, 1.4, false, false);
                }
            }
            PieceEvent::PillClick(dir) => {
                let id = {
                    let b = lock(&self.bindings);
                    if dir == Dir::Up {
                        b.pill_up.clone()
                    } else {
                        b.pill_down.clone()
                    }
                };
                if let Some(id) = id {
                    self.send(Command::JumpTo(id));
                }
            }
            PieceEvent::FlagEnter(_) => {
                // Пояснение к самому элементу — всегда, независимо от «Подсказок».
                self.show_toast("Удалить закладку", cursor, 1.5, true, true);
                self.toast_is_hint = true;
            }
            PieceEvent::PillEnter(_) => {
                self.show_toast("Перейти к закладке", cursor, 1.5, true, true);
                self.toast_is_hint = true;
            }
            PieceEvent::Leave => {
                if self.toast_is_hint && self.toast.is_shown() {
                    self.toast.hide();
                }
                self.toast_is_hint = false;
            }
        }
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_FOCUS_EARLY | TIMER_FOCUS_LATE => {
                self.kill_timer(id);
                self.give_back_focus(self.start_foreground);
            }
            TIMER_WATCH => self.watch(),
            TIMER_STATUS => {
                if self.panel.as_ref().is_some_and(|p| p.is_open()) {
                    self.send(Command::Status); // «закрыто» следует за закрытием и открытием вкладок
                } else {
                    self.stop_status_timer();
                }
            }
            _ => {}
        }
    }

    /// Сторож (P 2980–2987): tracker молчит дольше 3 с — спрятать метки и попросить `Rerender`;
    /// вернуть подсказку и окно-владельца в их состояние.
    fn watch(&mut self) {
        let now = (monotonic() * 1000.0).max(0.0) as u64;
        if stale(now, self.heartbeat.load(Ordering::Relaxed)) && pieces::any_visible(&self.windows) {
            self.hide_marks();
            self.send(Command::Rerender);
        }
        self.toast.guard();
        // SAFETY: окно своё; его никто не должен показывать.
        unsafe {
            let owner = to_hwnd(self.owner);
            if IsWindowVisible(owner).as_bool() {
                let _ = ShowWindow(owner, SW_HIDE);
            }
        }
    }

    // ---- клавиши и трей

    fn register(&mut self, id: i32, text: &str) -> bool {
        let Ok((modifiers, vk)) = parse_hotkey(text) else { return false };
        // SAFETY: окно своё; клавиша снимается в unregister_hotkeys.
        let ok = unsafe { RegisterHotKey(to_hwnd(self.owner), id, HOT_KEY_MODIFIERS(modifiers), vk) }.is_ok();
        self.registered[(id - 1) as usize] = ok;
        ok
    }

    fn unregister_hotkeys(&mut self) {
        for (index, registered) in self.registered.iter_mut().enumerate() {
            if *registered {
                // SAFETY: клавиша зарегистрирована на своём окне.
                unsafe {
                    let _ = UnregisterHotKey(to_hwnd(self.owner), index as i32 + 1);
                }
                *registered = false;
            }
        }
    }

    /// Зарегистрировать обе клавиши (P 2476–2486, 2906–2910, 2953–2964): занятая — сообщение;
    /// значок в трее, если его ещё нет; приветствие один раз, если хоть одна клавиша работает.
    fn start_hotkeys(&mut self) {
        self.unregister_hotkeys();
        let config = self.config();
        let mut any = false;
        for (id, text) in [(1, &config.hotkey_mark), (2, &config.hotkey_extra)] {
            if self.register(id, text) {
                any = true;
            } else {
                self.modal(0, busy_text(text), MB_ICONWARNING, Then::Rerender);
            }
        }
        if !self.tray.is_added() {
            self.add_tray();
        }
        if any && !self.greeted {
            self.greeted = true;
            let text = format!("{APP_TITLE}: выделите текст и нажмите {}", config.hotkey_mark);
            self.show_toast(&text, None, 3.0, false, false);
        }
    }

    fn add_tray(&mut self) {
        if !self.tray.add() {
            store::log_error(&self.shared, "TRAY_ADD");
        }
    }

    fn open_hotkeys(&mut self) {
        if hotkeys_dialog::is_open() {
            hotkeys_dialog::raise();
            return;
        }
        // Снять свои сочетания, чтобы их можно было нажать в окне (P 2874).
        self.unregister_hotkeys();
        let config = self.config();
        let (mailbox, owner) = (self.mailbox.clone(), self.owner);
        let done: hotkeys_dialog::OnDone = Box::new(move |result| post(&mailbox, owner, Deferred::Hotkeys(result)));
        if !hotkeys_dialog::open(self.api.clone(), &config.hotkey_mark, &config.hotkey_extra, done) {
            self.start_hotkeys();
        }
    }

    /// P 2912–2919.
    fn save_hotkeys(&mut self, mark: String, extra: String) {
        self.save_config(&[("hotkey_mark", Value::from(mark.as_str())), ("hotkey_extra", Value::from(extra.as_str()))]);
        if let Ok(mut config) = self.shared.config.write() {
            config.hotkey_mark = mark.clone();
            config.hotkey_extra = extra;
        }
        self.start_hotkeys();
        self.tray.set_tip(&tray::tip_text(self.shared.profile.tray_suffix, &mark));
        self.send(Command::ConfigChanged);
        self.show_toast(&format!("Сочетания сохранены: {mark}"), None, 2.5, false, false);
    }

    /// Записать изменённые ключи, сохранив остальные (P 3103–3120).
    fn save_config(&self, changes: &[(&str, Value)]) {
        if store::save_config_keys(&self.config_path, changes).is_err() {
            store::log_error(&self.shared, "SAVE_CONFIG");
        }
    }

    // ---- панель (P 3019–3069)

    fn open_panel(&mut self, anchor: Point) {
        if self.panel.is_none() {
            let (mailbox, owner) = (self.mailbox.clone(), self.owner);
            let on_action = Box::new(move |action| post(&mailbox, owner, Deferred::Panel(action)));
            self.panel = Some(Panel::new(self.api.clone(), on_action));
        }
        let current = self.api.foreground();
        if current != 0 && self.api.process(current).0 != self.api.own_pid() {
            self.panel_focus = current;
        }
        self.hide_marks();
        let rows = self.panel_rows();
        let state = self.panel_state();
        let opened = self.panel.as_mut().is_some_and(|p| p.show(anchor, rows, state));
        if opened {
            self.send(Command::Status); // «закрыто» придёт чуть позже
            if !self.status_timer {
                self.set_timer(TIMER_STATUS, STATUS_MS);
                self.status_timer = true;
            }
        } else {
            // Повторный щелчок закрыл панель (или щелчок сразу после закрытия): метки вернуть.
            self.send(Command::Rerender);
        }
    }

    fn panel_rows(&self) -> Vec<PanelRow> {
        panel_rows(&self.items, &self.statuses, self.config().color)
    }

    fn panel_state(&self) -> PanelState {
        panel_state(&self.config(), &self.undo, self.paused)
    }

    fn refresh_panel(&mut self) {
        let open = self.panel.as_ref().is_some_and(|p| p.is_open());
        if open {
            let (rows, state) = (self.panel_rows(), self.panel_state());
            if let Some(panel) = self.panel.as_mut() {
                panel.update(rows, state);
            }
        }
    }

    /// Панель уже закрыта (все действия её закрывают). Метки были спрятаны при открытии — после
    /// любого действия `Rerender` (в Python их возвращал сам следующий тик, в Rust кэш `Overlay`
    /// о прятании не знает).
    fn on_panel_action(&mut self, action: PanelAction) {
        self.stop_status_timer();
        match action {
            PanelAction::Go(id) => {
                self.panel_focus = 0;
                self.send(Command::Goto(id));
                self.send(Command::Rerender);
            }
            PanelAction::Delete(id) => {
                self.send(Command::Delete(id));
                self.send(Command::Rerender);
            }
            PanelAction::Undo => {
                self.panel_focus = 0;
                self.send(Command::Undo);
                self.send(Command::Rerender);
            }
            PanelAction::DeleteAll => {
                self.send(Command::DeleteAll);
                self.send(Command::Rerender);
            }
            PanelAction::ToggleMarks => self.toggle_marks(),
            PanelAction::ToggleHints => {
                self.toggle_hints();
                self.send(Command::Rerender);
            }
            PanelAction::Hotkeys => {
                self.panel_focus = 0;
                self.send(Command::Rerender);
                self.open_hotkeys();
            }
            PanelAction::Colors => {
                self.panel_focus = 0;
                self.send(Command::Rerender);
                self.open_colors();
            }
            PanelAction::Help => {
                self.panel_focus = 0;
                self.send(Command::Rerender);
                self.show_help();
            }
            PanelAction::Exit => self.close(),
            PanelAction::Closed => {
                // Закрыта без действия: клавиатуру — окну, где был пользователь (P 3052–3057).
                let previous = std::mem::take(&mut self.panel_focus);
                if previous != 0 {
                    self.give_back_focus(previous);
                }
                self.send(Command::Rerender);
            }
        }
    }

    fn toggle_marks(&mut self) {
        self.paused = !self.paused;
        self.send(Command::Pause(self.paused));
        if self.paused {
            self.hide_marks();
        } else {
            self.send(Command::Rerender);
        }
        self.update_watch();
        self.refresh_panel();
    }

    fn toggle_hints(&mut self) {
        let hints = {
            let Ok(mut config) = self.shared.config.write() else { return };
            config.hints = !config.hints;
            config.hints
        };
        self.save_config(&[("hints", Value::from(hints))]);
        self.refresh_panel();
    }

    fn show_help(&mut self) {
        let c = self.config();
        let text = help_text(&c.hotkey_mark, &c.hotkey_extra, c.max_age_days);
        self.modal(self.owner, text, MB_ICONINFORMATION, Then::Rerender);
    }

    fn give_back_focus(&self, previous: Hwnd) {
        let current = self.api.foreground();
        let ours = current != 0 && self.api.process(current).0 == self.api.own_pid();
        if should_give_back(previous, current, self.api.is_visible(previous), ours) {
            self.api.set_foreground(previous);
        }
    }

    // ---- цвета (P 3076–3101)

    fn open_colors(&mut self) {
        if self.color_dialog.as_ref().is_some_and(|d| d.is_open()) {
            return;
        }
        let c = self.config();
        let current = Colors { color: c.color, opacity: c.opacity, bar_color: c.bar_color, label_color: c.label_color };
        let (mailbox, owner) = (self.mailbox.clone(), self.owner);
        let on_save = Box::new(move |colors| post(&mailbox, owner, Deferred::Colors(colors)));
        self.color_dialog = Some(ColorDialog::open(self.api.clone(), current, on_save));
    }

    /// Новые цвета: config.json, `Shared.config`, окна меток, значок трея, tracker.
    fn apply_colors(&mut self, c: Colors) {
        if let Ok(mut config) = self.shared.config.write() {
            config.color = c.color;
            config.opacity = c.opacity;
            config.bar_color = c.bar_color;
            config.label_color = c.label_color;
        }
        self.save_config(&[
            ("color", Value::from(c.color.hex())),
            ("opacity", Value::from(c.opacity)),
            ("bar_color", Value::from(c.bar_color.hex())),
            ("label_color", label_value(c.label_color)),
        ]);
        pieces::recolor(&self.shared, &self.windows);
        self.tray.set_icon(self.api.make_icon(32, c.bar_color));
        self.send(Command::ConfigChanged);
        self.send(Command::Rerender);
    }

    // ---- выход (P 2992–3005)

    /// Остановить tracker (до 3 с), убрать значок, клавиши, окна. Повторный вызов ничего не делает.
    pub(crate) fn close(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        for id in [TIMER_FOCUS_EARLY, TIMER_FOCUS_LATE, TIMER_WATCH, TIMER_STATUS] {
            self.kill_timer(id);
        }
        self.watching = false;
        self.status_timer = false;
        self.modals.clear();
        self.unregister_hotkeys();
        if let Some(panel) = self.panel.as_mut() {
            panel.hide();
        }
        if let Some(dialog) = self.color_dialog.as_mut() {
            if dialog.is_open() {
                dialog.close();
            }
        }
        hotkeys_dialog::close();
        self.toast.hide();
        self.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let deadline = Instant::now() + STOP_WAIT;
            while !join.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if join.is_finished() {
                let _ = join.join();
            }
        }
        self.tray.remove();
        pieces::destroy(&self.windows);
        // SAFETY: окно своё; WM_DESTROY владельца ставит WM_QUIT.
        unsafe {
            let _ = DestroyWindow(to_hwnd(self.owner));
        }
    }
}

fn read_config(shared: &Shared) -> Config {
    match shared.config.read() {
        Ok(config) => config.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn lock(bindings: &Mutex<PieceBindings>) -> std::sync::MutexGuard<'_, PieceBindings> {
    bindings.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---- окно-владелец и связь с ним

thread_local! {
    static HOST: RefCell<Option<Rc<RefCell<Host>>>> = const { RefCell::new(None) };
    static OWNER: Cell<Hwnd> = const { Cell::new(0) };
    static IN_MODAL: Cell<bool> = const { Cell::new(false) };
}

/// Скрытое окно-владелец: не `HWND_MESSAGE` — такие окна не получают рассылку `TaskbarCreated`.
pub(crate) fn create_owner() -> Option<Hwnd> {
    let atom = register_class(OWNER_CLASS, owner_proc, false);
    if atom == 0 {
        return None;
    }
    // SAFETY: класс зарегистрирован; окно никогда не показывается.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(atom as usize as *const u16),
            w!("Закладка"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND(null_mut()),
            HMENU(null_mut()),
            instance(),
            None,
        )
    }
    .ok()?;
    (!window.0.is_null()).then(|| from_hwnd(window))
}

/// Сделать `host` хозяином окна-владельца этого потока.
pub(crate) fn install(host: Rc<RefCell<Host>>) {
    let owner = host.try_borrow().map_or(0, |h| h.owner);
    let _ = OWNER.try_with(|o| o.set(owner));
    let _ = HOST.try_with(|h| *h.borrow_mut() = Some(host));
}

pub(crate) fn uninstall() {
    let _ = HOST.try_with(|h| h.borrow_mut().take());
    let _ = OWNER.try_with(|o| o.set(0));
}

fn current() -> Option<Rc<RefCell<Host>>> {
    HOST.try_with(|h| h.try_borrow().ok().and_then(|h| h.clone())).ok().flatten()
}

/// Показать очередь модальных окон, когда `Host` свободен; вложенно — никогда (окна по одному).
pub(crate) fn run_modals(rc: &Rc<RefCell<Host>>) {
    if IN_MODAL.try_with(|m| m.get()).unwrap_or(true) {
        return;
    }
    loop {
        let modal = {
            let Ok(mut host) = rc.try_borrow_mut() else { return };
            if host.stopping {
                host.modals.clear();
                return;
            }
            let modal = host.modals.pop_front();
            if modal.is_some() {
                host.hide_marks(); // метки не следуют за текстом, пока окно модальное (P 2763–2769)
            }
            modal
        };
        let Some(modal) = modal else { return };
        let _ = IN_MODAL.try_with(|m| m.set(true));
        let text = HSTRING::from(modal.text.as_str());
        // SAFETY: строки живут до конца вызова; Host сейчас не занят.
        unsafe {
            MessageBoxW(to_hwnd(modal.owner), &text, &HSTRING::from(APP_TITLE), modal.flags);
        }
        let _ = IN_MODAL.try_with(|m| m.set(false));
        let Ok(mut host) = rc.try_borrow_mut() else { return };
        match modal.then {
            Then::Rerender => host.send(Command::Rerender),
            Then::Close => host.close(),
        }
    }
}

fn owner_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    match msg {
        WM_DESTROY => {
            if OWNER.try_with(|o| o.get()).unwrap_or(0) == hwnd {
                // SAFETY: выход из цикла сообщений своего потока.
                unsafe { PostQuitMessage(0) };
            }
            return Some(0);
        }
        WM_QUERYENDSESSION => return Some(1),
        _ => {}
    }
    let rc = current()?;
    let result = {
        let Ok(mut host) = rc.try_borrow_mut() else { return None };
        if host.owner != hwnd {
            return None;
        }
        host.handle(msg, wparam, lparam)
    };
    run_modals(&rc);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, IsWindow, PeekMessageW, SetWindowPos, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };

    use crate::types::{EventSink, Kind};
    use crate::ui::pieces::tests::test_shared;
    use crate::win::Wake;

    fn row(id: &str, created: &str) -> ItemRow {
        ItemRow { id: id.into(), exe: "claude.exe".into(), label: format!("текст {id}"), created: created.into(), kind: Kind::Pos }
    }

    #[test]
    fn rows_newest_first_with_statuses() {
        let items = vec![
            row("a", "2026-11-19T10:00:00"),
            row("b", "2026-11-20T09:00:00"),
            row("c", "2026-11-19T10:00:00"),
        ];
        let statuses = HashMap::from([("a".to_string(), Status::Closed), ("b".to_string(), Status::NotFound)]);
        let rows = panel_rows(&items, &statuses, Rgb(1, 2, 3));
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]); // равные — в прежнем порядке
        assert_eq!(rows[0].status, Status::NotFound);
        assert_eq!(rows[1].status, Status::Closed);
        assert_eq!(rows[2].status, Status::Shown);
        assert!(rows.iter().all(|r| r.color == Rgb(1, 2, 3) && r.program == "claude.exe"));
        assert_eq!(rows[1].text, "текст a");
    }

    #[test]
    fn state_follows_config() {
        let mut config = store::default_config(&crate::app_id::DEV);
        config.hints = false;
        let s = panel_state(&config, &Some("старая".into()), true);
        assert_eq!(
            (s.hotkey.as_str(), s.extra_hotkey.as_str(), s.undo.as_deref(), s.marks_visible, s.hints, s.accent),
            ("Ctrl+Alt+X", "Ctrl+Alt+Shift+X", Some("старая"), false, false, Rgb(0x34, 0xC7, 0x59))
        );
        assert_eq!(panel_state(&config, &Some(String::new()), false).undo, None);
    }

    #[test]
    fn focus_and_watchdog_rules() {
        assert!(should_give_back(10, 0, true, false)); // активного нет
        assert!(should_give_back(10, 20, true, true)); // активно наше окно
        assert!(!should_give_back(10, 20, true, false)); // пользователь уже в другом окне
        assert!(!should_give_back(10, 10, true, true)); // уже активно
        assert!(!should_give_back(10, 0, false, false)); // окно скрыто
        assert!(!should_give_back(0, 0, true, true));
        assert!(!stale(10_000, 8_000));
        assert!(!stale(10_000, 7_000));
        assert!(stale(10_001, 7_000));
        assert!(stale(10_000, 0));
        assert!(!stale(5, 10)); // heartbeat «из будущего» не устарел
        assert!(busy_text("Ctrl+Alt+X").starts_with("Сочетание Ctrl+Alt+X занято другой программой.\n"));
    }

    fn pump() {
        let mut msg = MSG::default();
        // SAFETY: разбор сообщений своего потока.
        unsafe {
            while PeekMessageW(&mut msg, HWND(null_mut()), 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
    }

    /// Заглушка tracker: скрытое окно-владелец, без значка в трее и без глобальных клавиш.
    #[test]
    fn host_on_hidden_owner_handles_events_and_actions() {
        let shared = test_shared("zakladka-part6-host");
        let _ = std::fs::create_dir_all(&shared.data_dir);
        let config_path = shared.data_dir.join("config.json");
        let _ = std::fs::remove_file(&config_path);
        let owner = create_owner().expect("окно-владелец");
        pieces::set_owner(owner);
        let windows = pieces::create_overlay_windows(&shared, 1.0);
        let kept = pieces::copy(&windows);
        let bindings = Arc::new(Mutex::new(PieceBindings::default()));
        let (commands, received) = CommandSender::channel(Arc::new(Wake::new()));
        let (sink, events) = EventSink::channel(owner);
        let mailbox: Mailbox = Rc::new(RefCell::new(VecDeque::new()));
        let host = Rc::new(RefCell::new(Host::new(Parts {
            shared: shared.clone(),
            api: Arc::new(Api::new()),
            config_path: config_path.clone(),
            owner,
            windows: kept,
            bindings: bindings.clone(),
            commands,
            heartbeat: Arc::new(AtomicU64::new(0)),
            join: None,
            events,
            mailbox: mailbox.clone(),
        })));
        install(host.clone());
        let drain = |rx: &mpsc::Receiver<Command>| -> Vec<String> {
            let mut out = Vec::new();
            while let Ok(c) = rx.try_recv() {
                out.push(match c {
                    Command::Mark { extra, .. } => format!("mark:{extra}"),
                    Command::JumpTo(id) => format!("jump:{id}"),
                    Command::Delete(id) => format!("delete:{id}"),
                    Command::Pause(p) => format!("pause:{p}"),
                    Command::Rerender => "rerender".into(),
                    Command::Status => "status".into(),
                    Command::ConfigChanged => "config".into(),
                    Command::Stop => "stop".into(),
                    _ => "other".into(),
                });
            }
            out
        };

        // события tracker: строки и «закрыто» (подсказки выключены — на экран ничего не выходит)
        sink.send(UiEvent::Items { rows: vec![row("a", "2026-11-19T10:00:00"), row("b", "2026-11-20T09:00:00")], undo: Some("x".into()) });
        sink.send(UiEvent::Statuses(vec![("a".into(), Status::Closed), ("zz".into(), Status::Closed)]));
        sink.send(UiEvent::Toast { text: "Поставлена".into(), at: None, important: false });
        pump();
        {
            let h = host.borrow();
            assert_eq!(h.items.len(), 2);
            assert_eq!(h.undo.as_deref(), Some("x"));
            assert_eq!(h.statuses.get("a"), Some(&Status::Closed));
            assert!(h.watching, "сторож включён: закладки есть");
            assert!(!h.toast.is_shown(), "неважная подсказка при hints=false молчит");
        }
        sink.send(UiEvent::Items { rows: vec![row("b", "2026-11-20T09:00:00")], undo: None });
        pump();
        assert_eq!(host.borrow().statuses.get("a"), None, "статусы удалённых забыты");

        // клавиши (сообщение WM_HOTKEY без регистрации), щелчок по плашке и флажку
        lock(&bindings).pill_down = Some("b".into());
        lock(&bindings).flags[2] = Some("b".into());
        // SAFETY: сообщения своему окну.
        unsafe {
            let _ = PostMessageW(to_hwnd(owner), WM_HOTKEY, WPARAM(2), LPARAM(0));
            let (w, l) = PieceEvent::PillClick(Dir::Down).encode();
            let _ = PostMessageW(to_hwnd(owner), WM_APP_PIECE, WPARAM(w), LPARAM(l));
            let (w, l) = PieceEvent::FlagClick(2).encode();
            let _ = PostMessageW(to_hwnd(owner), WM_APP_PIECE, WPARAM(w), LPARAM(l));
            let (w, l) = PieceEvent::FlagClick(3).encode(); // флажок без закладки — ничего
            let _ = PostMessageW(to_hwnd(owner), WM_APP_PIECE, WPARAM(w), LPARAM(l));
        }
        pump();
        assert_eq!(drain(&received), ["mark:true", "jump:b", "delete:b"]);

        // действия панели через почтовый ящик
        post(&mailbox, owner, Deferred::Panel(PanelAction::Delete("b".into())));
        post(&mailbox, owner, Deferred::Panel(PanelAction::ToggleMarks));
        post(&mailbox, owner, Deferred::Panel(PanelAction::Closed));
        pump();
        assert_eq!(drain(&received), ["delete:b", "rerender", "pause:true", "rerender"]);
        assert!(host.borrow().paused && !host.borrow().watching, "на паузе сторож не нужен");
        post(&mailbox, owner, Deferred::Panel(PanelAction::ToggleMarks));
        post(&mailbox, owner, Deferred::Panel(PanelAction::ToggleHints));
        pump();
        assert_eq!(drain(&received), ["pause:false", "rerender", "rerender"]);
        assert!(read_config(&shared).hints, "подсказки включены");
        let saved = std::fs::read_to_string(&config_path).unwrap_or_default();
        assert!(saved.contains("\"hints\": true"), "{saved}");
        if let Ok(mut c) = shared.config.write() {
            c.hints = false;
        }

        // сторож: heartbeat старый, метка видна (за экраном) — спрятать и Rerender
        let first = windows.highlights[0];
        // SAFETY: своё окно показывается за экраном без активации.
        unsafe {
            let _ = SetWindowPos(to_hwnd(first), HWND(null_mut()), -32000, -32000, 1, 1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        assert!(pieces::any_visible(&windows));
        host.borrow_mut().watch();
        assert!(!pieces::any_visible(&windows));
        assert_eq!(drain(&received), ["rerender"]);
        host.borrow_mut().watch();
        assert!(drain(&received).is_empty(), "нечего прятать — ничего не шлём");

        // цвета: config.json, снимок окон меток, tracker
        let colors = Colors {
            color: Rgb(0x22, 0xC7, 0xE8),
            opacity: 0.35,
            bar_color: Rgb(0x9B, 0x6C, 0xFF),
            label_color: LabelColor::Custom(Rgb(0xFF, 0xFF, 0xFF)),
        };
        post(&mailbox, owner, Deferred::Colors(colors));
        pump();
        assert_eq!(drain(&received), ["config", "rerender"]);
        assert_eq!(read_config(&shared).bar_color, Rgb(0x9B, 0x6C, 0xFF));
        assert_eq!(pieces::colors().map(|c| c.highlight), Some(Rgb(0x22, 0xC7, 0xE8)));
        let saved = std::fs::read_to_string(&config_path).unwrap_or_default();
        assert!(saved.contains("\"bar_color\": \"#9B6CFF\"") && saved.contains("\"label_color\": \"#FFFFFF\""), "{saved}");
        assert!(saved.contains("\"opacity\": 0.35"), "{saved}");

        // выход: Stop, окна уничтожены, WM_QUIT
        host.borrow_mut().close();
        assert_eq!(drain(&received), ["stop"]);
        // SAFETY: проверка дескрипторов.
        unsafe {
            assert!(!IsWindow(to_hwnd(owner)).as_bool());
            assert!(!IsWindow(to_hwnd(first)).as_bool());
        }
        host.borrow_mut().close(); // повторно — ничего
        assert!(drain(&received).is_empty());
        pump();
        uninstall();
        drop(host);
        let _ = std::fs::remove_file(&config_path);
    }
}
