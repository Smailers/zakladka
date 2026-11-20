//! Окна меток (часть 6; P 2591–2689, 2711–2755): 24 подсветки, 6 полос, 6 флажков ✕, 2 плашки.
//!
//! Окна создаёт UI-поток заранее (никогда не во время работы пользователя), скрытыми в
//! (−32000, −32000); ставит и прячет их поток tracker (`Overlay`, `SWP_ASYNCWINDOWPOS`). Все —
//! `WS_POPUP` с `WS_EX_TOPMOST | NOACTIVATE | TOOLWINDOW | LAYERED`; у подсветок и полос ещё
//! `WS_EX_TRANSPARENT` (щелчки проходят насквозь). Альфа: подсветка = `opacity`, полоса 0,9,
//! флажок и плашка 0,96 (подсветкам её потом меняет `Overlay`).
//!
//! Цвета рисования — снимок `Shared.config`, сделанный при создании и в `recolor`. Мышь над
//! флажком или плашкой — сообщение `WM_APP_PIECE` окну-владельцу (`set_owner`): щелчок, наведение,
//! уход указателя. Что с этим делать, решает владелец (`ui::host`).

use std::cell::{Cell, RefCell};
use std::ptr::null_mut;
use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    FillRect, GetTextExtentPoint32W, GetTextMetricsW, InvalidateRect, SelectObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, FW_BOLD, HDC, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, GetWindowLongPtrW, IsWindowVisible, PostMessageW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, GWLP_USERDATA, HMENU, HTTRANSPARENT, LWA_ALPHA,
    MA_NOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WINDOW_EX_STYLE,
    WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::core::color::{label_color, FLAG_COLOR};
use crate::types::{
    from_hwnd, to_hwnd, Config, Dir, Hwnd, LabelColor, OverlayWindows, Rgb, Shared, BARS, FLAGS, HIGHLIGHTS,
    WM_APP_EVENTS,
};
use crate::ui::gfx::{instance, register_class};

/// В crate `windows` константа лежит в `Win32_UI_Controls` (feature не включён).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Сообщение окну-владельцу о мыши над флажком или плашкой (`PieceEvent::encode`).
pub(crate) const WM_APP_PIECE: u32 = WM_APP_EVENTS + 2;

pub(crate) const PILL_UP_TEXT: &str = "▲ Закладка";
pub(crate) const PILL_DOWN_TEXT: &str = "▼ Закладка";
const FLAG_TEXT: &str = "✕";

const HIGHLIGHT_CLASS: &str = "ZakladkaHighlight";
const BAR_CLASS: &str = "ZakladkaBar";
const FLAG_CLASS: &str = "ZakladkaFlag";
const PILL_CLASS: &str = "ZakladkaPill";

/// Альфа полосы и флажка/плашки (P 2608, 2613).
const BAR_ALPHA: f64 = 0.9;
const BUTTON_ALPHA: f64 = 0.96;

/// Что сделала мышь над флажком или плашкой.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PieceEvent {
    FlagClick(usize),
    PillClick(Dir),
    FlagEnter(usize),
    PillEnter(Dir),
    Leave,
}

impl PieceEvent {
    /// (wparam, lparam) для `WM_APP_PIECE`.
    pub(crate) fn encode(self) -> (usize, isize) {
        let pill = |d: Dir| if d == Dir::Up { 0 } else { 1 };
        match self {
            PieceEvent::FlagClick(i) => (1, i as isize),
            PieceEvent::PillClick(d) => (2, pill(d)),
            PieceEvent::FlagEnter(i) => (3, i as isize),
            PieceEvent::PillEnter(d) => (4, pill(d)),
            PieceEvent::Leave => (5, 0),
        }
    }

