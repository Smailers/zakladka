//! Буфер обмена и Ctrl+C (P 925–1103) — дословно по шагам.
//!
//! Исключения Python (буфер занят, GetClipboardData/GlobalLock не удались) здесь — `Err(Failed)`;
//! снаружи любой такой исход — `None`, как `except` у вызывающего в Python. Порядок шагов,
//! проверки номера последовательности и ветки `finally` сохранены.

use std::thread::sleep;
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY,
};

use crate::types::{to_hwnd, Hwnd};
use crate::win::api::{window_pid, Api};
use crate::win::wake::monotonic;

const CF_UNICODETEXT: u32 = 13;
/// BITMAP, METAFILEPICT, PALETTE, ENHMETAFILE: не память (`GlobalSize` к ним неприменим).
const GDI_FORMATS: [u32; 4] = [2, 3, 9, 14];
/// Больше — буфер не трогаем вовсе.
const BACKUP_LIMIT: usize = 64 * 1024 * 1024;
/// Предел текста в `read_clipboard`.
const TEXT_LIMIT: usize = 32 * 1024 * 1024;
/// Shift, Ctrl, Alt, левая и правая Win.
const MODIFIERS: [i32; 5] = [0x10, 0x11, 0x12, 0x5B, 0x5C];
const VK_CONTROL: u16 = 0x11;
const VK_C: u16 = 0x43;

/// Шаг не удался (в Python — исключение).
struct Failed;
type Step<T> = Result<T, Failed>;

/// Сохранённые форматы: (номер формата, байты).
type Saved = Vec<(u32, Vec<u8>)>;

impl Api {
    /// Ctrl+C в `foreground` с сохранением и восстановлением буфера; `owner` — наше окно-владелец.
    pub fn copy_selection(&self, owner: Hwnd, foreground: Hwnd, wait_s: f64) -> Option<String> {
        copy_selection(self, owner, foreground, wait_s).ok().flatten()
    }
}

/// До 0,2 с ждать, пока буфер освободится.
fn open_clipboard(owner: Hwnd) -> Step<()> {
    let deadline = monotonic() + 0.2;
    while monotonic() < deadline {
        // SAFETY: открытие буфера; закрывается вызывающим на всех ветках.
        if unsafe { OpenClipboard(to_hwnd(owner)) }.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(10));
    }
    Err(Failed) // Clipboard busy
}

fn close_clipboard() {
    // SAFETY: закрытие буфера, открытого этим потоком.
    unsafe {
        let _ = CloseClipboard();
    }
}

fn sequence() -> u32 {
    // SAFETY: без аргументов.
    unsafe { GetClipboardSequenceNumber() }
}

/// Вызывающий держит OpenClipboard; `None` — нет текста, `""` — пустой текст.
fn read_clipboard() -> Step<Option<String>> {
    // SAFETY: буфер открыт вызывающим; память формата читается под GlobalLock в пределах
    // GlobalSize и отпускается GlobalUnlock на всех ветках.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return Ok(None);
        }
        let handle = GetClipboardData(CF_UNICODETEXT).map_err(|_| Failed)?;
        let memory = HGLOBAL(handle.0);
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            return Err(Failed);
        }
        let size = GlobalSize(memory);
        let text = if size == 0 || size > TEXT_LIMIT {
            Err(Failed) // Clipboard size
        } else {
            let units = std::slice::from_raw_parts(pointer as *const u16, size / 2);
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            Ok(Some(String::from_utf16_lossy(&units[..end])))
        };
        let _ = GlobalUnlock(memory);
        text
    }
}

/// Форматы, которые не сохраняются: GDI, «частные» 0x80–0x8E, 0x300–0x3FF.
fn skipped(format: u32) -> bool {
    GDI_FORMATS.contains(&format) || (0x80..=0x8E).contains(&format) || (0x300..=0x3FF).contains(&format)
}

