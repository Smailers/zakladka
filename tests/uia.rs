//! Часть 3: UI Automation и Windows OCR (раздел 6 плана, «Готово, когда» (а)–(в)).
//!
//! Тесты не мешают человеку за компьютером: дерево UIA только читается (никаких Invoke, Select,
//! ScrollIntoView, фокуса), окна не создаются и не показываются, буфер обмена и клавиши не
//! трогаются. Текст чужих окон не печатается — только числа. OCR читает собственный битмап в памяти.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Media::Ocr::OcrEngine;
use windows::Win32::Foundation::{COLORREF, HANDLE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush, PatBlt, SelectObject,
    SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, TRANSPARENT,
    WHITENESS,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

use zakladka::ocr::Ocr;
use zakladka::types::{from_hwnd, Hwnd, Point};
use zakladka::uia::{Element, ScopeKey, Uia};
use zakladka::win::Bitmap;

/// Тесты идут по одному: замер памяти не должен видеть выделений соседнего теста (OCR — десятки МБ).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Поток теста — в MTA, как tracker и locator.
fn mta() {
    // SAFETY: инициализация COM текущего потока; повторный вызов безвреден.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn desktop() -> Hwnd {
    // SAFETY: GetDesktopWindow не имеет предусловий.
    from_hwnd(unsafe { GetDesktopWindow() })
}

/// Корень рабочего стола: `shown_scopes` без прямоугольника — без hit test, сразу `ElementFromHandle`.
fn desktop_root(uia: &Uia) -> Element {
    let mut scopes = uia.shown_scopes(desktop(), 0, None);
    assert_eq!(scopes.len(), 1, "корень рабочего стола");
    let (key, root) = scopes.pop().expect("корень");
    assert!(matches!(key, ScopeKey::Root(_)), "без прямоугольника — только Root");
    root
}

/// Private bytes своего процесса.
fn private_bytes() -> i64 {
    // SAFETY: структура нужного размера, дескриптор своего процесса.
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            counters.cb,
        )
        .expect("GetProcessMemoryInfo");
        counters.PrivateUsage as i64
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
#[ignore = "читает поддерево всего рабочего стола (2–4,5 с, нагружает все программы) и упирается в таймаут UIA 2 с при многих окнах; запускать вручную: cargo test --test uia -- --ignored"]
fn uia_new_and_desktop_root_segments_200() {
    let _serial = serial();
    mta();
    let started = Instant::now();
    let uia = Uia::new().expect("Uia::new");
    let created = started.elapsed();
    let root = desktop_root(&uia);
    let started = Instant::now();
    let segments = uia.segments(&root, 200).expect("segments: BuildUpdatedCache корня рабочего стола");
    let took = started.elapsed();
    assert!(segments.len() <= 200, "листьев не больше пройденных узлов");
    assert!(segments.iter().all(|s| !s.name.trim().is_empty()), "лист с пустым именем — не сегмент");
    println!(
        "Uia::new {:.1} мс; segments(корень рабочего стола, 200): {} листьев за {:.1} мс",
        ms(created),
        segments.len(),
        ms(took)
    );
}

#[test]
fn desktop_root_read_only_calls() {
    let _serial = serial();
    mta();
    let uia = Uia::new().expect("Uia::new");
    let root = desktop_root(&uia);

    let rid = uia.rid(&root).expect("runtime id корня");
    assert!(!rid.is_empty());
    assert_eq!(uia.parent_rid(&root), None, "у корня нет родителя");
    assert!(!uia.is_document(&root));
    assert!(uia.document_of(&root).is_none());
    assert!(uia.text_pattern(&root).is_none());
    let rect = uia.current_rect(&root).expect("прямоугольник корня");
    assert!(rect.r > rect.l && rect.b > rect.t, "{rect:?}");
    assert!(uia.current_name(&root).is_some());
    let (url, _name) = uia.document_info(&root); // имя не печатается
    assert!(url.is_empty(), "у корня нет ValuePattern");

    let (inner, outer) = uia.lineage(&root, 40);
    assert!(inner.contains(&rid) && inner.len() == 1, "Document нет — корень внутри");
    assert!(outer.is_empty());

    // Три hit test по средней линии; процесс 0 ни с чем не совпадёт — корень окна как Root.
    let scopes = uia.shown_scopes(desktop(), 0, Some(rect));
    assert!(matches!(scopes.as_slice(), [(ScopeKey::Root(key), _)] if *key == rid));

    // Выделение своего процесса: окон у теста нет — пусто (фокус и точка — чужих процессов).
    let selection = uia.selection(std::process::id(), Point { x: rect.l + 1, y: rect.t + 1 });
    assert!(selection.text.is_empty() && selection.rects.is_empty() && selection.scope.is_none());

    // hit test: только чтение; цепочка не длиннее depth.
    let p = Point { x: (rect.l + rect.r) / 2, y: (rect.t + rect.b) / 2 };
    uia.prime(p);
    assert!(uia.hit_chain(p, 3, 0.0).len() <= 3);
    assert!(uia.hit_chain(p, 0, 0.0).is_empty());

    // Без TextPattern прямоугольников нет; неверное окно — пусто, без паники.
    assert!(uia.piece_rects(None, &root, "текст", 0, 5).is_empty());
    assert!(uia.shown_scopes(0x7FFF_FFF0, 0, None).is_empty());
    assert!(uia.tabs(0x7FFF_FFF0).is_empty());
}

#[test]
fn rid_and_current_rect_do_not_leak() {
    let _serial = serial();
    mta();
    let uia = Uia::new().expect("Uia::new");
    let root = desktop_root(&uia);
    // Прогрев: кучи процесса и кэши COM.
    for _ in 0..200 {
        let _ = uia.rid(&root);
        let _ = uia.current_rect(&root);
    }
    let before = private_bytes();
    let started = Instant::now();
    for _ in 0..1000 {
        assert!(uia.rid(&root).is_some());
        assert!(uia.current_rect(&root).is_some());
    }
    let took = started.elapsed();
    let after_1000 = private_bytes();
    // Ещё 19 000 пар: утечка SAFEARRAY (~50 байт на вызов) дала бы здесь около 1 МБ.
    for _ in 0..19_000 {
        let _ = uia.rid(&root);
        let _ = uia.current_rect(&root);
    }
    let after_20000 = private_bytes();
    let (grown_1000, grown_20000) = (after_1000 - before, after_20000 - before);
    println!(
        "rid + current_rect: 1000 пар за {:.1} мс; private: +{} КБ после 1000, +{} КБ после 20000",
        ms(took),
        grown_1000 / 1024,
        grown_20000 / 1024
    );
    assert!(grown_1000 < 1 << 20, "после 1000 вызовов private выросла на {grown_1000} байт");
    assert!(grown_20000 < 1 << 20, "после 20000 вызовов private выросла на {grown_20000} байт");
}

/// Строка `text` шрифтом Segoe UI `px` пикселей, чёрным по белому, в собственном битмапе в памяти
/// (memory DC + DIB; на экран ничего не выводится).
fn render(text: &str, width: i32, height: i32, px: i32) -> Bitmap {
    // SAFETY: собственные объекты GDI; все освобождаются в конце.
    unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        assert!(!dc.0.is_null(), "CreateCompatibleDC");
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // сверху вниз
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).expect("CreateDIBSection");
        let old_bitmap = SelectObject(dc, dib);
        let _ = PatBlt(dc, 0, 0, width, height, WHITENESS);
        // 400 = FW_NORMAL, 1 = DEFAULT_CHARSET, 4 = ANTIALIASED_QUALITY.
        let font = CreateFontW(-px, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 4, 0, w!("Segoe UI"));
        assert!(!font.0.is_null(), "CreateFontW");
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0));
        let wide: Vec<u16> = text.encode_utf16().collect();
        let _ = TextOutW(dc, 16, (height - px) / 2, &wide);
        let _ = GdiFlush();
        let mut bgra = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel[3] = 255; // GDI оставляет альфу нулевой; кадр экрана непрозрачен
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(font);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(dc);
        Bitmap { width, height, bgra }
    }
}

