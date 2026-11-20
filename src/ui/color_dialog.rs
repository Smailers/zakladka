//! Окно цветов «Закладка — цвет» (часть 8; N 1132–1398). Рисование — через `ui::gfx`.
//!
//! Одно обычное окно с заголовком, поверх всех, без изменения размера; всё внутри рисуется GDI в
//! `Canvas` (двойная буферизация). Значения — в `ColorModel` (без окон), раскладка — `Layout`
//! (без окон), внешнее — `DialogEnv` (в тестах подставное: окно остаётся скрытым, фокус не берётся,
//! системное окно выбора цвета не открывается). Окно создаётся в `open` и уничтожается при
//! закрытии; `Drop` ручки тоже закрывает окно. `on_save` вызывается после того, как окно уже
//! уничтожено и состояние отпущено.

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, ClientToScreen, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_ENABLEHOOK, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyIcon, DestroyWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, SendMessageW,
    SetWindowLongPtrW, SetWindowTextW, GWLP_USERDATA, HICON, HMENU, ICON_BIG, ICON_SMALL, WM_CAPTURECHANGED,
    WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_INITDIALOG, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_SETICON, WS_CAPTION, WS_EX_TOPMOST, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU,
};

use crate::core::color::{mix, Colors, OPACITY_RANGE, PRESETS};
use crate::types::{from_hwnd, to_hwnd, Hwnd, Point, Rect, Rgb};
use crate::ui::color_model::{
    ColorModel, Field, LabelChoice, Pick, PREVIEW_TEXT, TIP_OTHER, TIP_PIPETTE, TIP_RESET, TIP_SLIDER,
};
use crate::ui::eyedropper::{Eyedropper, ScreenEnv};
use crate::ui::gfx::{
    dark_title_bar, draw_button, draw_swatch, instance, is_light, light_theme, palette, px, register_class, wide,
    Align, ButtonSpec, Canvas, Font, Palette,
};
use crate::ui::panel_model::DELETE_COLOR;
use crate::ui::tooltip::Tip;
use crate::win::Api;

/// Окружение окна цветов: экран (`ScreenEnv`, его же получает пипетка) и то, что нужно только
/// окну. Настоящее — `Api`.
pub trait DialogEnv: ScreenEnv {
    /// Светлая тема приложений (AppsUseLightTheme, N 1145).
    fn light(&self) -> bool;
    /// Системное окно выбора цвета, модальное к `owner`; `None` — отмена.
    fn choose_color(&self, owner: Hwnd, start: Rgb, title: &str) -> Option<Rgb>;
    /// Значок окна (HICON как число; окно цветов освобождает его при закрытии).
    fn icon(&self, tint: Rgb) -> Option<isize>;
}

impl DialogEnv for Api {
    fn light(&self) -> bool {
        light_theme(false)
    }

    fn choose_color(&self, owner: Hwnd, start: Rgb, title: &str) -> Option<Rgb> {
        choose_color(owner, start, title)
    }

    fn icon(&self, tint: Rgb) -> Option<isize> {
        self.make_icon(32, tint)
    }
}

thread_local! {
    /// 16 «дополнительных цветов» системного окна — живут, пока работает программа.
    static CUSTOM_COLORS: RefCell<[COLORREF; 16]> = const { RefCell::new([COLORREF(0x00FF_FFFF); 16]) };
}

/// `ChooseColorW` (как `colorchooser.askcolor` Tk): полное окно, начальный цвет, свой заголовок
/// (ставит перехватчик на WM_INITDIALOG).
pub fn choose_color(owner: Hwnd, start: Rgb, title: &str) -> Option<Rgb> {
    let title = wide(title);
    let mut custom = CUSTOM_COLORS
        .try_with(|c| c.try_borrow().map(|v| *v).ok())
        .ok()
        .flatten()
        .unwrap_or([COLORREF(0x00FF_FFFF); 16]);
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: to_hwnd(owner),
        rgbResult: COLORREF(start.colorref()),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN | CC_ENABLEHOOK,
        lCustData: LPARAM(title.as_ptr() as isize),
        lpfnHook: Some(choose_hook),
        ..Default::default()
    };
    // SAFETY: структура, массив цветов и заголовок живут до конца модального вызова.
    let chosen = unsafe { ChooseColorW(&mut cc) }.as_bool();
    let _ = CUSTOM_COLORS.try_with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            *c = custom;
        }
    });
    if !chosen {
        return None;
    }
    let v = cc.rgbResult.0;
    Some(Rgb((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8))
}

/// Перехватчик ChooseColorW: заголовок окна из `lCustData` (строка с нулём).
unsafe extern "system" fn choose_hook(dialog: HWND, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> usize {
    if msg == WM_INITDIALOG {
        let cc = lparam.0 as *const CHOOSECOLORW;
        if !cc.is_null() {
            let title = (*cc).lCustData.0 as *const u16;
            if !title.is_null() {
                let _ = SetWindowTextW(dialog, PCWSTR(title));
            }
        }
    }
    0
}

// ------------------------------------------------------------ шрифты и раскладка (без окон)

/// Шрифты окна (N 1146–1148).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Heading,
    Text,
    Small,
    Sample,
    Plate,
    Symbol,
}

impl Style {
    pub const ALL: [Style; 6] = [Style::Heading, Style::Text, Style::Small, Style::Sample, Style::Plate, Style::Symbol];

    /// (px при 96 DPI, полужирный, гарнитура).
    pub fn spec(self) -> (f64, bool, &'static str) {
        match self {
            Style::Heading => (14.0, true, "Segoe UI"),
            Style::Text => (13.0, false, "Segoe UI"),
            Style::Small => (12.0, false, "Segoe UI"),
            Style::Sample => (14.0, false, "Segoe UI"),
            Style::Plate => (13.0, true, "Segoe UI"),
            Style::Symbol => (11.0, false, "Segoe UI Symbol"),
        }
    }
}

