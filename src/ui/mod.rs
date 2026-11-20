//! UI-поток (часть 6): окно-владелец, трей, клавиши, окна меток, подсказка; панель — часть 7,
//! окно цветов и пипетка — часть 8. UIA здесь не вызывается никогда.

pub mod host; // часть 6
pub mod hotkeys_dialog; // часть 6
pub mod pieces; // часть 6
pub mod toast; // часть 6
pub mod tray; // часть 6

pub mod gfx; // часть 7
pub mod panel; // часть 7
pub mod panel_model; // часть 7
pub mod tooltip; // часть 7

pub mod color_dialog; // часть 8
pub mod color_model; // часть 8
pub mod eyedropper; // часть 8

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};

use crate::types::{EventSink, PieceBindings, Point, Shared};
use crate::win::Api;

/// Весь UI-поток до выхода; возвращает код выхода процесса.
///
/// Порядок (раздел 4, сценарий 1): окно-владелец и окна меток → `tracker::spawn` → клавиши
/// (занято — сообщение) → значок в трее → приветствие → проверка OCR → цикл сообщений.
pub fn run(shared: Arc<Shared>, api: Arc<Api>, config_path: PathBuf) -> i32 {
    // Окно, активное при запуске (в Python его передавал `main`): ему вернётся клавиатура.
    let previous_foreground = api.foreground();
    let Some(owner) = host::create_owner() else {
        crate::store::log_error(&shared, "OWNER_WINDOW");
        return 1;
    };
    pieces::set_owner(owner);
    // Масштаб основного монитора (как `winfo_fpixels("1i") / 96` в Python): размеры флажка и плашки.
    let scale = api.dpi_scale_at(Point::default());
    let windows = pieces::create_overlay_windows(&shared, scale);
    let kept = pieces::copy(&windows);
    let bindings = Arc::new(Mutex::new(PieceBindings::default()));
    let (sink, events) = EventSink::channel(owner);
    let tracker = crate::tracker::spawn(shared.clone(), api.clone(), sink, windows, bindings.clone(), owner);
    let mailbox: host::Mailbox = Rc::new(RefCell::new(VecDeque::new()));
    let state = Rc::new(RefCell::new(host::Host::new(host::Parts {
        shared,
        api,
        config_path,
        owner,
        windows: kept,
        bindings,
        commands: tracker.commands,
        heartbeat: tracker.heartbeat,
        join: Some(tracker.join),
        events,
        mailbox,
    })));
    host::install(state.clone());
    if let Ok(mut h) = state.try_borrow_mut() {
        h.start(previous_foreground);
    }
    host::run_modals(&state);

    let mut msg = MSG::default();
    // SAFETY: обычный цикл сообщений UI-потока; -1 (ошибка) тоже завершает цикл.
    unsafe {
        while GetMessageW(&mut msg, HWND(null_mut()), 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    host::uninstall();
    if let Ok(mut h) = state.try_borrow_mut() {
        h.close(); // если цикл кончился не через «Выход» (WM_QUIT извне); иначе ничего не делает
    }
    drop(state); // подсказка, значок, панель — их окна уничтожаются здесь, в своём потоке
    0
}