    pub(crate) fn decode(wparam: usize, lparam: isize) -> Option<PieceEvent> {
        let dir = || match lparam {
            0 => Some(Dir::Up),
            1 => Some(Dir::Down),
            _ => None,
        };
        let flag = || usize::try_from(lparam).ok().filter(|&i| i < FLAGS);
        match wparam {
            1 => flag().map(PieceEvent::FlagClick),
            2 => dir().map(PieceEvent::PillClick),
            3 => flag().map(PieceEvent::FlagEnter),
            4 => dir().map(PieceEvent::PillEnter),
            5 => Some(PieceEvent::Leave),
            _ => None,
        }
    }
}

/// `round()` Python (половины — к чётному).
fn py_round(v: f64) -> i32 {
    let r = v.round_ties_even();
    if r.is_finite() {
        r.clamp(i32::MIN as f64, i32::MAX as f64) as i32
    } else {
        0
    }
}

/// Масштаб для размеров: 1…4, нечисло — 1.
pub(crate) fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(1.0, 4.0)
    } else {
        1.0
    }
}

/// Сторона флажка: `max(14, round(16 × scale))` (P 2540).
pub(crate) fn flag_size(scale: f64) -> i32 {
    py_round(16.0 * scale).max(14)
}

/// Высота «✕»: `round(0,7 × размер)` (P 2619).
pub(crate) fn flag_font_px(size: i32) -> i32 {
    py_round(size as f64 * 0.7).max(1)
}

/// Высота надписи плашки: `round(14 × scale)` (P 2621).
pub(crate) fn pill_font_px(scale: f64) -> i32 {
    py_round(14.0 * scale).max(1)
}

/// Поля плашки: `round(9 × scale)` по бокам, `round(4 × scale)` сверху и снизу (P 2622).
pub(crate) fn pill_padding(scale: f64) -> (i32, i32) {
    (py_round(9.0 * scale), py_round(4.0 * scale))
}

/// Размер плашки по размерам надписей «▲ …» и «▼ …»: общий для обеих (одно `pill_size`).
pub(crate) fn pill_size(texts: &[(i32, i32)], scale: f64) -> (i32, i32) {
    let (padx, pady) = pill_padding(scale);
    let width = texts.iter().map(|t| t.0).max().unwrap_or(0);
    let height = texts.iter().map(|t| t.1).max().unwrap_or(0);
    ((width + 2 * padx).max(1), (height + 2 * pady).max(1))
}

/// Байт альфы: `max(1, min(255, round(value × 255)))` (как `Overlay.set_alpha`, P 2364).
pub(crate) fn alpha_byte(value: f64) -> u8 {
    let v = (value * 255.0).round_ties_even();
    if v.is_nan() {
        1
    } else {
        v.clamp(1.0, 255.0) as u8
    }
}

/// Цвет надписи на плашке: выбранный или авто по контрасту с акцентом (P 2591–2593).
pub(crate) fn pill_text_color(config: &Config) -> Rgb {
    match config.label_color {
        LabelColor::Custom(c) => c,
        LabelColor::Auto => label_color(config.bar_color),
    }
}

/// Цвета, которыми рисуются окна меток (снимок настроек).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Snapshot {
    pub highlight: Rgb,
    pub bar: Rgb,
    pub pill_text: Rgb,
}

impl Snapshot {
    pub(crate) fn of(config: &Config) -> Snapshot {
        Snapshot { highlight: config.color, bar: config.bar_color, pill_text: pill_text_color(config) }
    }
}

/// Шрифт GDI с освобождением.
struct GdiFont(HFONT);

