//! Рисование GDI (часть 7; N 289–376, 522–630): тема, палитра, шрифты, холст, кнопки, классы окон.
//! Ею пользуются части 6 и 8.

use std::ffi::c_void;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr::null_mut;
use std::sync::{OnceLock, RwLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    BOOL, COLORREF, ERROR_SUCCESS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, DeleteDC, DeleteObject, DrawTextW,
    FillRect, GdiFlush, GetStockObject, GetTextExtentPoint32W, GetTextFaceW, GetTextMetricsW, LineTo, MoveToEx,
    Polygon, Rectangle, RoundRect, SelectObject, SetBkMode, SetDCBrushColor, SetDCPenColor, SetTextColor,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DC_BRUSH, DC_PEN,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOCLIP,
    DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HBITMAP,
    HBRUSH, HDC, HFONT, HGDIOBJ, NULL_BRUSH, NULL_PEN, OUT_DEFAULT_PRECIS, PS_INSIDEFRAME, SRCCOPY, TEXTMETRICW,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClassLongPtrW, LoadCursorW, RegisterClassExW, GCW_ATOM, IDC_ARROW, IDC_HAND, WNDCLASSEXW,
};

use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect, Rgb};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    pub muted: Rgb,
    pub line: Rgb,
    pub border: Rgb,
    pub hover: Rgb,
    pub pressed: Rgb,
    pub danger: Rgb,
    pub button: Rgb,
    pub button_hover: Rgb,
    pub primary: Rgb,
    pub primary_hover: Rgb,
    pub primary_fg: Rgb,
}

/// Палитра светлой или тёмной темы (N 302–313). `pressed` = `mix(hover, fg, 0.06)`, как нажатая
/// кнопка в N 557–558.
pub fn palette(light: bool) -> Palette {
    if light {
        Palette {
            bg: Rgb(0xF9, 0xF9, 0xF9),
            fg: Rgb(0x1A, 0x1A, 0x1A),
            muted: Rgb(0x5F, 0x5F, 0x5F),
            line: Rgb(0xE3, 0xE3, 0xE3),
            border: Rgb(0xCF, 0xCF, 0xCF),
            hover: Rgb(0xEA, 0xEA, 0xEA),
            pressed: Rgb(0xDE, 0xDE, 0xDE),
            danger: Rgb(0xC4, 0x2B, 0x1C),
            button: Rgb(0xED, 0xED, 0xED),
            button_hover: Rgb(0xE2, 0xE2, 0xE2),
            primary: Rgb(0x00, 0x5F, 0xB8),
            primary_hover: Rgb(0x1A, 0x6F, 0xC2),
            primary_fg: Rgb(0xFF, 0xFF, 0xFF),
        }
    } else {
        Palette {
            bg: Rgb(0x2C, 0x2C, 0x2C),
            fg: Rgb(0xFF, 0xFF, 0xFF),
            muted: Rgb(0xAB, 0xAB, 0xAB),
            line: Rgb(0x3F, 0x3F, 0x3F),
            border: Rgb(0x4B, 0x4B, 0x4B),
            hover: Rgb(0x3D, 0x3D, 0x3D),
            pressed: Rgb(0x49, 0x49, 0x49),
            danger: Rgb(0xFF, 0x99, 0xA4),
            button: Rgb(0x3A, 0x3A, 0x3A),
            button_hover: Rgb(0x46, 0x46, 0x46),
            primary: Rgb(0x60, 0xCD, 0xFF),
            primary_hover: Rgb(0x7A, 0xD6, 0xFF),
            primary_fg: Rgb(0x00, 0x00, 0x00),
        }
    }
}

/// Цвета Python-палитры, которых нет в `Palette` (N 304–313): `faint` (бледный текст, полоса
/// прокрутки, рамка наведения образца) и `tag` (фон метки «закрыто»).
pub(crate) fn faint(light: bool) -> Rgb {
    if light {
        Rgb(0x9A, 0x9A, 0x9A)
    } else {
        Rgb(0x7A, 0x7A, 0x7A)
    }
}

pub(crate) fn tag_fill(light: bool) -> Rgb {
    if light {
        Rgb(0xEB, 0xEB, 0xEB)
    } else {
        Rgb(0x3A, 0x3A, 0x3A)
    }
}