/// Все форматы-память открытого буфера как байты (картинки — через CF_DIB);
/// `None` — слишком много, чтобы сохранить.
fn backup_clipboard(limit: usize) -> Option<Saved> {
    let mut saved = Saved::new();
    let mut total = 0usize;
    let mut format = 0u32;
    loop {
        // SAFETY: буфер открыт вызывающим.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            return Some(saved);
        }
        if skipped(format) {
            continue;
        }
        // SAFETY: буфер открыт; дескриптор принадлежит системе, мы его только читаем.
        let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
            continue;
        };
        let memory = HGLOBAL(handle.0);
        // SAFETY: см. выше.
        let size = unsafe { GlobalSize(memory) };
        if size == 0 {
            continue;
        }
        total = total.saturating_add(size);
        if total > limit {
            return None;
        }
        // SAFETY: чтение `size` байт под GlobalLock, затем GlobalUnlock.
        unsafe {
            let pointer = GlobalLock(memory);
            if pointer.is_null() {
                continue;
            }
            let data = std::slice::from_raw_parts(pointer as *const u8, size).to_vec();
            let _ = GlobalUnlock(memory);
            saved.push((format, data));
        }
    }
}

/// Вернуть сохранённые форматы, если пользователь тем временем не скопировал новое.
/// Добавляет `CanIncludeInClipboardHistory` = 0: восстановленное не попадает в журнал Win+V.
fn restore_all(owner: Hwnd, saved: &[(u32, Vec<u8>)], expected_sequence: u32) -> Step<()> {
    open_clipboard(owner)?;
    let result = put_all(saved, expected_sequence);
    close_clipboard(); // finally
    result
}

fn put_all(saved: &[(u32, Vec<u8>)], expected_sequence: u32) -> Step<()> {
    if sequence() != expected_sequence {
        return Ok(());
    }
    // SAFETY: буфер открыт вызывающим.
    unsafe { EmptyClipboard() }.map_err(|_| Failed)?;
    // SAFETY: регистрация имени формата (статическая строка).
    let history = unsafe { RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")) };
    let marker = [0u8; 4];
    let extra = (history != 0).then_some((history, &marker[..]));
    for (format, data) in saved.iter().map(|(f, d)| (*f, d.as_slice())).chain(extra) {
        put_data(format, data);
    }
    Ok(())
}

/// GlobalAlloc + копия + SetClipboardData; память освобождаем сами, только если система её
/// не приняла.
fn put_data(format: u32, data: &[u8]) {
    // SAFETY: собственный блок памяти: запись в пределах выделенного размера под GlobalLock;
    // после успешного SetClipboardData блок принадлежит системе и не освобождается.
    unsafe {
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)) else {
            return;
        };
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(memory);
            return;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), pointer as *mut u8, data.len());
        let _ = GlobalUnlock(memory);
        if SetClipboardData(format, HANDLE(memory.0)).is_err() {
            let _ = GlobalFree(memory); // всё ещё наш, если SetClipboardData не удался
        }
    }
}