impl Drop for GdiFont {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            // SAFETY: шрифт создан здесь; в DC он выбирается только на время рисования.
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

/// «Segoe UI» полужирный (bold, как Tk `"bold"`) высотой символов `px`.
fn bold_font(px: i32) -> GdiFont {
    // SAFETY: имя — литерал с нулём.
    let font = unsafe {
        CreateFontW(
            -px.max(1),
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        )
    };
    GdiFont(font)
}

/// (ширина, высота строки) текста шрифтом `font`.
fn measure(font: HFONT, text: &str) -> (i32, i32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: DC свой, создаётся и удаляется здесь; прежний шрифт возвращается.
    unsafe {
        let dc = CreateCompatibleDC(HDC(null_mut()));
        if dc.0.is_null() {
            return (0, 0);
        }
        let old = SelectObject(dc, font);
        let _ = GetTextMetricsW(dc, &mut metrics);
        if !wide.is_empty() {
            let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
        }
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
    }
    (size.cx, metrics.tmHeight)
}

struct Style {
    colors: Snapshot,
    flag_font: GdiFont,
    pill_font: GdiFont,
}

thread_local! {
    static STYLE: RefCell<Option<Style>> = const { RefCell::new(None) };
    static OWNER: Cell<Hwnd> = const { Cell::new(0) };
    static HOVER: RefCell<Vec<Hwnd>> = const { RefCell::new(Vec::new()) };
}

/// Окно-владелец, которому флажки и плашки сообщают о мыши. Вызывать до `create_overlay_windows`.
pub(crate) fn set_owner(owner: Hwnd) {
    OWNER.with(|o| o.set(owner));
}

/// Снимок цветов, которыми сейчас рисуются окна меток (`None` — окон ещё нет). Для тестов.
#[cfg(test)]
pub(crate) fn colors() -> Option<Snapshot> {
    STYLE.try_with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.colors))).ok().flatten()
}

fn read_config(shared: &Shared) -> Config {
    match shared.config.read() {
        Ok(config) => config.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

pub fn create_overlay_windows(shared: &Arc<Shared>, scale: f64) -> OverlayWindows {
    let scale = clean_scale(scale);
    let config = read_config(shared);
    let flag = flag_size(scale);
    let flag_font = bold_font(flag_font_px(flag));
    let pill_font = bold_font(pill_font_px(scale));
    let texts = [measure(pill_font.0, PILL_UP_TEXT), measure(pill_font.0, PILL_DOWN_TEXT)];
    let pill = pill_size(&texts, scale);
    let _ = STYLE.try_with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            *s = Some(Style { colors: Snapshot::of(&config), flag_font, pill_font });
        }
    });

    let base = WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED;
    let clear = base | WS_EX_TRANSPARENT;
    let highlight = register_class(HIGHLIGHT_CLASS, highlight_proc, false);
    let bar = register_class(BAR_CLASS, bar_proc, false);
    let flag_class = register_class(FLAG_CLASS, flag_proc, true);
    let pill_class = register_class(PILL_CLASS, pill_proc, true);

    let opacity = alpha_byte(config.opacity);
    let button = alpha_byte(BUTTON_ALPHA);
    OverlayWindows {
        highlights: (0..HIGHLIGHTS).map(|i| create(highlight, clear, (1, 1), opacity, i)).collect(),
        bars: (0..BARS).map(|i| create(bar, clear, (1, 1), alpha_byte(BAR_ALPHA), i)).collect(),
        flags: (0..FLAGS).map(|i| create(flag_class, base, (flag, flag), button, i)).collect(),
        pill_up: create(pill_class, base, pill, button, 0),
        pill_down: create(pill_class, base, pill, button, 1),
        pill_size: pill,
        flag_size: flag,
    }
}

/// После смены цветов.
pub fn recolor(shared: &Shared, w: &OverlayWindows) {
    let config = read_config(shared);
    let _ = STYLE.try_with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Some(style) = s.as_mut() {
                style.colors = Snapshot::of(&config);
            }
        }
    });
    for hwnd in all_windows(w) {
        // SAFETY: окно своё (или 0 — тогда вызов ничего не делает); перерисовка асинхронная.
        unsafe {
            let _ = InvalidateRect(to_hwnd(hwnd), None, BOOL(0));
        }
    }
}

/// Все окна меток (без нулевых).
pub(crate) fn all_windows(w: &OverlayWindows) -> impl Iterator<Item = Hwnd> + '_ {
    w.highlights
        .iter()
        .chain(&w.bars)
        .chain(&w.flags)
        .copied()
        .chain([w.pill_up, w.pill_down])
        .filter(|&h| h != 0)
}