#[test]
fn ocr_reads_rendered_line() {
    let _serial = serial();
    mta();
    let started = Instant::now();
    let supported = Ocr::supported();
    let check = started.elapsed();
    if !supported {
        // Урезанная Windows без языков OCR: программа обязана работать дальше, тест — тоже.
        println!("OCR в этой Windows нет ({:.1} мс) — распознавание не проверялось", ms(check));
        assert!(Ocr::new().is_err());
        return;
    }
    let memory_start = private_bytes();
    let started = Instant::now();
    let ocr = Ocr::new().expect("Ocr::new");
    let created = started.elapsed();

    let (width, height) = (600, 80);
    let bitmap = render("Проверка OCR 123", width, height, 20);
    let started = Instant::now();
    let words = ocr.words(&bitmap, true).expect("words");
    let first = started.elapsed();
    let started = Instant::now();
    let again = ocr.words(&bitmap, true).expect("words, второй раз");
    let second = started.elapsed();
    let latin = ocr.words(&bitmap, false).expect("words, en-US");
    let memory_peak = private_bytes();

    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    let latin_texts: Vec<&str> = latin.iter().map(|w| w.text.as_str()).collect();
    println!(
        "OCR: supported {:.1} мс; new {:.1} мс; words 600×80: {:.1} мс, повтор {:.1} мс; ru {:?}; en-US {:?}",
        ms(check),
        ms(created),
        ms(first),
        ms(second),
        texts,
        latin_texts
    );
    assert!(texts.iter().any(|t| t.contains("Проверка")), "нет «Проверка»: {texts:?}");
    assert_eq!(words.len(), again.len(), "повтор на том же движке");
    for word in &words {
        let r = word.rect;
        assert!(r.l >= 0 && r.t >= 0 && r.r <= width && r.b <= height, "вне битмапа: {r:?}");
        assert!(r.l < r.r && r.t < r.b, "пустой прямоугольник: {r:?}");
    }

    // Границы: пусто — пусто; неверная длина — Err; сторона больше MaxImageDimension — пусто.
    let empty = Bitmap { width: 0, height: 0, bgra: Vec::new() };
    assert!(ocr.words(&empty, false).expect("пустой").is_empty());
    let broken = Bitmap { width: 10, height: 10, bgra: vec![0; 10] };
    assert!(ocr.words(&broken, false).is_err());
    let max = OcrEngine::MaxImageDimension().expect("MaxImageDimension") as i32;
    let wide = Bitmap { width: max + 1, height: 1, bgra: vec![255; (max as usize + 1) * 4] };
    assert!(ocr.words(&wide, false).expect("слишком широкий").is_empty());

    drop(ocr);
    let memory_end = private_bytes();
    println!(
        "OCR private: +{} КБ с движками и распознаванием, +{} КБ после drop",
        (memory_peak - memory_start) / 1024,
        (memory_end - memory_start) / 1024
    );
}