fn key(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Пусто или одни пробельные знаки (как `not text.strip()` в Python: там пробельными считаются
/// ещё U+001C–U+001F).
fn blank(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || ('\u{1C}'..='\u{1F}').contains(&c))
}

/// Выделенный текст через искусственный Ctrl+C; прежний буфер восстанавливается целиком.
fn copy_selection(api: &Api, owner: Hwnd, foreground: Hwnd, wait: f64) -> Step<Option<String>> {
    let held = || MODIFIERS.iter().any(|&vk| api.key_down(vk));
    let deadline = monotonic() + 1.0;
    while held() {
        if monotonic() >= deadline {
            return Ok(None);
        }
        sleep(Duration::from_millis(5));
    }
    if api.foreground() != foreground {
        return Ok(None);
    }
    open_clipboard(owner)?;
    let backup = (|| -> Step<_> {
        let previous = read_clipboard()?;
        let saved = backup_clipboard(BACKUP_LIMIT);
        Ok((previous, saved, sequence()))
    })();
    close_clipboard(); // finally
    let (previous, saved, before) = backup?;
    let Some(saved) = saved else {
        return Ok(None); // слишком много в буфере, чтобы надёжно сохранить: не трогаем
    };
    if api.foreground() != foreground || sequence() != before || held() {
        return Ok(None);
    }
    let inputs = [
        key(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key(VK_C, KEYBD_EVENT_FLAGS(0)),
        key(VK_C, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let size = std::mem::size_of::<INPUT>() as i32;
    // SAFETY: собственный массив событий клавиатуры.
    let sent = unsafe { SendInput(&inputs, size) };
    if sent != 4 {
        // Отпустить C и Ctrl, что бы ни дошло.
        // SAFETY: собственный массив событий клавиатуры.
        unsafe {
            SendInput(&inputs[2..], size);
        }
        if sent < 2 {
            return Ok(None);
        }
    }
    let deadline = monotonic() + wait;
    while sequence() == before {
        if monotonic() >= deadline {
            return Ok(None); // ничего не скопировано; старый буфер никогда не выдаём за выделение
        }
        sleep(Duration::from_millis(5));
    }
    let mut copied_sequence = sequence();
    let mut restore_previous = true;
    // try:
    let outcome = read_copied(owner, foreground, &previous, &mut copied_sequence, &mut restore_previous);
    // finally:
    if restore_previous {
        restore_all(owner, &saved, copied_sequence)?;
    }
    let Some(selected) = outcome? else {
        return Ok(None);
    };
    if sent != 4 || api.foreground() != foreground {
        return Ok(None);
    }
    Ok(selected.filter(|text| !blank(text)))
}

/// Тело `try` после Ctrl+C: прочитать скопированное. `Ok(None)` — ранний `return None`
/// (восстановление решает `restore_previous`), `Ok(Some(selected))` — дальше по шагам.
fn read_copied(
    owner: Hwnd,
    foreground: Hwnd,
    previous: &Option<String>,
    copied_sequence: &mut u32,
    restore_previous: &mut bool,
) -> Step<Option<Option<String>>> {
    open_clipboard(owner)?;
    let result = (|| {
        // SAFETY: без аргументов; нет владельца — 0.
        let clipboard_owner = unsafe { GetClipboardOwner() }.unwrap_or_default();
        let owner_pid = window_pid(clipboard_owner);
        let foreground_pid = window_pid(to_hwnd(foreground));
        // Программа пишет несколько форматов подряд, и номер растёт после каждого
        // (EmptyClipboard, затем каждый SetClipboardData). Ожидание выше могло застать
        // запись на середине: если буфер принадлежит той же программе, это её копия,
        // а не новое копирование пользователя.
        let by_target = foreground_pid != 0 && owner_pid == foreground_pid;
        if sequence() != *copied_sequence && !by_target {
            return Ok(None);
        }
        let selected = read_clipboard()?;
        *copied_sequence = sequence();
        if selected == *previous && !by_target {
            *restore_previous = false;
            return Ok(None);
        }
        Ok(Some(selected))
    })();
    close_clipboard(); // finally
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    use crate::types::from_hwnd;

    #[test]
    fn skipped_formats() {
        for format in [2, 3, 9, 14, 0x80, 0x8E, 0x300, 0x3FF] {
            assert!(skipped(format), "{format:#x}");
        }
        for format in [1, 7, 8, 13, 16, 17, 0x7F, 0x8F, 0x2FF, 0x400, 0xC000] {
            assert!(!skipped(format), "{format:#x}");
        }
    }

    #[test]
    fn blank_like_python_strip() {
        assert!(blank(""));
        assert!(blank(" \t\r\n\u{A0}\u{3000}\u{1C}\u{1F}"));
        assert!(!blank(" a "));
        assert!(!blank("\u{200B}")); // в Python тоже не пробел
    }

    /// Скрытое окно только для сообщений: не видно, фокус не берёт.
    struct Owner(Hwnd);

    impl Owner {
        fn new() -> Owner {
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("zakladka-clipboard-test"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    None,
                    None,
                    None,
                )
            }
            .expect("окно для сообщений");
            Owner(from_hwnd(hwnd))
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(to_hwnd(self.0));
            }
        }
    }

    fn utf16z(text: &str) -> Vec<u8> {
        text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    fn data_of(saved: &[(u32, Vec<u8>)], format: u32) -> Option<&[u8]> {
        saved.iter().find(|(f, _)| *f == format).map(|(_, d)| d.as_slice())
    }

    fn snapshot(owner: Hwnd) -> (Option<String>, Saved) {
        assert!(open_clipboard(owner).is_ok(), "буфер занят");
        let text = read_clipboard().ok().flatten();
        let saved = backup_clipboard(BACKUP_LIMIT);
        close_clipboard();
        (text, saved.expect("сохранение"))
    }

    /// Круговой тест на собственном скрытом окне. Трогает настоящий буфер обмена человека
    /// (на миллисекунды; в конце буфер человека возвращается тем же `restore_all`), поэтому
    /// `#[ignore]`. Запуск из PowerShell в папке проекта:
    /// `& .\build.ps1 test --lib clipboard_round_trip '--' --ignored`
    /// (`--` в кавычках: без них PowerShell не передаёт его скрипту; через
    /// `powershell -File build.ps1 … -- …` не работает).
    #[test]
    #[ignore = "трогает настоящий буфер обмена; запускает Claude"]
    fn clipboard_round_trip() {
        let owner = Owner::new();
        assert!(open_clipboard(owner.0).is_ok(), "буфер занят");
        let user = backup_clipboard(BACKUP_LIMIT);
        close_clipboard();
        let user = user.expect("в буфере человека больше 64 МБ — тест его не трогает");
        let result = std::panic::catch_unwind(|| round_trip(owner.0));
        // Вернуть буфер человека (если он ничего не скопировал за время теста).
        let returned = restore_all(owner.0, &user, sequence()).is_ok();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        assert!(returned, "буфер человека не возвращён");
    }

    fn round_trip(owner: Hwnd) {
        let custom = unsafe { RegisterClipboardFormatW(w!("Zakladka.Test.RoundTrip")) };
        assert_ne!(custom, 0);
        let history = unsafe { RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")) };
        let text = "Закладка: проверка ✓ 123";
        let blob: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        // Положить текст и свой формат (restore_all добавит запрет журнала Win+V).
        let start = [(CF_UNICODETEXT, utf16z(text)), (custom, blob.clone())];
        assert!(restore_all(owner, &start, sequence()).is_ok(), "запись");
        // Снять копию, как copy_selection перед Ctrl+C.
        let (previous, saved) = snapshot(owner);
        let before = sequence();
        assert!(previous.as_deref() == Some(text), "текст не прочитан");
        assert!(data_of(&saved, custom).is_some_and(|d| d.starts_with(&blob)), "свой формат не сохранён");
        // «Ctrl+C»: в буфере другое.
        let copied = [(CF_UNICODETEXT, utf16z("скопировано"))];
        assert!(restore_all(owner, &copied, before).is_ok(), "запись");
        let after_copy = sequence();
        assert_ne!(after_copy, before);
        // Номер последовательности устарел (человек скопировал новое) — буфер не трогаем.
        assert!(restore_all(owner, &saved, before).is_ok());
        assert_eq!(sequence(), after_copy, "буфер тронут при устаревшем номере");
        assert!(snapshot(owner).0.as_deref() == Some("скопировано"), "буфер тронут");
        // Обычное восстановление: текст, свой формат и запрет журнала.
        assert!(restore_all(owner, &saved, sequence()).is_ok());
        let (restored, all) = snapshot(owner);
        assert!(restored.as_deref() == Some(text), "текст не восстановлен");
        assert!(data_of(&all, custom).is_some_and(|d| d.starts_with(&blob)), "свой формат не восстановлен");
        assert!(data_of(&all, history).is_some_and(|d| d.starts_with(&[0, 0, 0, 0])), "нет запрета журнала");
    }
}