/// Копия набора дескрипторов: сам набор уходит в tracker, UI нужен свой.
pub(crate) fn copy(w: &OverlayWindows) -> OverlayWindows {
    OverlayWindows {
        highlights: w.highlights.clone(),
        bars: w.bars.clone(),
        flags: w.flags.clone(),
        pill_up: w.pill_up,
        pill_down: w.pill_down,
        pill_size: w.pill_size,
        flag_size: w.flag_size,
    }
}

/// Видно ли хоть одно окно меток.
pub(crate) fn any_visible(w: &OverlayWindows) -> bool {
    // SAFETY: только чтение состояния окон.
    all_windows(w).any(|h| unsafe { IsWindowVisible(to_hwnd(h)).as_bool() })
}

/// Спрятать все окна меток из UI-потока (панель, справка, сторож). Кэш `Overlay` об этом не
/// знает — после надо послать `Command::Rerender`.
pub(crate) fn hide_all(w: &OverlayWindows) {
    for hwnd in all_windows(w) {
        // SAFETY: окна свои, потока UI.
        unsafe {
            let window = to_hwnd(hwnd);
            if IsWindowVisible(window).as_bool() {
                let _ = SetWindowPos(
                    window,
                    HWND(null_mut()),
                    0,
                    0,
                    0,
                    0,
                    SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                );
            }
        }
    }
}

/// Уничтожить окна меток (выход).
pub(crate) fn destroy(w: &OverlayWindows) {
    for hwnd in all_windows(w) {
        // SAFETY: окна свои, уничтожаются потоком, который их создал.
        unsafe {
            let _ = DestroyWindow(to_hwnd(hwnd));
        }
    }
}

fn create(atom: u16, ex: WINDOW_EX_STYLE, size: (i32, i32), alpha: u8, index: usize) -> Hwnd {
    if atom == 0 {
        return 0;
    }
    // SAFETY: класс зарегистрирован; окно создаётся скрытым за экраном; альфа ставится до показа.
    unsafe {
        let created = CreateWindowExW(
            ex,
            PCWSTR(atom as usize as *const u16),
            w!("Закладка"),
            WS_POPUP,
            -32000,
            -32000,
            size.0.max(1),
            size.1.max(1),
            HWND(null_mut()),
            HMENU(null_mut()),
            instance(),
            None,
        );
        let Ok(window) = created else { return 0 };
        if window.0.is_null() {
            return 0;
        }
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA);
        SetWindowLongPtrW(window, GWLP_USERDATA, index as isize);
        from_hwnd(window)
    }
}

/// Номер окна в его наборе (флажок 0…5, плашка 0 — вверх, 1 — вниз).
fn index_of(hwnd: Hwnd) -> usize {
    // SAFETY: в GWLP_USERDATA окон меток только номер.
    unsafe { GetWindowLongPtrW(to_hwnd(hwnd), GWLP_USERDATA).max(0) as usize }
}

fn notify(event: PieceEvent) {
    let owner = OWNER.try_with(|o| o.get()).unwrap_or(0);
    if owner == 0 {
        return;
    }
    let (wparam, lparam) = event.encode();
    // SAFETY: окно-владелец своё; ошибка (окна уже нет) не важна.
    unsafe {
        let _ = PostMessageW(to_hwnd(owner), WM_APP_PIECE, WPARAM(wparam), LPARAM(lparam));
    }
}

/// Указатель вошёл в окно: начать слежение за уходом и сообщить один раз.
fn enter(hwnd: Hwnd, event: PieceEvent) {
    let first = HOVER
        .try_with(|h| {
            let mut h = h.borrow_mut();
            if h.contains(&hwnd) {
                false
            } else {
                h.push(hwnd);
                true
            }
        })
        .unwrap_or(false);
    if !first {
        return;
    }
    let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: to_hwnd(hwnd),
        dwHoverTime: 0,
    };
    // SAFETY: структура локальная.
    if unsafe { TrackMouseEvent(&mut track) }.is_err() {
        forget_hover(hwnd); // без слежения «уход» не придёт — сообщим о входе в следующий раз
    }
    notify(event);
}