/// Светлая ли палитра (по яркости фона).
pub(crate) fn is_light(p: &Palette) -> bool {
    p.bg.0 as u32 + p.bg.1 as u32 + p.bg.2 as u32 > 3 * 128
}

/// `color`, сдвинутый к `other` на долю `share` (0…1), округление как `round()` в Python.
/// Приватная копия `core::color::mix` (часть 7 не зависит от готовности части 1).
pub(crate) fn mix(color: Rgb, other: Rgb, share: f64) -> Rgb {
    let share = if share.is_nan() { 0.0 } else { share.clamp(0.0, 1.0) };
    let channel = |a: u8, b: u8| {
        let v = a as f64 + (b as f64 - a as f64) * share;
        v.round_ties_even().clamp(0.0, 255.0) as u8
    };
    Rgb(channel(color.0, other.0), channel(color.1, other.1), channel(color.2, other.2))
}

/// Флаг светлой темы Windows (N 289–299): панели задач (`system = true`, для панели у трея) или
/// приложений. Нет значения в реестре — светлая.
pub fn light_theme(system: bool) -> bool {
    let name = if system { w!("SystemUsesLightTheme") } else { w!("AppsUseLightTheme") };
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: буфер — локальное u32, размер передан верно.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    if status == ERROR_SUCCESS {
        value != 0
    } else {
        true
    }
}

/// `max(1, round(value * scale))`, округление как в Python (банковское).
pub fn px(value: f64, scale: f64) -> i32 {
    let v = (value * scale).round_ties_even();
    if v.is_nan() || v < 1.0 {
        1
    } else if v > i32::MAX as f64 {
        i32::MAX
    } else {
        v as i32
    }
}

/// Windows 11 (сборка 22000+) сама рисует скруглённые углы и рамку всплывающих окон (N 49).
pub(crate) fn rounded_corners() -> bool {
    static ROUNDED: OnceLock<bool> = OnceLock::new();
    *ROUNDED.get_or_init(|| windows_build() >= 22000)
}

fn windows_build() -> u32 {
    let mut buffer = [0u16; 32];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: буфер локальный, размер в байтах передан верно.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return 0;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().parse().unwrap_or(0)
}