/// Шрифты одного масштаба + холст 1×1 для измерений.
pub struct DialogFonts {
    scale: f64,
    fonts: Vec<Font>,
    probe: Canvas,
}

impl DialogFonts {
    pub fn new(scale: f64) -> DialogFonts {
        let scale = clean_scale(scale);
        let fonts = Style::ALL
            .iter()
            .map(|style| {
                let (size, semibold, family) = style.spec();
                Font::new(family, px(size, scale), semibold)
            })
            .collect();
        DialogFonts { scale, fonts, probe: Canvas::new(1, 1) }
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn font(&self, style: Style) -> &Font {
        &self.fonts[style as usize]
    }

    pub fn width(&self, style: Style, s: &str) -> i32 {
        self.probe.measure(s, self.font(style)).0
    }

    /// Высота строки (`linespace` Tk).
    pub fn line(&self, style: Style) -> i32 {
        self.probe.measure("", self.font(style)).1
    }
}

fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(1.0, 4.0)
    } else {
        1.0
    }
}

/// Часть окна под указателем.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    /// Готовый цвет (номер в `PRESETS`).
    Swatch(Field, usize),
    /// Свой цвет (только показывает; щелчок ничего не делает, как в N 1286).
    Custom(Field),
    /// «Другой…» — системное окно выбора цвета.
    Other(Field),
    Pipette(Field),
    Slider,
    Label(LabelChoice),
    Reset,
    Cancel,
    Save,
}

/// Ряд образцов: 8 готовых, свой (если цвет не из готовых), «Другой…», «Пипетка» (N 1276–1296).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SwatchRow {
    pub presets: [Rect; 8],
    pub custom: Option<Rect>,
    pub other: Rect,
    pub pipette: Rect,
}

/// Подпись: место, текст, шрифт, приглушённая ли (подсказки под заголовками).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Caption {
    pub r: Rect,
    pub text: &'static str,
    pub style: Style,
    pub muted: bool,
}

/// Раскладка окна в клиентских координатах (как `pack` в N 1160–1217).
#[derive(Clone, PartialEq, Debug)]
pub struct Layout {
    /// Размер клиентской области.
    pub size: (i32, i32),
    pub captions: Vec<Caption>,
    pub fill: SwatchRow,
    pub accent: SwatchRow,
    /// Желоб ползунка «Плотность» (длина 220, толщина 10).
    pub trough: Rect,
    /// Где ползунок ловит мышь (вся строка по высоте).
    pub slider_hit: Rect,
    pub opacity_label: Rect,
    pub labels: [Rect; 4],
    pub contrast: Rect,
    pub preview: Rect,
    pub reset: Rect,
    pub cancel: Rect,
    pub save: Rect,
    scale: f64,
}

/// Поле Tk-подписи по каждой стороне: рамка 2 + отступ 1 (px при 96 DPI).
const LABEL_PAD: f64 = 3.0;
const SLIDER_LENGTH: f64 = 220.0;
const SLIDER_WIDTH: f64 = 10.0;
const KNOB: f64 = 20.0;

/// Сборка раскладки сверху вниз (как `pack` с отступами `pady`).
struct Builder<'a> {
    fonts: &'a DialogFonts,
    scale: f64,
    left: i32,
    pad: i32,
    y: i32,
    widths: Vec<i32>,
    captions: Vec<Caption>,
}

impl Builder<'_> {
    fn s(&self, v: f64) -> i32 {
        px(v, self.scale)
    }

    /// Подпись строкой во всю высоту `height` (или по высоте шрифта).
    fn caption(&mut self, text: &'static str, style: Style, muted: bool, height: Option<i32>) -> Rect {
        let w = self.fonts.width(style, text);
        let h = height.unwrap_or(self.fonts.line(style) + 2 * self.pad);
        let r = Rect { l: self.left + self.pad, t: self.y, r: self.left + self.pad + w, b: self.y + h };
        self.captions.push(Caption { r, text, style, muted });
        self.widths.push(w + 2 * self.pad);
        r
    }

    /// Заголовок раздела и подсказка под ним (N 1268–1274).
    fn heading(&mut self, title: &'static str, hint: Option<&'static str>, first: bool) {
        if !first {
            self.y += self.s(16.0);
        }
        self.y = self.caption(title, Style::Heading, false, None).b;
        if let Some(hint) = hint {
            self.y = self.caption(hint, Style::Small, true, None).b;
        }
    }

    /// Ряд образцов (N 1276–1296): образцы 26 с отступом 2, свой (+4), «Другой…» (+8, поля 10,
    /// высота 28), «Пипетка» (+4). Ширина окна считается со своим образцом — размер не меняется.
    fn swatch_row(&mut self, model: &ColorModel, field: Field) -> SwatchRow {
        self.y += self.s(6.0);
        let (size, button_h) = (self.s(26.0), self.s(28.0));
        let row_h = size.max(button_h);
        let sy = self.y + (row_h - size) / 2;
        let by = self.y + (row_h - button_h) / 2;
        let mut row = SwatchRow::default();
        let mut x = self.left;
        for r in row.presets.iter_mut() {
            *r = Rect { l: x, t: sy, r: x + size, b: sy + size };
            x += size + self.s(2.0);
        }
        let w_other = self.fonts.width(Style::Text, "Другой…") + 2 * self.s(10.0);
        let w_pipette = self.fonts.width(Style::Text, "Пипетка") + 2 * self.s(10.0);
        let tail = self.s(8.0) + w_other + self.s(4.0) + w_pipette;
        self.widths.push(x + self.s(4.0) + size + tail - self.left);
        if model.custom_swatch(field).is_some() {
            let cx = x + self.s(4.0);
            row.custom = Some(Rect { l: cx, t: sy, r: cx + size, b: sy + size });
            x = cx + size;
        }
        let ox = x + self.s(8.0);
        row.other = Rect { l: ox, t: by, r: ox + w_other, b: by + button_h };
        let pipette_x = row.other.r + self.s(4.0);
        row.pipette = Rect { l: pipette_x, t: by, r: pipette_x + w_pipette, b: by + button_h };
        self.y += row_h;
        row
    }
}