fn forget_hover(hwnd: Hwnd) -> bool {
    HOVER
        .try_with(|h| {
            let mut h = h.borrow_mut();
            let before = h.len();
            h.retain(|&x| x != hwnd);
            h.len() != before
        })
        .unwrap_or(false)
}

fn leave(hwnd: Hwnd) {
    if forget_hover(hwnd) {
        notify(PieceEvent::Leave);
    }
}

/// Залить окно цветом `bg` и (если есть) нарисовать по центру надпись.
fn paint(hwnd: Hwnd, bg: Rgb, label: Option<(&str, Rgb, HFONT)>) {
    let window = to_hwnd(hwnd);
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: обработка WM_PAINT своего окна; кисть создаётся и удаляется здесь, шрифт
    // выбирается на время рисования и возвращается.
    unsafe {
        let dc = BeginPaint(window, &mut ps);
        let mut rect = RECT::default();
        let _ = GetClientRect(window, &mut rect);
        let brush = CreateSolidBrush(COLORREF(bg.colorref()));
        if !brush.0.is_null() {
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush);
        }
        if let Some((text, fg, font)) = label {
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(fg.colorref()));
            let old = SelectObject(dc, font);
            DrawTextW(dc, &mut wide, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(dc, old);
        }
        let _ = EndPaint(window, &ps);
    }
}

/// Цвета и шрифты для рисования: (снимок, шрифт флажка, шрифт плашки).
fn style() -> Option<(Snapshot, HFONT, HFONT)> {
    STYLE
        .try_with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| (s.colors, s.flag_font.0, s.pill_font.0))))
        .ok()
        .flatten()
}

/// Общее для подсветки и полосы: заливка цветом настроек, мышь насквозь.
fn solid_proc(hwnd: Hwnd, msg: u32, pick: fn(&Snapshot) -> Rgb) -> Option<isize> {
    match msg {
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_NCHITTEST => Some(HTTRANSPARENT as isize),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            let color = style().map(|(colors, _, _)| pick(&colors)).unwrap_or(Rgb(0xFF, 0xD4, 0x00));
            paint(hwnd, color, None);
            Some(0)
        }
        _ => None,
    }
}

fn highlight_proc(hwnd: Hwnd, msg: u32, _wparam: usize, _lparam: isize) -> Option<isize> {
    solid_proc(hwnd, msg, |c| c.highlight)
}

fn bar_proc(hwnd: Hwnd, msg: u32, _wparam: usize, _lparam: isize) -> Option<isize> {
    solid_proc(hwnd, msg, |c| c.bar)
}

fn flag_proc(hwnd: Hwnd, msg: u32, _wparam: usize, _lparam: isize) -> Option<isize> {
    match msg {
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            let font = style().map(|(_, flag, _)| flag);
            paint(hwnd, FLAG_COLOR, font.map(|f| (FLAG_TEXT, Rgb(0xFF, 0xFF, 0xFF), f)));
            Some(0)
        }
        WM_MOUSEMOVE => {
            enter(hwnd, PieceEvent::FlagEnter(index_of(hwnd)));
            Some(0)
        }
        WM_MOUSELEAVE => {
            leave(hwnd);
            Some(0)
        }
        WM_LBUTTONDOWN => {
            notify(PieceEvent::FlagClick(index_of(hwnd)));
            Some(0)
        }
        WM_DESTROY => {
            forget_hover(hwnd);
            None
        }
        _ => None,
    }
}

fn pill_dir(hwnd: Hwnd) -> Dir {
    if index_of(hwnd) == 0 {
        Dir::Up
    } else {
        Dir::Down
    }
}

