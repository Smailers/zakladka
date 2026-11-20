//! `Api`: DPI, курсор, монитор, процесс, окна, простой (P 624–671, 776–922, 980–985; N 160–280).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI, PROCESS_PER_MONITOR_DPI_AWARE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindowAsync, SwitchToThisWindow, WindowFromPoint, GA_ROOT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
};

use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect};

/// Больше записей — кэш очищается целиком (как в Python).
const EXE_CACHE_LIMIT: usize = 500;
/// Рабочая область, если монитор не нашёлся (как в Python).
const FALLBACK_WORK_AREA: Rect = Rect { l: 0, t: 0, r: 1920, b: 1080 };

/// `Arc<Api>`, Send + Sync (проверяется в lib.rs).
pub struct Api {
    own_pid: u32,
    /// pid -> имя exe в нижнем регистре ("" — не удалось узнать).
    exe_cache: Mutex<HashMap<u32, String>>,
}

impl Default for Api {
    fn default() -> Api {
        Api::new()
    }
}

impl Api {
    pub fn new() -> Api {
        Api { own_pid: std::process::id(), exe_cache: Mutex::new(HashMap::new()) }
    }

    /// PER_MONITOR_AWARE_V2, запасной shcore. Вызывается до любого окна.
    pub fn enable_dpi() {
        // SAFETY: настройка процесса без указателей; ошибка (уже задано) не страшна.
        unsafe {
            if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() {
                return;
            }
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }

    pub fn own_pid(&self) -> u32 {
        self.own_pid
    }

    pub fn cursor(&self) -> Point {
        let mut point = POINT::default();
        // SAFETY: запись в собственную структуру.
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return Point::default();
        }
        point.into()
    }

    pub fn work_area(&self, p: Point) -> Rect {
        // SAFETY: вызовы без указателей на чужую память.
        let monitor = unsafe { MonitorFromPoint(p.into(), MONITOR_DEFAULTTONEAREST) };
        match monitor_info(monitor) {
            Some(info) => info.rcWork.into(),
            None => FALLBACK_WORK_AREA,
        }
    }

    /// Весь виртуальный экран (все мониторы) как прямоугольник l, t, r, b
    /// (в Python было x, y, ширина, высота).
    pub fn virtual_screen(&self) -> Rect {
        // SAFETY: GetSystemMetrics без указателей.
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        Rect { l: x, t: y, r: x.saturating_add(w), b: y.saturating_add(h) }
    }

    /// Масштаб монитора с точкой `p` (1.0 = 96 DPI), от 1 до 4.
    pub fn dpi_scale_at(&self, p: Point) -> f64 {
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: запись в собственные переменные; монитор проверен.
        let dpi = unsafe {
            let monitor = MonitorFromPoint(p.into(), MONITOR_DEFAULTTONEAREST);
            if !monitor.is_invalid()
                && GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok()
                && x != 0
            {
                x
            } else {
                96
            }
        };
        (dpi as f64 / 96.0).clamp(1.0, 4.0)
    }

    pub fn foreground(&self) -> Hwnd {
        // SAFETY: без аргументов.
        from_hwnd(unsafe { GetForegroundWindow() })
    }

    /// (pid, exe в нижнем регистре); кэш до 500.
    pub fn process(&self, h: Hwnd) -> (u32, String) {
        let pid = window_pid(to_hwnd(h));
        if let Some(exe) = self.cache().get(&pid) {
            return (pid, exe.clone());
        }
        let exe = exe_name(pid);
        let mut cache = self.cache();
        if cache.len() > EXE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(pid, exe.clone());
        (pid, exe)
    }

    pub fn forget_pid(&self, pid: u32) {
        self.cache().remove(&pid);
    }

    /// Обрезан по монитору.
    pub fn window_rect(&self, h: Hwnd) -> Option<Rect> {
        let mut rect = RECT::default();
        // SAFETY: запись в собственную структуру; чужое окно могло исчезнуть — тогда ошибка.
        unsafe { GetWindowRect(to_hwnd(h), &mut rect) }.ok()?;
        let window = Rect::from(rect);
        // SAFETY: без указателей на чужую память.
        let monitor = unsafe { MonitorFromWindow(to_hwnd(h), MONITOR_DEFAULTTONEAREST) };
        match monitor_info(monitor) {
            // Развёрнутое окно выступает за монитор на несколько пикселей.
            Some(info) => Some(intersect(window, info.rcMonitor.into()).unwrap_or(window)),
            None => Some(window),
        }
    }

    pub fn class_name(&self, h: Hwnd) -> String {
        let mut buffer = [0u16; 256];
        // SAFETY: запись в собственный буфер, длина передаётся срезом.
        let count = unsafe { GetClassNameW(to_hwnd(h), &mut buffer) };
        wide_text(&buffer, count)
    }

    pub fn title(&self, h: Hwnd) -> String {
        let mut buffer = [0u16; 512];
        // SAFETY: запись в собственный буфер, длина передаётся срезом.
        let count = unsafe { GetWindowTextW(to_hwnd(h), &mut buffer) };
        wide_text(&buffer, count)
    }

    pub fn is_chromium(&self, h: Hwnd) -> bool {
        self.class_name(h).starts_with("Chrome_WidgetWin")
    }

    pub fn is_iconic(&self, h: Hwnd) -> bool {
        // SAFETY: только чтение состояния окна.
        unsafe { IsIconic(to_hwnd(h)) }.as_bool()
    }

    pub fn is_visible(&self, h: Hwnd) -> bool {
        // SAFETY: только чтение состояния окна.
        unsafe { IsWindowVisible(to_hwnd(h)) }.as_bool()
    }