impl Layout {
    pub fn new(fonts: &DialogFonts, model: &ColorModel) -> Layout {
        let scale = fonts.scale();
        let s = |v: f64| px(v, scale);
        let mut b = Builder {
            fonts,
            scale,
            left: s(20.0),
            pad: s(LABEL_PAD),
            y: s(14.0),
            widths: vec![s(420.0)],
            captions: Vec::new(),
        };
        let (left, pad) = (b.left, b.pad);

        b.heading("Заливка", Some("Подсветка отмеченного текста"), true);
        let fill = b.swatch_row(model, Field::Fill);

        // «Плотность» + ползунок (длина 220, толщина 10, отступы 12) + «30 %» (N 1165–1179)
        b.y += s(8.0);
        let row_h = (fonts.line(Style::Text) + 2 * pad).max(s(SLIDER_WIDTH));
        let density = b.caption("Плотность", Style::Text, false, Some(row_h));
        let tx = density.r + pad + s(12.0);
        let ty = b.y + (row_h - s(SLIDER_WIDTH)) / 2;
        let trough = Rect { l: tx, t: ty, r: tx + s(SLIDER_LENGTH), b: ty + s(SLIDER_WIDTH) };
        let slider_hit = Rect { l: trough.l, t: b.y, r: trough.r, b: b.y + row_h };
        let ox = trough.r + s(12.0) + pad;
        let opacity_label = Rect { l: ox, t: b.y, r: ox + fonts.width(Style::Text, "60 %"), b: b.y + row_h };
        b.widths.push(opacity_label.r + pad - left);
        b.y += row_h;

        b.heading("Акцент", Some("Полоса слева, плашки «▲/▼ Закладка» и значок в трее"), false);
        let accent = b.swatch_row(model, Field::Accent);

        // «Авто / Чёрный / Белый / Другой…»: высота 30, поля 14, отступ 6 (N 1184–1197)
        b.heading("Цвет надписи на плашке", None, false);
        b.y += s(6.0);
        let mut labels = [Rect::default(); 4];
        let mut x = left;
        for (r, choice) in labels.iter_mut().zip(LabelChoice::ALL) {
            let w = fonts.width(Style::Text, choice.caption()) + 2 * s(14.0);
            *r = Rect { l: x, t: b.y, r: x + w, b: b.y + s(30.0) };
            x += w + s(6.0);
        }
        b.widths.push(x - left);
        b.y += s(30.0);

        // Текст о контрасте: место под самый длинный вариант, чтобы окно не меняло размер.
        b.y += s(6.0);
        let contrast_h = fonts.line(Style::Small) + 2 * pad;
        for longest in ["Контраст 10,0:1 · Плохо читается — лучше чёрный", "Контраст 10,0:1 · Плохо читается — лучше белый"] {
            b.widths.push(fonts.width(Style::Small, longest) + 2 * pad);
        }
        let contrast_top = b.y;
        b.y += contrast_h;

        b.heading("Как будет выглядеть", None, false);
        b.y += s(6.0);
        let preview_top = b.y;
        let preview_h = s(2.0 * 52.0 + 8.0);
        b.y += preview_h;

        // «По умолчанию» слева; «Отмена» и «Сохранить» справа: высота 32, поля 16 (N 1207–1217)
        b.y += s(16.0);
        let button = |text: &str| fonts.width(Style::Text, text) + 2 * s(16.0);
        let (w_reset, w_cancel, w_save) = (button("По умолчанию"), button("Отмена"), button("Сохранить"));
        b.widths.push(w_reset + s(8.0) + w_cancel + s(8.0) + w_save);
        let buttons_top = b.y;
        b.y += s(32.0) + s(18.0);

        let content = b.widths.iter().copied().max().unwrap_or(0);
        let right = left + content;
        let save = Rect { l: right - w_save, t: buttons_top, r: right, b: buttons_top + s(32.0) };
        let cancel = Rect { l: save.l - s(8.0) - w_cancel, t: buttons_top, r: save.l - s(8.0), b: save.b };
        let reset = Rect { l: left, t: buttons_top, r: left + w_reset, b: save.b };
        Layout {
            size: (right + left, b.y),
            captions: b.captions,
            fill,
            accent,
            trough,
            slider_hit,
            opacity_label,
            labels,
            contrast: Rect { l: left + pad, t: contrast_top, r: right - pad, b: contrast_top + contrast_h },
            preview: Rect { l: left, t: preview_top, r: right, b: preview_top + preview_h },
            reset,
            cancel,
            save,
            scale,
        }
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    fn row(&self, field: Field) -> &SwatchRow {
        match field {
            Field::Fill => &self.fill,
            Field::Accent => &self.accent,
        }
    }

    /// Место части окна; `None` — её сейчас нет (свой цвет из готовых).
    pub fn rect(&self, target: Target) -> Option<Rect> {
        match target {
            Target::Swatch(field, index) => self.row(field).presets.get(index).copied(),
            Target::Custom(field) => self.row(field).custom,
            Target::Other(field) => Some(self.row(field).other),
            Target::Pipette(field) => Some(self.row(field).pipette),
            Target::Slider => Some(self.slider_hit),
            Target::Label(choice) => Some(self.labels[choice as usize]),
            Target::Reset => Some(self.reset),
            Target::Cancel => Some(self.cancel),
            Target::Save => Some(self.save),
        }
    }

    /// Часть окна в точке `p` (клиентские координаты).
    pub fn hit(&self, p: Point) -> Option<Target> {
        let inside = |r: Rect| p.x >= r.l && p.x < r.r && p.y >= r.t && p.y < r.b;
        for field in [Field::Fill, Field::Accent] {
            let row = self.row(field);
            if let Some(index) = row.presets.iter().position(|r| inside(*r)) {
                return Some(Target::Swatch(field, index));
            }
            if row.custom.is_some_and(inside) {
                return Some(Target::Custom(field));
            }
            if inside(row.other) {
                return Some(Target::Other(field));
            }
            if inside(row.pipette) {
                return Some(Target::Pipette(field));
            }
        }
        if inside(self.slider_hit) {
            return Some(Target::Slider);
        }
        if let Some(index) = self.labels.iter().position(|r| inside(*r)) {
            return Some(Target::Label(LabelChoice::ALL[index]));
        }
        [(self.reset, Target::Reset), (self.cancel, Target::Cancel), (self.save, Target::Save)]
            .into_iter()
            .find(|(r, _)| inside(*r))
            .map(|(_, t)| t)
    }

    /// Значение ползунка под точкой `x` (середина движка — у указателя), в пределах 20…60.
    pub fn slider_value(&self, x: i32) -> i64 {
        let (low, high) = OPACITY_RANGE;
        let knob = px(KNOB, self.scale);
        let span = (self.trough.r - self.trough.l - knob).max(1) as f64;
        let offset = (x - (self.trough.l + knob / 2)) as f64;
        let value = low as f64 + (offset * (high - low) as f64 / span).round();
        (value as i64).clamp(low as i64, high as i64)
    }

    /// Движок ползунка при значении `percent`.
    pub fn knob(&self, percent: u32) -> Rect {
        let (low, high) = OPACITY_RANGE;
        let knob = px(KNOB, self.scale);
        let span = (self.trough.r - self.trough.l - knob).max(0) as f64;
        let share = (percent.clamp(low, high) - low) as f64 / (high - low) as f64;
        let l = self.trough.l + (share * span).round() as i32;
        Rect { l, t: self.trough.t, r: l + knob, b: self.trough.b }
    }
}

// ------------------------------------------------------------ рисование (без окон)

/// Зелёный «Хорошо читается» (`ok` палитры Python, N 306, 311).
fn ok_color(light: bool) -> Rgb {
    if light {
        Rgb(0x0F, 0x7B, 0x0F)
    } else {
        Rgb(0x6C, 0xCB, 0x5F)
    }
}

/// Нарисовать всё окно в холст размера `layout.size`.
pub fn render(
    c: &mut Canvas,
    model: &ColorModel,
    layout: &Layout,
    fonts: &DialogFonts,
    p: &Palette,
    hover: Option<Target>,
    pressed: Option<Target>,
) {
    let scale = fonts.scale();
    let (w, h) = layout.size;
    c.fill(Rect { l: 0, t: 0, r: w, b: h }, p.bg);
    for caption in &layout.captions {
        let color = if caption.muted { p.muted } else { p.fg };
        c.text(caption.r, caption.text, fonts.font(caption.style), color, Align::Left);
    }
    let button = |c: &mut Canvas, target: Target, caption: &str, selected: bool, primary: bool| {
        let Some(rect) = layout.rect(target) else { return };
        let (fill, hover_fill, fg) =
            if primary { (p.primary, p.primary_hover, p.primary_fg) } else { (p.button, p.button_hover, p.fg) };
        let spec = ButtonSpec {
            rect,
            caption,
            hover: hover == Some(target),
            pressed: pressed == Some(target),
            selected,
            fill: Some(fill),
            hover_fill,
            fg,
            align: Align::Center,
        };
        draw_button(c, &spec, fonts.font(Style::Text), scale);
    };
    for field in [Field::Fill, Field::Accent] {
        let row = layout.row(field);
        let selected = model.selected_preset(field);
        for (index, r) in row.presets.iter().enumerate() {
            let target = Target::Swatch(field, index);
            draw_swatch(c, *r, PRESETS[index].0, selected == Some(index), hover == Some(target), p, scale);
        }
        if let (Some(r), Some(color)) = (row.custom, model.custom_swatch(field)) {
            draw_swatch(c, r, color, true, hover == Some(Target::Custom(field)), p, scale);
        }
        button(c, Target::Other(field), "Другой…", false, false);
        button(c, Target::Pipette(field), "Пипетка", false, false);
    }
    // Ползунок Tk: желоб `line`, движок `primary` (при наведении и перетаскивании — `primary_hover`).
    c.fill(layout.trough, p.line);
    let active = hover == Some(Target::Slider) || pressed == Some(Target::Slider);
    c.fill(layout.knob(model.opacity_percent()), if active { p.primary_hover } else { p.primary });
    c.text(layout.opacity_label, &model.opacity_text(), fonts.font(Style::Text), p.muted, Align::Left);
    let chosen = model.selected_label();
    for choice in LabelChoice::ALL {
        button(c, Target::Label(choice), choice.caption(), choice == chosen, false);
    }
    let (text, good) = model.contrast_text();
    let color = if good { ok_color(is_light(p)) } else { p.danger };
    c.text(layout.contrast, &text, fonts.font(Style::Small), color, Align::Left);
    draw_preview(c, layout.preview, model, fonts);
    button(c, Target::Reset, "По умолчанию", false, false);
    button(c, Target::Cancel, "Отмена", false, false);
    button(c, Target::Save, "Сохранить", false, true);
}

/// «Как будет выглядеть» (N 1368–1397): светлая и тёмная полосы с образцом текста под заливкой,
/// полосой акцента, «✕» и плашкой «▼ Закладка».
fn draw_preview(c: &mut Canvas, area: Rect, model: &ColorModel, fonts: &DialogFonts) {
    let s = |v: f64| px(v, fonts.scale());
    let width = area.r - area.l;
    let values = model.result();
    let (fill, opacity, accent, label) = (values.color, values.opacity, values.bar_color, model.label_actual());
    let (strip, gap) = (s(52.0), s(8.0));
    let line = fonts.line(Style::Sample);
    let themes = [(Rgb(0xFF, 0xFF, 0xFF), Rgb(0x1F, 0x1F, 0x1F)), (Rgb(0x1E, 0x1E, 0x1E), Rgb(0xE8, 0xE8, 0xE8))];
    for (index, (background, foreground)) in themes.into_iter().enumerate() {
        let top = area.t + index as i32 * (strip + gap);
        let middle = top + strip / 2;
        let outline = mix(background, Rgb(0x80, 0x80, 0x80), 0.4);
        c.round_rect(Rect { l: area.l, t: top, r: area.l + width, b: top + strip + 1 }, s(6.0), background, Some(outline));
        let text_x = area.l + s(46.0);
        let text_w = fonts.width(Style::Sample, PREVIEW_TEXT);
        let (high, low) = (middle - line / 2, middle + (line + 1) / 2);
        c.fill(Rect { l: text_x - s(2.0), t: high, r: text_x + text_w + s(2.0), b: low }, mix(background, fill, opacity));
        let text_r = Rect { l: text_x, t: high, r: text_x + text_w + s(4.0), b: low };
        c.text(text_r, PREVIEW_TEXT, fonts.font(Style::Sample), mix(foreground, fill, opacity), Align::Left);
        let bar_x = text_x - s(9.0);
        c.fill(Rect { l: bar_x, t: high, r: bar_x + s(4.0), b: low }, mix(background, accent, 0.9));
        // «✕» с якорем «ne» в (bar_x - 4, high - 1)
        let (cross_w, cross_h) = (fonts.width(Style::Symbol, "✕"), fonts.line(Style::Symbol));
        let (x1, y0) = (bar_x - s(4.0), high - s(1.0));
        c.text(Rect { l: x1 - cross_w, t: y0, r: x1, b: y0 + cross_h }, "✕", fonts.font(Style::Symbol), DELETE_COLOR, Align::Right);
        let caption = "▼ Закладка";
        let (plate_w, plate_h) = (fonts.width(Style::Plate, caption) + 2 * s(10.0), s(26.0));
        let plate_x = area.l + width - plate_w - s(12.0);
        let plate = Rect { l: plate_x, t: middle - plate_h / 2, r: plate_x + plate_w, b: middle - plate_h / 2 + plate_h };
        c.round_rect(plate, s(6.0), mix(background, accent, 0.96), None);
        c.text(plate, caption, fonts.font(Style::Plate), label, Align::Center);
    }
}

// ------------------------------------------------------------ окно

const CLASS: &str = "ZakladkaColors";

/// Действие после обработки сообщения (выполняется без заимствования состояния).
enum Act {
    Save,
    Cancel,
    Choose(Pick),
    Pipette(Field),
    /// Окно уничтожил кто-то другой: убрать значок и пипетку.
    Cleanup,
}

struct Inner {
    env: Arc<dyn DialogEnv>,
    model: ColorModel,
    on_save: Option<Box<dyn FnMut(Colors)>>,
    light: bool,
    palette: Palette,
    fonts: Option<DialogFonts>,
    layout: Option<Layout>,
    canvas: Option<Canvas>,
    hwnd: Hwnd,
    icon: isize,
    dirty: bool,
    hover: Option<Target>,
    pressed: Option<Target>,
    tracking: bool,
    tip_target: Option<Target>,
    picker: Option<Eyedropper>,
}

pub struct ColorDialog {
    inner: Rc<RefCell<Inner>>,
}

impl ColorDialog {
    pub fn open(api: Arc<Api>, current: Colors, on_save: Box<dyn FnMut(Colors)>) -> ColorDialog {
        ColorDialog::with_env(api, current, on_save)
    }

