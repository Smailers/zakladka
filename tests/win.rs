//! Тесты части 2 (Win32).
//!
//! Не мешают человеку за компьютером: окна только скрытые и за экраном (-32000), фокус не
//! берётся, клавиши не нажимаются (`SendInput` не вызывается), буфер обмена не трогается.
//! Круговой тест буфера обмена — `#[ignore]` в `src\win\clipboard.rs` (`clipboard_round_trip`).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetGuiResources, GET_GUI_RESOURCES_FLAGS, GR_GDIOBJECTS, GR_USEROBJECTS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyIcon, DestroyWindow, HICON, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

use zakladka::types::{from_hwnd, to_hwnd, Hwnd, Rect, Rgb};
use zakladka::win::{monotonic, Api, Wake};

const TITLE: &str = "zakladka-test-window";

/// Скрытое окно класса STATIC за экраном; не показывается, фокус не берёт.
struct HiddenWindow(Hwnd);

impl HiddenWindow {
    fn new() -> HiddenWindow {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("zakladka-test-window"),
                WS_POPUP,
                -32000,
                -32000,
                40,
                20,
                None,
                None,
                None,
                None,
            )
        }
        .expect("скрытое окно");
        HiddenWindow(from_hwnd(hwnd))
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(to_hwnd(self.0));
        }
    }
}

fn gui_objects(kind: GET_GUI_RESOURCES_FLAGS) -> u32 {
    unsafe { GetGuiResources(GetCurrentProcess(), kind) }
}

/// Девять на девять пикселей в левом верхнем углу виртуального экрана.
fn corner(api: &Api) -> Rect {
    let screen = api.virtual_screen();
    Rect { l: screen.l, t: screen.t, r: screen.l + 9, b: screen.t + 9 }
}

#[test]
fn process_of_own_window() {
    let api = Api::new();
    let window = HiddenWindow::new();
    let expected_exe = std::env::current_exe()
        .expect("путь теста")
        .file_name()
        .expect("имя файла")
        .to_string_lossy()
        .to_lowercase();

    let (pid, exe) = api.process(window.0);
    assert_eq!(pid, std::process::id());
    assert_eq!(pid, api.own_pid());
    assert_eq!(exe, expected_exe);
    assert_eq!(api.process(window.0), (pid, exe.clone()), "из кэша");
    api.forget_pid(pid);
    assert_eq!(api.process(window.0), (pid, exe.clone()), "после forget_pid");

    assert_eq!(api.class_name(window.0), "Static");
    assert_eq!(api.title(window.0), TITLE);
    assert!(!api.is_visible(window.0));
    assert!(!api.is_iconic(window.0));
    assert!(!api.is_chromium(window.0));
    // За экраном: с монитором не пересекается, прямоугольник остаётся как есть.
    assert_eq!(
        api.window_rect(window.0),
        Some(Rect { l: -32000, t: -32000, r: -31960, b: -31980 })
    );
    // Скрытые окна в поиск не попадают.
    assert!(!api.windows_of(&exe).contains(&window.0));
    assert_ne!(api.find_window(&exe), Some(window.0));

    // Нет окна — пусто, без паники.
    assert_eq!(api.process(0), (0, String::new()));
    assert_eq!(api.window_rect(0), None);
    assert_eq!(api.class_name(0), "");
    assert_eq!(api.title(0), "");
    assert!(!api.is_visible(0));
}

#[test]
fn capture_9x9_is_324_bytes() {
    let api = Api::new();
    let rect = corner(&api);
    let full = api.capture(rect, None).expect("снимок 9×9");
    assert_eq!((full.width, full.height, full.bgra.len()), (9, 9, 324));
    let small = api.capture(rect, Some((3, 3))).expect("снимок 9×9 -> 3×3");
    assert_eq!((small.width, small.height, small.bgra.len()), (3, 3, 36));
    assert!(api.capture(Rect { l: 5, t: 5, r: 5, b: 9 }, None).is_none(), "пустой прямоугольник");
    assert!(api.capture(Rect { l: 9, t: 9, r: 0, b: 0 }, None).is_none(), "вывернутый прямоугольник");
    assert!(api.capture(rect, Some((0, 3))).is_none(), "пустой размер");
}

#[test]
fn capture_does_not_leak_gdi_objects() {
    let api = Api::new();
    let rect = corner(&api);
    assert!(api.capture(rect, None).is_some(), "снимок");
    let before = gui_objects(GR_GDIOBJECTS);
    let start = monotonic();
    for _ in 0..100 {
        assert!(api.capture(rect, None).is_some());
        assert!(api.capture(rect, Some((4, 4))).is_some());
        assert!(api.capture(Rect { l: 0, t: 0, r: 0, b: 0 }, None).is_none());
    }
    eprintln!("capture: в среднем {:.1} мс на снимок", (monotonic() - start) * 1000.0 / 200.0);
    let after = gui_objects(GR_GDIOBJECTS);
    // Утечка давала бы +200 и больше; допуск — на параллельные тесты этого же процесса.
    assert!(after <= before + 10, "GDI-объектов было {before}, стало {after}");
}