    /// Под точкой — окно нашего процесса (метка, подсказка, панель).
    pub fn own_window_at(&self, p: Point) -> bool {
        // SAFETY: без указателей.
        let hwnd = unsafe { WindowFromPoint(p.into()) };
        !hwnd.is_invalid() && self.process(from_hwnd(hwnd)).0 == self.own_pid
    }

    /// Окно верхнего уровня, физически верхнее в точке экрана.
    pub fn top_window_at(&self, p: Point) -> Option<Hwnd> {
        // SAFETY: без указателей.
        let root = unsafe {
            let hwnd = WindowFromPoint(p.into());
            if hwnd.is_invalid() {
                return None;
            }
            GetAncestor(hwnd, GA_ROOT)
        };
        (!root.is_invalid()).then(|| from_hwnd(root))
    }

    /// Видимые окна верхнего уровня с заголовком у программы `exe` (свёрнутые тоже).
    pub fn windows_of(&self, exe: &str) -> Vec<Hwnd> {
        top_windows().into_iter().filter(|&h| self.titled_window_of(h, exe)).collect()
    }

    /// Самое большое видимое окно с заголовком у программы `exe` (selftest).
    pub fn find_window(&self, exe: &str) -> Option<Hwnd> {
        let mut best: Option<(i64, Hwnd)> = None;
        for h in top_windows() {
            if !self.titled_window_of(h, exe) {
                continue;
            }
            if let Some(rect) = self.window_rect(h) {
                let key = (area(rect), h); // как max((area, hwnd)) в Python
                if best.map_or(true, |b| key > b) {
                    best = Some(key);
                }
            }
        }
        best.map(|(_, h)| h)
    }

    /// Развернуть и активировать окно (можно: пользователь только что щёлкнул нашу панель).
    pub fn bring_up(&self, h: Hwnd) -> bool {
        let hwnd = to_hwnd(h);
        // SAFETY: вызовы с дескриптором окна; несуществующее окно даёт ошибку, не сбой.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
            if !SetForegroundWindow(hwnd).as_bool() {
                SwitchToThisWindow(hwnd, true);
            }
            GetForegroundWindow() == hwnd
        }
    }

    pub fn set_foreground(&self, h: Hwnd) -> bool {
        // SAFETY: вызов с дескриптором окна.
        unsafe { SetForegroundWindow(to_hwnd(h)) }.as_bool()
    }

    /// Секунды с последнего ввода с клавиатуры или мыши.
    pub fn idle_seconds(&self) -> f64 {
        let mut info =
            LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: запись в собственную структуру с верным cbSize.
        unsafe {
            if !GetLastInputInfo(&mut info).as_bool() {
                return 0.0;
            }
            GetTickCount().wrapping_sub(info.dwTime) as f64 / 1000.0
        }
    }

    /// GetAsyncKeyState.
    pub fn key_down(&self, vk: i32) -> bool {
        // SAFETY: без указателей.
        (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<u32, String>> {
        // Паника под замком не делает кэш неверным: продолжаем с тем, что в нём есть.
        self.exe_cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Те же три проверки, что в Python, но exe (из кэша) раньше заголовка: GetWindowTextW
    /// своему окну из другого потока шлёт WM_GETTEXT и ждёт UI-поток; чужим окнам — не шлёт.
    fn titled_window_of(&self, h: Hwnd, exe: &str) -> bool {
        self.is_visible(h) && self.process(h).1 == exe && !self.title(h).is_empty()
    }
}

/// pid процесса окна; 0 — окна нет.
pub(super) fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: запись в собственную переменную.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

/// Имя exe процесса в нижнем регистре; "" — нет доступа или процесса.
fn exe_name(pid: u32) -> String {
    // SAFETY: открытие процесса только на чтение сведений; дескриптор закрывается ниже.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return String::new();
    };
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    // SAFETY: запись в собственный буфер длины `size`; дескриптор закрывается на всех ветках.
    let ok = unsafe {
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(process);
        ok
    };
    if !ok {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buffer[..(size as usize).min(buffer.len())]);
    path.rsplit('\\').next().unwrap_or_default().to_lowercase()
}

/// Все окна верхнего уровня (в порядке EnumWindows). Проверки делаются после перечисления:
/// обратный вызов ничего не делает, кроме записи в список, и не может паниковать.
fn top_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` — указатель на список из `top_windows`, живой всё время EnumWindows.
        let list = unsafe { &mut *(data.0 as *mut Vec<Hwnd>) };
        list.push(from_hwnd(hwnd));
        BOOL(1)
    }
    let mut list: Vec<Hwnd> = Vec::with_capacity(512);
    // SAFETY: обратный вызов синхронный, указатель на `list` действителен до возврата.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut Vec<Hwnd> as isize));
    }
    list
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFO> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info =
        MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: запись в собственную структуру с верным cbSize.
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then_some(info)
}

/// Текст из буфера UTF-16: `count` знаков, до первого NUL (как `buffer.value` в ctypes).
fn wide_text(buffer: &[u16], count: i32) -> String {
    let count = (count.max(0) as usize).min(buffer.len());
    let end = buffer[..count].iter().position(|&u| u == 0).unwrap_or(count);
    String::from_utf16_lossy(&buffer[..end])
}

/// Пересечение как `intersect` в Python: пустое -> None. (Своя копия, чтобы `win` не зависел
/// от `core`; совпадает с `core::geometry::intersect` для двух прямоугольников.)
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect { l: a.l.max(b.l), t: a.t.max(b.t), r: a.r.min(b.r), b: a.b.min(b.b) };
    (r.r > r.l && r.b > r.t).then_some(r)
}

/// Площадь как `area` в Python.
fn area(r: Rect) -> i64 {
    (r.r as i64 - r.l as i64).max(0) * (r.b as i64 - r.t as i64).max(0)
}