    /// Окно с подставным окружением (тесты: окно остаётся скрытым, фокус не берётся).
    /// Окно не создалось — `is_open() == false`.
    pub fn with_env(env: Arc<dyn DialogEnv>, current: Colors, on_save: Box<dyn FnMut(Colors)>) -> ColorDialog {
        let cursor = env.cursor();
        let scale = clean_scale(env.scale_at(cursor));
        let light = env.light();
        let model = ColorModel::new(current);
        let fonts = DialogFonts::new(scale);
        let layout = Layout::new(&fonts, &model);
        let accent = model.get(Field::Accent);
        let (cw, ch) = layout.size;
        let inner = Rc::new(RefCell::new(Inner {
            env: env.clone(),
            model,
            on_save: Some(on_save),
            light,
            palette: palette(light),
            fonts: Some(fonts),
            layout: Some(layout),
            canvas: None,
            hwnd: 0,
            icon: 0,
            dirty: true,
            hover: None,
            pressed: None,
            tracking: false,
            tip_target: None,
            picker: None,
        }));
        let dialog = ColorDialog { inner };
        // Внешний размер окна по клиентскому — с рамкой и заголовком монитора с курсором.
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut frame = RECT { left: 0, top: 0, right: cw, bottom: ch };
        let dpi = (96.0 * scale).round() as u32;
        // SAFETY: структура локальная.
        if unsafe { AdjustWindowRectExForDpi(&mut frame, style, false, WS_EX_TOPMOST, dpi) }.is_err() {
            frame = RECT { left: 0, top: 0, right: cw + 16, bottom: ch + 39 };
        }
        let (ww, wh) = (frame.right - frame.left, frame.bottom - frame.top);
        let work = env.work_area(cursor);
        let x = work.l.max((work.l + work.r - ww) / 2);
        let y = work.t.max((work.t + work.b - wh) / 2);
        let rect = Rect { l: x, t: y, r: x + ww, b: y + wh };
        let atom = register_class(CLASS, dialog_proc, false);
        if atom == 0 {
            return dialog;
        }
        let title = wide(&format!("{} — цвет", crate::core::text::APP_TITLE));
        // SAFETY: класс зарегистрирован; окно создаётся скрытым; привязка к состоянию ставится
        // после создания и снимается в WM_NCDESTROY.
        let hwnd = unsafe {
            let created = CreateWindowExW(
                WS_EX_TOPMOST,
                PCWSTR(atom as usize as *const u16),
                PCWSTR(title.as_ptr()),
                style,
                rect.l,
                rect.t,
                ww,
                wh,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                instance(),
                None,
            );
            let Ok(window) = created else { return dialog };
            if window.0.is_null() {
                return dialog;
            }
            let link = Box::into_raw(Box::new(Rc::downgrade(&dialog.inner)));
            SetWindowLongPtrW(window, GWLP_USERDATA, link as isize);
            from_hwnd(window)
        };
        let icon = env.icon(accent).unwrap_or(0);
        if icon != 0 {
            // SAFETY: окно своё; значок освобождается при закрытии.
            unsafe {
                SendMessageW(to_hwnd(hwnd), WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon));
                SendMessageW(to_hwnd(hwnd), WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(icon));
            }
        }
        if !light {
            dark_title_bar(hwnd, true); // до показа: без вспышки светлого заголовка
        }
        if let Ok(mut state) = dialog.inner.try_borrow_mut() {
            state.hwnd = hwnd;
            state.icon = icon;
        }
        env.show(hwnd, rect, true);
        dialog
    }