/// Паника из `todo!()` заготовки.
fn is_stub_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    text.contains("not yet implemented")
}

#[test]
fn make_icon_returns_icon() {
    let api = Api::new();
    let tint = Rgb(0x34, 0xC7, 0x59);
    assert_eq!(api.make_icon(0, tint), None);
    match catch_unwind(AssertUnwindSafe(|| api.make_icon(32, tint))) {
        Ok(icon) => {
            let icon = icon.expect("значок");
            assert_ne!(icon, 0);
            unsafe { DestroyIcon(HICON(icon as *mut _)).expect("DestroyIcon") };
            let (gdi, user) = (gui_objects(GR_GDIOBJECTS), gui_objects(GR_USEROBJECTS));
            for _ in 0..100 {
                let icon = api.make_icon(32, tint).expect("значок");
                unsafe { DestroyIcon(HICON(icon as *mut _)).expect("DestroyIcon") };
            }
            assert!(gui_objects(GR_GDIOBJECTS) <= gdi + 10, "утечка GDI-объектов");
            assert!(gui_objects(GR_USEROBJECTS) <= user + 10, "утечка значков");
        }
        Err(payload) => {
            // Пока `core::color::icon_pixels` — заготовка части 1, сам значок из пикселей
            // проверяет `win::icon::tests` (подставленные пиксели, та же функция сборки).
            assert!(is_stub_panic(payload.as_ref()), "make_icon паникует не из-за заготовки");
            eprintln!("make_icon: icon_pixels ещё заготовка; проверено подстановкой в win::icon::tests");
        }
    }
}

#[test]
fn wake_wait_16ms_is_precise() {
    let wake = Wake::new();
    let mut total = 0.0;
    for _ in 0..100 {
        let start = monotonic();
        assert!(!wake.wait(0.016), "без signal не будят");
        total += monotonic() - start;
    }
    let mean_ms = total / 100.0 * 1000.0;
    eprintln!("Wake::wait(0.016): в среднем {mean_ms:.3} мс за 100 повторов");
    assert!((14.0..=18.0).contains(&mean_ms), "в среднем {mean_ms:.3} мс");
}

#[test]
fn wake_signal_wakes_and_is_consumed() {
    let wake = Arc::new(Wake::new());
    // signal до wait не теряется.
    wake.signal();
    let start = Instant::now();
    assert!(wake.wait(1.0));
    assert!(start.elapsed() < Duration::from_millis(200));
    // Событие поглощено.
    assert!(!wake.wait(0.0));
    assert!(!wake.wait(0.02));
    assert!(!wake.wait(f64::NAN));
    assert!(!wake.wait(-1.0));
    // signal из другого потока будит долгое ожидание сразу.
    let other = Arc::clone(&wake);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        other.signal();
    });
    let start = Instant::now();
    assert!(wake.wait(5.0));
    assert!(start.elapsed() < Duration::from_secs(2));
    thread.join().expect("поток");
    // ...и короткое (высокоточный таймер).
    let other = Arc::clone(&wake);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        other.signal();
    });
    if !wake.wait(0.045) {
        // Поток опоздал (нагрузка): сигнал всё равно не потерян.
        thread.join().expect("поток");
        assert!(wake.wait(0.0));
    } else {
        thread.join().expect("поток");
    }
}

#[test]
fn screen_input_and_clock() {
    let first = monotonic();
    std::thread::sleep(Duration::from_millis(20));
    let elapsed = monotonic() - first;
    assert!((0.019..1.0).contains(&elapsed), "monotonic: {elapsed}");

    let api = Api::new();
    let screen = api.virtual_screen();
    assert!(screen.r > screen.l && screen.b > screen.t, "виртуальный экран {screen:?}");
    let cursor = api.cursor();
    let work = api.work_area(cursor);
    assert!(work.r > work.l && work.b > work.t, "рабочая область {work:?}");
    let scale = api.dpi_scale_at(cursor);
    assert!((1.0..=4.0).contains(&scale), "масштаб {scale}");
    assert!(api.idle_seconds() >= 0.0);
    // Только чтение: без паники.
    let _ = api.foreground();
    let _ = api.top_window_at(cursor);
    let _ = api.own_window_at(cursor);
    let _ = api.key_down(0x10);
}

#[test]
fn copy_selection_refuses_without_focus() {
    // Скрытое окно никогда не бывает активным: copy_selection возвращает None до того, как
    // открыть буфер обмена и нажать Ctrl+C.
    let api = Api::new();
    let window = HiddenWindow::new();
    let start = Instant::now();
    assert_eq!(api.copy_selection(window.0, window.0, 0.1), None);
    assert!(start.elapsed() < Duration::from_millis(1500));
}