/// Строка UTF-16 с завершающим нулём.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Дескриптор модуля программы для классов и окон.
pub(crate) fn instance() -> HINSTANCE {
    // SAFETY: GetModuleHandleW(NULL) — дескриптор exe, освобождать не нужно.
    unsafe { GetModuleHandleW(PCWSTR::null()).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Шрифт GDI. Drop (DeleteObject) добавляет часть 7.
pub struct Font(HFONT);

impl Font {
    /// `px` — высота символов в пикселях (как `size=-px` в Tk). `semibold` для «Segoe UI» —
    /// гарнитура «Segoe UI Semibold», если она есть, иначе полужирный «Segoe UI» (N 332–344).
    pub fn new(family: &str, px: i32, semibold: bool) -> Font {
        let (face, weight) = if semibold && family.eq_ignore_ascii_case("Segoe UI") {
            if semibold_face() {
                ("Segoe UI Semibold", FW_NORMAL)
            } else {
                ("Segoe UI", FW_BOLD)
            }
        } else if semibold {
            (family, FW_SEMIBOLD)
        } else {
            (family, FW_NORMAL)
        };
        Font(create_font(face, px.max(1), weight.0 as i32))
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            // SAFETY: шрифт создан нами и больше нигде не выбран (Canvas возвращает прежний).
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

fn create_font(face: &str, px: i32, weight: i32) -> HFONT {
    let name = wide(face);
    // SAFETY: имя — строка с нулём, живёт до конца вызова.
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

/// Есть ли гарнитура «Segoe UI Semibold» (один раз на процесс, как `_SEMIBOLD` в N 329–335).
fn semibold_face() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let font = create_font("Segoe UI Semibold", 16, FW_NORMAL.0 as i32);
        if font.0.is_null() {
            return false;
        }
        let mut name = [0u16; 64];
        // SAFETY: DC и шрифт свои; прежний объект DC возвращается до удаления.
        unsafe {
            let dc = CreateCompatibleDC(HDC(null_mut()));
            if !dc.0.is_null() {
                let old = SelectObject(dc, font);
                GetTextFaceW(dc, Some(&mut name));
                SelectObject(dc, old);
                let _ = DeleteDC(dc);
            }
            let _ = DeleteObject(font);
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case("Segoe UI Semibold")
    })
}

/// Memory DC + DIB. Drop добавляет часть 7.
pub struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8, // BGRA сверху вниз, width × height × 4; null, если DIB не создался
    width: i32,
    height: i32,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: DC и DIB созданы в `new`; DIB сначала выводится из DC, затем удаляется.
        unsafe {
            if !self.dc.0.is_null() {
                if !self.old.0.is_null() {
                    SelectObject(self.dc, self.old);
                }
                let _ = DeleteDC(self.dc);
            }
            if !self.bitmap.0.is_null() {
                let _ = DeleteObject(self.bitmap);
            }
        }
    }
}

impl Canvas {
    /// Холст в памяти (32 бита, сверху вниз). Размер меньше 1 становится 1.
    pub fn new(width: i32, height: i32) -> Canvas {
        let (width, height) = (width.max(1), height.max(1));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        // SAFETY: структуры локальные; при ошибке дескрипторы остаются нулевыми, Drop это учитывает.
        unsafe {
            let dc = CreateCompatibleDC(HDC(null_mut()));
            let bitmap = if dc.0.is_null() {
                HBITMAP(null_mut())
            } else {
                CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE(null_mut()), 0).unwrap_or_default()
            };
            let old = if bitmap.0.is_null() { HGDIOBJ(null_mut()) } else { SelectObject(dc, bitmap) };
            if bitmap.0.is_null() {
                bits = null_mut();
            }
            if !dc.0.is_null() {
                SetBkMode(dc, TRANSPARENT);
            }
            Canvas { dc, bitmap, old, bits: bits as *mut u8, width, height }
        }
    }

    pub fn fill(&mut self, r: Rect, c: Rgb) {
        if r.r <= r.l || r.b <= r.t {
            return;
        }
        let rect = RECT::from(r);
        // SAFETY: DC свой; DC_BRUSH — стоковый объект, удалять не нужно.
        unsafe {
            SetDCBrushColor(self.dc, COLORREF(c.colorref()));
            FillRect(self.dc, &rect, HBRUSH(GetStockObject(DC_BRUSH).0));
        }
    }

    /// Скруглённый прямоугольник (N 367–374): `r` без правого и нижнего края, как `fill`.
    pub fn round_rect(&mut self, r: Rect, radius: i32, fill: Rgb, outline: Option<Rgb>) {
        let (w, h) = (r.r - r.l, r.b - r.t);
        if w <= 0 || h <= 0 {
            return;
        }
        let radius = radius.clamp(0, w.min(h) / 2);
        self.with_tools(Some(fill), Some(outline.unwrap_or(fill)), |dc| {
            // SAFETY: DC свой, кисть и перо выбраны в with_tools.
            unsafe {
                if radius == 0 {
                    let _ = Rectangle(dc, r.l, r.t, r.r, r.b);
                } else {
                    let _ = RoundRect(dc, r.l, r.t, r.r, r.b, 2 * radius, 2 * radius);
                }
            }
        });
    }

    pub fn polygon(&mut self, points: &[Point], fill: Rgb) {
        if points.len() < 3 {
            return;
        }
        let points: Vec<POINT> = points.iter().map(|&p| POINT::from(p)).collect();
        self.with_tools(Some(fill), Some(fill), |dc| {
            // SAFETY: DC свой, массив точек живёт до конца вызова.
            unsafe {
                let _ = Polygon(dc, &points);
            }
        });
    }

    pub fn line(&mut self, a: Point, b: Point, c: Rgb) {
        self.with_tools(None, Some(c), |dc| {
            // SAFETY: DC свой.
            unsafe {
                let _ = MoveToEx(dc, a.x, a.y, None);
                let _ = LineTo(dc, b.x, b.y);
            }
        });
    }

    /// Одна строка, по вертикали — по центру `r`; без обрезки по `r` (текст заранее укорачивают
    /// `fit_text`).
    pub fn text(&mut self, r: Rect, s: &str, f: &Font, c: Rgb, align: Align) {
        let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_NOCLIP | align_flag(align);
        self.draw_text(r, s, f, c, flags);
    }

    /// (ширина, высота строки) текста шрифтом `f`.
    pub fn measure(&self, s: &str, f: &Font) -> (i32, i32) {
        let mut size = SIZE::default();
        let mut metrics = TEXTMETRICW::default();
        let text: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: DC свой; прежний шрифт возвращается.
        unsafe {
            let old = SelectObject(self.dc, f.0);
            let _ = GetTextMetricsW(self.dc, &mut metrics);
            if !text.is_empty() {
                let _ = GetTextExtentPoint32W(self.dc, &text, &mut size);
            }
            SelectObject(self.dc, old);
        }
        (size.cx, metrics.tmHeight)
    }

    /// `hdc` — HDC окна как число (из BeginPaint).
    pub fn present(&self, hdc: isize, x: i32, y: i32) {
        if hdc == 0 {
            return;
        }
        // SAFETY: hdc дан вызывающим (BeginPaint), источник — свой DC.
        unsafe {
            let _ = BitBlt(HDC(hdc as *mut c_void), x, y, self.width, self.height, self.dc, 0, 0, SRCCOPY);
        }
    }

    /// (ширина, высота) холста.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Цвет точки (для тестов и сверки вида); вне холста — `None`.
    pub fn pixel(&self, x: i32, y: i32) -> Option<Rgb> {
        if self.bits.is_null() || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        // SAFETY: GdiFlush завершает рисование GDI; индекс внутри буфера width × height × 4.
        unsafe {
            let _ = GdiFlush();
            let at = self.bits.add(((y * self.width + x) * 4) as usize);
            Some(Rgb(*at.add(2), *at.add(1), *at))
        }
    }

    /// Сохранить картинку в BMP (24 бита) — для сверки вида со снимком Python-версии.
    pub fn save_bmp(&self, path: &Path) -> std::io::Result<()> {
        if self.bits.is_null() {
            return Err(std::io::Error::other("no bitmap"));
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = (w * 3 + 3) & !3;
        let image = stride * h;
        let mut out = Vec::with_capacity(54 + image);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + image) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes()); // снизу вверх
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(image as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        // SAFETY: буфер DIB width × height × 4 живёт, пока жив холст.
        let pixels = unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.bits, w * h * 4)
        };
        for y in (0..h).rev() {
            let row = &pixels[y * w * 4..(y + 1) * w * 4];
            for px in row.chunks_exact(4) {
                out.extend_from_slice(&px[..3]);
            }
            out.resize(out.len() + (stride - w * 3), 0);
        }
        std::fs::File::create(path)?.write_all(&out)
    }

    /// Текст с переносом по словам в `r` (подсказки, пустая панель).
    pub(crate) fn text_wrapped(&mut self, r: Rect, s: &str, f: &Font, c: Rgb) {
        self.draw_text(r, s, f, c, DT_WORDBREAK | DT_NOPREFIX | DT_NOCLIP | DT_LEFT);
    }

    /// (ширина, высота) текста с переносом по словам при ширине `width`.
    pub(crate) fn measure_wrapped(&self, s: &str, f: &Font, width: i32) -> (i32, i32) {
        let mut text: Vec<u16> = s.encode_utf16().collect();
        if text.is_empty() {
            return (0, self.measure("", f).1);
        }
        let mut rect = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
        // SAFETY: DC свой; DT_CALCRECT только меряет.
        unsafe {
            let old = SelectObject(self.dc, f.0);
            DrawTextW(self.dc, &mut text, &mut rect, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX | DT_LEFT);
            SelectObject(self.dc, old);
        }
        (rect.right - rect.left, rect.bottom - rect.top)
    }

    /// Рамка скруглённого прямоугольника толщиной `width` внутрь `r` (кольцо выбранной кнопки и
    /// образца цвета, N 560–562, 615–617).
    pub(crate) fn round_frame(&mut self, r: Rect, radius: i32, color: Rgb, width: i32) {
        let (w, h) = (r.r - r.l, r.b - r.t);
        if w <= 0 || h <= 0 {
            return;
        }
        let radius = radius.clamp(0, w.min(h) / 2);
        // SAFETY: перо создаётся, выбирается, возвращается и удаляется здесь же.
        unsafe {
            let pen = CreatePen(PS_INSIDEFRAME, width.max(1), COLORREF(color.colorref()));
            if pen.0.is_null() {
                return;
            }
            let old_pen = SelectObject(self.dc, pen);
            let old_brush = SelectObject(self.dc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(self.dc, r.l, r.t, r.r, r.b, 2 * radius, 2 * radius);
            SelectObject(self.dc, old_brush);
            SelectObject(self.dc, old_pen);
            let _ = DeleteObject(pen);
        }
    }

    fn draw_text(&mut self, r: Rect, s: &str, f: &Font, c: Rgb, flags: DRAW_TEXT_FORMAT) {
        let mut text: Vec<u16> = s.encode_utf16().collect();
        if text.is_empty() {
            return;
        }
        let mut rect = RECT::from(r);
        // SAFETY: DC свой; прежний шрифт возвращается.
        unsafe {
            let old = SelectObject(self.dc, f.0);
            SetTextColor(self.dc, COLORREF(c.colorref()));
            DrawTextW(self.dc, &mut text, &mut rect, flags);
            SelectObject(self.dc, old);
        }
    }

    /// Выбрать кисть и перо цвета DC (стоковые объекты: создавать и удалять нечего), нарисовать,
    /// вернуть прежние.
    fn with_tools(&mut self, brush: Option<Rgb>, pen: Option<Rgb>, draw: impl FnOnce(HDC)) {
        // SAFETY: DC свой; стоковые объекты не удаляются.
        unsafe {
            let brush = match brush {
                Some(c) => {
                    SetDCBrushColor(self.dc, COLORREF(c.colorref()));
                    GetStockObject(DC_BRUSH)
                }
                None => GetStockObject(NULL_BRUSH),
            };
            let pen = match pen {
                Some(c) => {
                    SetDCPenColor(self.dc, COLORREF(c.colorref()));
                    GetStockObject(DC_PEN)
                }
                None => GetStockObject(NULL_PEN),
            };
            let old_brush = SelectObject(self.dc, brush);
            let old_pen = SelectObject(self.dc, pen);
            draw(self.dc);
            SelectObject(self.dc, old_pen);
            SelectObject(self.dc, old_brush);
        }
    }
}