    pub fn is_open(&self) -> bool {
        self.inner.try_borrow().is_ok_and(|i| i.hwnd != 0)
    }

    pub fn close(&mut self) {
        close(&self.inner);
    }

    /// Окно (0 — закрыто).
    pub fn hwnd(&self) -> Hwnd {
        self.inner.try_borrow().map_or(0, |i| i.hwnd)
    }

    /// Значения сейчас (то, что сохранит «Сохранить»).
    pub fn values(&self) -> Option<Colors> {
        self.inner.try_borrow().ok().map(|i| i.model.result())
    }

    /// Окно спрятано на время пипетки.
    pub fn hidden(&self) -> bool {
        self.inner.try_borrow().is_ok_and(|i| i.model.hidden())
    }
}

impl Drop for ColorDialog {
    fn drop(&mut self) {
        close(&self.inner);
    }
}

/// Закрыть: окно уничтожить, значок освободить, пипетку отменить (её обработчик видит, что окна
/// уже нет, и ничего не делает — как N 1257–1262).
fn close(inner: &Rc<RefCell<Inner>>) {
    let (hwnd, icon, picker) = match inner.try_borrow_mut() {
        Ok(mut state) => state.take_window(),
        Err(_) => return,
    };
    if hwnd != 0 {
        Tip::hide(hwnd);
        // SAFETY: окно своё; WM_NCDESTROY освобождает привязку.
        unsafe {
            let _ = DestroyWindow(to_hwnd(hwnd));
        }
    }
    if icon != 0 {
        // SAFETY: значок создан для этого окна и больше нигде не используется.
        unsafe {
            let _ = DestroyIcon(HICON(icon as *mut c_void));
        }
    }
    if let Some(mut picker) = picker {
        picker.cancel();
    }
}