fn pill_proc(hwnd: Hwnd, msg: u32, _wparam: usize, _lparam: isize) -> Option<isize> {
    match msg {
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            let text = if pill_dir(hwnd) == Dir::Up { PILL_UP_TEXT } else { PILL_DOWN_TEXT };
            match style() {
                Some((colors, _, font)) => paint(hwnd, colors.bar, Some((text, colors.pill_text, font))),
                None => paint(hwnd, Rgb(0xFF, 0x7A, 0x00), None),
            }
            Some(0)
        }
        WM_MOUSEMOVE => {
            enter(hwnd, PieceEvent::PillEnter(pill_dir(hwnd)));
            Some(0)
        }
        WM_MOUSELEAVE => {
            leave(hwnd);
            Some(0)
        }
        WM_LBUTTONDOWN => {
            notify(PieceEvent::PillClick(pill_dir(hwnd)));
            Some(0)
        }
        WM_DESTROY => {
            forget_hover(hwnd);
            None
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::RwLock;

    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowRect, GWL_EXSTYLE};

    use crate::app_id::DEV;

    pub(crate) fn test_shared(dir: &str) -> Arc<Shared> {
        let mut config = crate::store::default_config(&DEV);
        config.hints = false; // в тестах подсказки на экран не выходят
        Arc::new(Shared {
            config: RwLock::new(config),
            profile: &DEV,
            data_dir: std::env::temp_dir().join(dir),
            debug: AtomicBool::new(false),
        })
    }

    #[test]
    fn sizes_follow_scale_like_python() {
        assert_eq!(flag_size(1.0), 16);
        assert_eq!(flag_size(1.25), 20);
        assert_eq!(flag_size(1.5), 24);
        assert_eq!(flag_size(2.0), 32);
        assert_eq!(flag_size(0.8), 14); // не меньше 14
        assert_eq!(flag_font_px(16), 11); // round(11.2)
        assert_eq!(flag_font_px(20), 14);
        assert_eq!(flag_font_px(24), 17); // round(16.8)
        assert_eq!(pill_font_px(1.0), 14);
        assert_eq!(pill_font_px(1.25), 18); // round(17.5) == 18 (к чётному)
        assert_eq!(pill_font_px(1.5), 21);
        assert_eq!(pill_padding(1.0), (9, 4));
        assert_eq!(pill_padding(1.25), (11, 5));
        assert_eq!(pill_padding(1.5), (14, 6)); // round(13.5) == 14
        assert_eq!(pill_size(&[(80, 19), (82, 19)], 1.0), (100, 27));
        assert_eq!(pill_size(&[], 1.0), (18, 8));
        assert_eq!(clean_scale(f64::NAN), 1.0);
        assert_eq!(clean_scale(0.5), 1.0);
        assert_eq!(clean_scale(9.0), 4.0);
    }

    #[test]
    fn alpha_like_overlay() {
        assert_eq!(alpha_byte(0.3), 76); // round(76.5) == 76
        assert_eq!(alpha_byte(0.9), 230); // round(229.5) == 230
        assert_eq!(alpha_byte(0.96), 245);
        assert_eq!(alpha_byte(0.0), 1);
        assert_eq!(alpha_byte(2.0), 255);
        assert_eq!(alpha_byte(f64::NAN), 1);
    }

    #[test]
    fn pill_text_auto_or_chosen() {
        let mut config = crate::store::default_config(&DEV);
        config.bar_color = Rgb(0x00, 0x00, 0x00);
        assert_eq!(pill_text_color(&config), Rgb(0xFF, 0xFF, 0xFF));
        config.bar_color = Rgb(0xFF, 0xD4, 0x00);
        assert_eq!(pill_text_color(&config), Rgb(0, 0, 0));
        config.label_color = LabelColor::Custom(Rgb(0x12, 0x34, 0x56));
        assert_eq!(pill_text_color(&config), Rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn piece_events_round_trip() {
        for e in [
            PieceEvent::FlagClick(0),
            PieceEvent::FlagClick(5),
            PieceEvent::PillClick(Dir::Up),
            PieceEvent::PillClick(Dir::Down),
            PieceEvent::FlagEnter(3),
            PieceEvent::PillEnter(Dir::Down),
            PieceEvent::Leave,
        ] {
            let (w, l) = e.encode();
            assert_eq!(PieceEvent::decode(w, l), Some(e));
        }
        assert_eq!(PieceEvent::decode(1, 6), None); // флажков 6
        assert_eq!(PieceEvent::decode(1, -1), None);
        assert_eq!(PieceEvent::decode(2, 2), None);
        assert_eq!(PieceEvent::decode(9, 0), None);
    }

    fn ex_style(h: Hwnd) -> u32 {
        // SAFETY: чтение стиля своего окна.
        unsafe { GetWindowLongW(to_hwnd(h), GWL_EXSTYLE) as u32 }
    }

    fn rect(h: Hwnd) -> RECT {
        let mut r = RECT::default();
        // SAFETY: окно своё, структура локальная.
        unsafe {
            let _ = GetWindowRect(to_hwnd(h), &mut r);
        }
        r
    }

    #[test]
    fn windows_are_created_hidden_off_screen() {
        let shared = test_shared("zakladka-part6-pieces");
        let w = create_overlay_windows(&shared, 1.0);
        assert_eq!((w.highlights.len(), w.bars.len(), w.flags.len()), (HIGHLIGHTS, BARS, FLAGS));
        assert_eq!(all_windows(&w).count(), HIGHLIGHTS + BARS + FLAGS + 2, "все окна созданы");
        assert!(!any_visible(&w));
        assert_eq!(w.flag_size, 16);
        let (pw, ph) = w.pill_size;
        assert!(pw > 60 && ph > 14, "{pw}x{ph}");

        let base = (WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED).0;
        for &h in w.highlights.iter().chain(&w.bars) {
            assert_eq!(ex_style(h) & (base | WS_EX_TRANSPARENT.0), base | WS_EX_TRANSPARENT.0);
        }
        for &h in w.flags.iter().chain([&w.pill_up, &w.pill_down]) {
            assert_eq!(ex_style(h) & base, base);
            assert_eq!(ex_style(h) & WS_EX_TRANSPARENT.0, 0, "флажок и плашка ловят щелчки");
        }
        let f = rect(w.flags[0]);
        assert_eq!((f.left, f.top, f.right - f.left, f.bottom - f.top), (-32000, -32000, 16, 16));
        let p = rect(w.pill_down);
        assert_eq!((p.right - p.left, p.bottom - p.top), w.pill_size);
        assert_eq!(index_of(w.flags[4]), 4);
        assert_eq!(pill_dir(w.pill_up), Dir::Up);
        assert_eq!(pill_dir(w.pill_down), Dir::Down);

        // своя копия дескрипторов, перекраска, прятание — ничего не показывают
        let kept = copy(&w);
        assert_eq!(all_windows(&kept).collect::<Vec<_>>(), all_windows(&w).collect::<Vec<_>>());
        if let Ok(mut c) = shared.config.write() {
            c.bar_color = Rgb(0x11, 0x22, 0x33);
            c.label_color = LabelColor::Custom(Rgb(1, 2, 3));
        }
        recolor(&shared, &kept);
        assert_eq!(colors().map(|c| (c.bar, c.pill_text)), Some((Rgb(0x11, 0x22, 0x33), Rgb(1, 2, 3))));
        hide_all(&kept);
        assert!(!any_visible(&kept));
        destroy(&w);
    }

    #[test]
    fn bigger_scale_gives_bigger_pieces() {
        let shared = test_shared("zakladka-part6-pieces2");
        let small = create_overlay_windows(&shared, 1.0);
        let small_pill = small.pill_size;
        destroy(&small);
        let big = create_overlay_windows(&shared, 1.5);
        assert_eq!(big.flag_size, 24);
        assert!(big.pill_size.0 > small_pill.0 && big.pill_size.1 > small_pill.1);
        destroy(&big);
    }
}