fn align_flag(align: Align) -> DRAW_TEXT_FORMAT {
    match align {
        Align::Left => DT_LEFT,
        Align::Center => DT_CENTER,
        Align::Right => DT_RIGHT,
    }
}

/// Текст с многоточием по ширине (N 142–155).
pub fn fit_text(measure: &dyn Fn(&str) -> i32, text: &str, width: i32) -> String {
    if width <= 0 {
        return String::new();
    }
    if measure(text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |count: usize| {
        let head: String = chars[..count].iter().collect();
        format!("{}…", head.trim_end())
    };
    let (mut low, mut high) = (0usize, chars.len());
    while low < high {
        let middle = (low + high + 1) / 2;
        if measure(&cut(middle)) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    cut(low)
}

/// Скруглённые углы Windows 11 и рамка всплывающего окна (DwmSetWindowAttribute 33).
/// Цвет рамки (атрибут 34) — `border` палитры темы панели задач, как у панели в N 692.
/// На Windows 10 ничего не делает: там рамку рисует само окно.
pub fn style_popup(hwnd: Hwnd) {
    if hwnd == 0 || !rounded_corners() {
        return;
    }
    let corner = DWMWCP_ROUND;
    let border = palette(light_theme(true)).border.colorref();
    // SAFETY: значения локальные, размеры верные; ошибка DWM не важна.
    unsafe {
        let _ = DwmSetWindowAttribute(
            to_hwnd(hwnd),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const c_void,
            std::mem::size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            to_hwnd(hwnd),
            DWMWA_BORDER_COLOR,
            &border as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

/// DwmSetWindowAttribute 20 (на ранних сборках Windows 10 — 19).
pub fn dark_title_bar(hwnd: Hwnd, dark: bool) {
    if hwnd == 0 {
        return;
    }
    let value = BOOL::from(dark);
    let size = std::mem::size_of::<BOOL>() as u32;
    // SAFETY: значение локальное, размер верный.
    unsafe {
        let pointer = &value as *const BOOL as *const c_void;
        if DwmSetWindowAttribute(to_hwnd(hwnd), DWMWA_USE_IMMERSIVE_DARK_MODE, pointer, size).is_err() {
            let _ = DwmSetWindowAttribute(to_hwnd(hwnd), DWMWINDOWATTRIBUTE(19), pointer, size);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ButtonSpec<'a> {
    pub rect: Rect,
    pub caption: &'a str,
    pub hover: bool,
    pub pressed: bool,
    pub selected: bool,
    pub fill: Option<Rgb>,
    pub hover_fill: Rgb,
    pub fg: Rgb,
    pub align: Align,
}

/// Плоская кнопка или пункт меню (N 522–566): скруглённое наведение, многоточие, поле 10.
/// Нажатая (под курсором) — заливка сдвинута на 6 % к цвету надписи `fg`; кольцо выбранной —
/// цветом `fg` (в Python — цвет текста палитры, у таких кнопок он и есть `fg`).
pub fn draw_button(c: &mut Canvas, b: &ButtonSpec, f: &Font, scale: f64) {
    let padx = px(10.0, scale);
    let radius = px(5.0, scale);
    let ring = px(1.5, scale);
    let r = b.rect;
    let mut fill = if b.hover { Some(b.hover_fill) } else { b.fill };
    if b.pressed && b.hover {
        fill = fill.map(|x| mix(x, b.fg, 0.06));
    }
    let shape = Rect { l: r.l + 1, t: r.t + 1, r: r.r - 1, b: r.b - 1 };
    if let Some(fill) = fill {
        c.round_rect(shape, radius, fill, None);
    }
    if b.selected {
        c.round_frame(shape, radius, b.fg, ring);
    }
    let caption = {
        let canvas = &*c;
        fit_text(&|s| canvas.measure(s, f).0, b.caption, r.r - r.l - 2 * padx)
    };
    let area = Rect { l: r.l + padx, t: r.t, r: r.r - padx, b: r.b };
    c.text(area, &caption, f, b.fg, b.align);
}

/// Образец цвета (N 590–619): кольцо у выбранного, бледное кольцо при наведении.
pub fn draw_swatch(c: &mut Canvas, r: Rect, color: Rgb, selected: bool, hover: bool, p: &Palette, scale: f64) {
    let ring = px(2.0, scale);
    let gap = px(4.0, scale);
    if selected || hover {
        let shape = Rect { l: r.l + 1, t: r.t + 1, r: r.r - 1, b: r.b - 1 };
        let outline = if selected { p.fg } else { faint(is_light(p)) };
        c.round_rect(shape, px(7.0, scale), p.bg, None);
        c.round_frame(shape, px(7.0, scale), outline, ring);
    }
    let inner = Rect { l: r.l + gap, t: r.t + gap, r: r.r - gap, b: r.b - gap };
    c.round_rect(inner, px(4.0, scale), color, Some(mix(color, Rgb(0, 0, 0), 0.25)));
}

/// Оконная процедура программы: безопасная Rust-функция, без `extern "system"`.
/// Возвращает `Some(результат)`, если сообщение обработано, `None` — передать в `DefWindowProcW`.
/// `register_class` ставит общий `extern "system"` переходник, который находит эту функцию для
/// класса, вызывает её внутри `catch_unwind` (паника -> `DefWindowProcW`; место паники пишет
/// panic hook) и не даёт панике выйти через границу FFI.
pub type WndProc = fn(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize>;

/// Обёртка с catch_unwind; возвращает ATOM класса.
/// Повторный вызов с тем же именем не регистрирует класс заново: возвращает прежний ATOM и
/// ставит новую `proc`. 0 — ошибка регистрации.
pub fn register_class(name: &str, proc: WndProc, cursor_hand: bool) -> u16 {
    let Ok(mut classes) = CLASSES.write() else {
        return 0;
    };
    if let Some(entry) = classes.iter_mut().find(|e| e.name == name) {
        entry.proc = proc;
        return entry.atom;
    }
    let class_name = wide(name);
    // SAFETY: структура и имя живут до конца вызова RegisterClassExW (имя система копирует).
    let atom = unsafe {
        let cursor = LoadCursorW(HINSTANCE(null_mut()), if cursor_hand { IDC_HAND } else { IDC_ARROW })
            .unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(trampoline),
            hInstance: instance(),
            hCursor: cursor,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&class)
    };
    if atom != 0 {
        classes.push(ClassEntry { name: name.to_string(), atom, proc });
    }
    atom
}

struct ClassEntry {
    name: String,
    atom: u16,
    proc: WndProc,
}

/// Классы программы: ATOM -> оконная процедура (классы общие для процесса, поэтому не thread_local).
static CLASSES: RwLock<Vec<ClassEntry>> = RwLock::new(Vec::new());

/// Общий переходник всех классов программы: находит `WndProc` по ATOM класса и вызывает её в
/// `catch_unwind` (паника не выходит за границу FFI; место паники пишет panic hook).
unsafe extern "system" fn trampoline(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let atom = GetClassLongPtrW(hwnd, GCW_ATOM) as u16;
    let proc = CLASSES
        .read()
        .ok()
        .and_then(|classes| classes.iter().find(|e| e.atom == atom).map(|e| e.proc));
    if let Some(proc) = proc {
        let handle = from_hwnd(hwnd);
        if let Ok(Some(result)) = catch_unwind(AssertUnwindSafe(|| proc(handle, msg, wparam.0, lparam.0))) {
            return LRESULT(result);
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_rounds_like_python() {
        assert_eq!(px(1.5, 1.0), 2);
        assert_eq!(px(2.5, 1.0), 2); // round(2.5) == 2 в Python
        assert_eq!(px(380.0, 1.25), 475);
        assert_eq!(px(0.2, 1.0), 1);
        assert_eq!(px(10.0, 1.5), 15);
    }

    #[test]
    fn palette_pressed_is_mixed_hover() {
        for light in [true, false] {
            let p = palette(light);
            assert_eq!(p.pressed, mix(p.hover, p.fg, 0.06));
            assert_eq!(is_light(&p), light);
        }
    }

    #[test]
    fn mix_matches_python() {
        assert_eq!(mix(Rgb(0, 0, 0), Rgb(255, 255, 255), 0.5), Rgb(0x80, 0x80, 0x80));
        assert_eq!(mix(Rgb(0xFF, 0, 0), Rgb(0, 0, 0xFF), 2.0), Rgb(0, 0, 0xFF));
        assert_eq!(mix(Rgb(0x20, 0x40, 0x60), Rgb(255, 255, 255), 0.25), Rgb(0x58, 0x70, 0x88));
    }

    #[test]
    fn fit_text_cuts_with_ellipsis() {
        let measure = |s: &str| s.chars().count() as i32 * 7;
        assert_eq!(fit_text(&measure, "коротко", 500), "коротко");
        assert_eq!(fit_text(&measure, "текст", 0), "");
        let cut = fit_text(&measure, &"очень длинный текст ".repeat(10), 120);
        assert!(cut.ends_with('…'));
        assert!(measure(&cut) <= 120);
        assert!(!cut.trim_end_matches('…').ends_with(' '));
        assert_eq!(fit_text(&measure, "абвгд", 3), "…");
    }

    #[test]
    fn canvas_draws_and_measures() {
        let mut c = Canvas::new(40, 20);
        c.fill(Rect { l: 0, t: 0, r: 40, b: 20 }, Rgb(10, 20, 30));
        assert_eq!(c.pixel(5, 5), Some(Rgb(10, 20, 30)));
        c.round_rect(Rect { l: 10, t: 2, r: 30, b: 18 }, 4, Rgb(200, 100, 50), None);
        assert_eq!(c.pixel(20, 10), Some(Rgb(200, 100, 50)));
        assert_eq!(c.pixel(10, 2), Some(Rgb(10, 20, 30))); // угол скруглён
        assert_eq!(c.pixel(40, 0), None);
        let font = Font::new("Segoe UI", 13, false);
        let (w, h) = c.measure("Закладка", &font);
        assert!(w > 30 && h >= 13, "{w}x{h}");
        let bold = Font::new("Segoe UI", 13, true);
        assert!(c.measure("Закладка", &bold).0 >= w);
        let (ww, wh) = c.measure_wrapped(&"слово ".repeat(20), &font, 100);
        assert!(ww <= 100 && wh > 3 * h, "{ww}x{wh}");
    }
}