fn perform(inner: &Rc<RefCell<Inner>>, act: Act) {
    match act {
        Act::Cancel | Act::Cleanup => close(inner),
        Act::Save => {
            let (values, callback) = match inner.try_borrow_mut() {
                Ok(mut state) => (state.model.result(), state.on_save.take()),
                Err(_) => return,
            };
            close(inner);
            if let Some(mut callback) = callback {
                // Паника в обработчике не выходит наружу: место паники пишет panic hook.
                let _ = catch_unwind(AssertUnwindSafe(|| callback(values)));
                if let Ok(mut state) = inner.try_borrow_mut() {
                    if state.on_save.is_none() {
                        state.on_save = Some(callback);
                    }
                }
            }
        }
        Act::Choose(pick) => {
            let (env, hwnd, start, title) = match inner.try_borrow() {
                Ok(state) if state.hwnd != 0 => {
                    (state.env.clone(), state.hwnd, state.model.choose_start(pick), state.model.choose_title(pick))
                }
                _ => return,
            };
            Tip::hide(hwnd);
            // Модальное окно: своё состояние здесь не занято, окно цветов перерисовывается.
            let chosen = env.choose_color(hwnd, start, &title);
            if let Some(color) = chosen {
                if let Ok(mut state) = inner.try_borrow_mut() {
                    if state.hwnd != 0 {
                        state.model.apply_pick(pick, color);
                        state.refresh();
                    }
                }
            }
        }
        Act::Pipette(field) => {
            let (env, hwnd) = match inner.try_borrow() {
                Ok(state) if state.hwnd != 0 => (state.env.clone(), state.hwnd),
                _ => return,
            };
            Tip::hide(hwnd);
            let (picked, cancelled) = (Rc::downgrade(inner), Rc::downgrade(inner));
            let screen: Arc<dyn ScreenEnv> = env.clone();
            // Сначала пипетка (мы на переднем плане и можем отдать ей фокус), потом — в сторону (N 1321).
            let picker = Eyedropper::with_env(
                screen,
                Box::new(move |color| pipette_done(&picked, Some(color))),
                Box::new(move || pipette_done(&cancelled, None)),
            );
            if !picker.active() {
                return;
            }
            let hide = match inner.try_borrow_mut() {
                Ok(mut state) if state.hwnd != 0 => {
                    state.model.pipette_started(field);
                    state.picker = Some(picker);
                    state.pressed = None;
                    state.hover = None;
                    state.tip_target = None;
                    state.tracking = false; // после возврата окна слежение за уходом мыши — заново
                    true
                }
                _ => false, // окно закрылось, пока запускалась пипетка: ручка отпущена, пипетка работает сама
            };
            if hide {
                env.hide(hwnd);
            }
        }
    }
}

/// Пипетка кончилась (N 1313–1319): окно вернуть и дать ему фокус; цвет — в поле.
fn pipette_done(dialog: &Weak<RefCell<Inner>>, color: Option<Rgb>) {
    let Some(inner) = dialog.upgrade() else { return };
    let (env, hwnd) = {
        let Ok(mut state) = inner.try_borrow_mut() else { return };
        if state.hwnd == 0 {
            return;
        }
        state.model.pipette_finished(color);
        state.picker = None;
        state.refresh();
        (state.env.clone(), state.hwnd)
    };
    let mut r = RECT::default();
    // SAFETY: окно своё, структура локальная.
    if unsafe { GetWindowRect(to_hwnd(hwnd), &mut r) }.is_ok() {
        env.show(hwnd, r.into(), true);
    }
}

fn point_from(lparam: isize) -> Point {
    Point { x: (lparam & 0xFFFF) as u16 as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32 }
}

fn to_screen(hwnd: Hwnd, p: Point) -> Point {
    let mut point = POINT::from(p);
    // SAFETY: окно своё, точка локальная.
    unsafe {
        let _ = ClientToScreen(to_hwnd(hwnd), &mut point);
    }
    point.into()
}

impl Inner {
    /// Отвязать окно (его уничтожает вызывающий); отпустить холст, шрифты, раскладку.
    fn take_window(&mut self) -> (Hwnd, isize, Option<Eyedropper>) {
        let taken = (std::mem::take(&mut self.hwnd), std::mem::take(&mut self.icon), self.picker.take());
        self.canvas = None;
        self.fonts = None;
        self.layout = None;
        self.hover = None;
        self.pressed = None;
        self.tracking = false;
        self.tip_target = None;
        taken
    }

    /// Модель изменилась: новая раскладка (свой образец мог появиться или пропасть) и перерисовка.
    fn refresh(&mut self) {
        if let Some(fonts) = self.fonts.as_ref() {
            self.layout = Some(Layout::new(fonts, &self.model));
        }
        self.invalidate();
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        if self.hwnd != 0 {
            // SAFETY: окно своё.
            unsafe {
                let _ = InvalidateRect(to_hwnd(self.hwnd), None, BOOL(0));
            }
        }
    }

    fn hit(&self, p: Point) -> Option<Target> {
        self.layout.as_ref().and_then(|l| l.hit(p))
    }

    fn render(&mut self) {
        let (Some(layout), Some(fonts)) = (self.layout.as_ref(), self.fonts.as_ref()) else { return };
        let size = layout.size;
        if self.canvas.as_ref().map_or(true, |c| c.size() != size) {
            self.canvas = Some(Canvas::new(size.0, size.1));
        }
        if let Some(canvas) = self.canvas.as_mut() {
            render(canvas, &self.model, layout, fonts, &self.palette, self.hover, self.pressed);
        }
        self.dirty = false;
    }

    fn tip_text(&self, target: Target) -> String {
        match target {
            Target::Swatch(_, index) => PRESETS.get(index).map(|p| p.1).unwrap_or("").to_string(),
            Target::Custom(field) => self.model.custom_tip(field),
            Target::Other(_) => TIP_OTHER.to_string(),
            Target::Pipette(_) => TIP_PIPETTE.to_string(),
            Target::Slider => TIP_SLIDER.to_string(),
            Target::Label(choice) => choice.tip().to_string(),
            Target::Reset => TIP_RESET.to_string(),
            Target::Cancel | Target::Save => String::new(),
        }
    }

    /// Пояснение к части под указателем: новая часть — спрятать и ждать снова.
    fn update_tip(&mut self, hwnd: Hwnd, p: Point) {
        if self.hover == self.tip_target {
            return;
        }
        self.tip_target = self.hover;
        Tip::hide(hwnd);
        if let Some(target) = self.hover {
            let text = self.tip_text(target);
            if !text.is_empty() {
                Tip::schedule(hwnd, text, to_screen(hwnd, p), self.light);
            }
        }
    }

    /// Ползунок у точки `x`.
    fn drag_slider(&mut self, x: i32) {
        let Some(value) = self.layout.as_ref().map(|l| l.slider_value(x)) else { return };
        if self.model.set_opacity_percent(value) {
            self.invalidate();
        }
    }

    /// Щелчок (нажатие и отпускание на одной и той же части).
    fn click(&mut self, target: Target) -> Option<Act> {
        match target {
            Target::Swatch(field, index) => {
                if let Some((color, _)) = PRESETS.get(index) {
                    self.model.set(field, *color);
                    self.refresh();
                }
                None
            }
            Target::Custom(_) | Target::Slider => None,
            Target::Other(field) => Some(Act::Choose(Pick::Field(field))),
            Target::Pipette(field) => Some(Act::Pipette(field)),
            Target::Label(LabelChoice::Custom) => Some(Act::Choose(Pick::Label)),
            Target::Label(choice) => {
                self.model.choose_label(choice);
                self.refresh();
                None
            }
            Target::Reset => {
                self.model.reset();
                self.refresh();
                None
            }
            Target::Cancel => Some(Act::Cancel),
            Target::Save => Some(Act::Save),
        }
    }

    fn handle(&mut self, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> (Option<isize>, Option<Act>) {
        if hwnd != self.hwnd || hwnd == 0 {
            return (None, None);
        }
        let window = to_hwnd(hwnd);
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: обработка WM_PAINT своего окна.
                unsafe {
                    let dc = BeginPaint(window, &mut ps);
                    if self.dirty || self.canvas.is_none() {
                        self.render();
                    }
                    if let Some(canvas) = &self.canvas {
                        canvas.present(dc.0 as isize, 0, 0);
                    }
                    let _ = EndPaint(window, &ps);
                }
                (Some(0), None)
            }
            WM_MOUSEMOVE => {
                // SAFETY: окно своё. Скрытому окну (тесты) слежение за уходом мыши не нужно.
                if !self.tracking && unsafe { IsWindowVisible(window) }.as_bool() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    // SAFETY: структура локальная.
                    self.tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
                }
                let p = point_from(lparam);
                if self.pressed == Some(Target::Slider) {
                    self.drag_slider(p.x);
                }
                let target = self.hit(p);
                if target != self.hover {
                    self.hover = target;
                    self.invalidate();
                }
                self.update_tip(hwnd, p);
                (Some(0), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover = None;
                self.tip_target = None;
                Tip::hide(hwnd);
                self.invalidate();
                (Some(0), None)
            }
            WM_LBUTTONDOWN => {
                Tip::hide(hwnd);
                let p = point_from(lparam);
                let target = self.hit(p);
                self.hover = target;
                self.tip_target = target; // после щелчка пояснение не возвращается, пока указатель здесь
                self.pressed = target;
                // SAFETY: окно своё. Скрытому окну (тесты) мышь не захватывать.
                unsafe {
                    if IsWindowVisible(window).as_bool() {
                        SetCapture(window);
                    }
                }
                if target == Some(Target::Slider) {
                    self.drag_slider(p.x);
                }
                self.invalidate();
                (Some(0), None)
            }
            WM_LBUTTONUP => {
                let pressed = self.pressed.take();
                // SAFETY: отпустить свой захват мыши.
                unsafe {
                    let _ = ReleaseCapture();
                }
                let target = self.hit(point_from(lparam));
                self.hover = target;
                self.invalidate();
                match pressed {
                    Some(t) if Some(t) == target => (Some(0), self.click(t)),
                    _ => (Some(0), None),
                }
            }
            WM_CAPTURECHANGED => {
                // Захват отняли (например, Alt+Tab во время перетаскивания ползунка).
                if self.pressed.take().is_some() {
                    self.invalidate();
                }
                (Some(0), None)
            }
            WM_KEYDOWN => {
                let vk = wparam as u16;
                if vk == VK_ESCAPE.0 {
                    (Some(0), Some(Act::Cancel))
                } else if vk == VK_RETURN.0 {
                    (Some(0), Some(Act::Save))
                } else if vk == VK_LEFT.0 || vk == VK_RIGHT.0 {
                    let step = if vk == VK_LEFT.0 { -1 } else { 1 };
                    if self.model.set_opacity_percent(self.model.opacity_percent() as i64 + step) {
                        self.invalidate();
                    }
                    (Some(0), None)
                } else {
                    (None, None)
                }
            }
            WM_CLOSE => (Some(0), Some(Act::Cancel)),
            WM_DESTROY => {
                // Окно уничтожил кто-то другой (наше закрытие отвязывает окно раньше).
                self.hwnd = 0;
                (None, Some(Act::Cleanup))
            }
            _ => (None, None),
        }
    }
}

/// Оконная процедура окна цветов (вызывается переходником `register_class` внутри `catch_unwind`).
fn dialog_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    let window = to_hwnd(hwnd);
    match msg {
        WM_ERASEBKGND => return Some(1), // фон рисует WM_PAINT из заднего буфера
        WM_DPICHANGED => return Some(0), // масштаб — по монитору открытия, как в Python
        WM_NCDESTROY => {
            // SAFETY: привязку ставит `with_env` (Box<Weak>), снимаем один раз.
            unsafe {
                let link = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut Weak<RefCell<Inner>>;
                if !link.is_null() {
                    drop(Box::from_raw(link));
                }
            }
            return None;
        }
        _ => {}
    }
    // SAFETY: в GWLP_USERDATA только наш Box<Weak> (или 0 до привязки).
    let inner = unsafe {
        let link = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Weak<RefCell<Inner>>;
        if link.is_null() {
            return None;
        }
        (*link).upgrade()?
    };
    let (result, act) = {
        // Занято (сообщение пришло изнутри вызова Win32 отсюда же) — обработка по умолчанию.
        let Ok(mut state) = inner.try_borrow_mut() else { return None };
        state.handle(hwnd, msg, wparam, lparam)
    };
    if let Some(act) = act {
        perform(&inner, act);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lparam_points_are_signed() {
        assert_eq!(point_from(0x0002_0001), Point { x: 1, y: 2 });
        let negative = ((-5i16 as u16 as isize) << 16) | (-7i16 as u16 as isize);
        assert_eq!(point_from(negative), Point { x: -7, y: -5 });
    }
}
