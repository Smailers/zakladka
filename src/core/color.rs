//! Цвета: проверка, контраст WCAG, смешение, значок (P 520–536, 590–603; N 26–125).
//!
//! Яркость — с порогом 0,04045 (panel.py). В text_bookmark.pyw порог 0,03928; для 8-битных
//! каналов это одно и то же: между 10/255 = 0,0392 и 11/255 = 0,0431 нет ни одного значения канала
//! (проверено тестом по всем 256 значениям).

use crate::core::flatten::py_strip;
use crate::types::{LabelColor, Rgb};

/// Цветовые настройки (подмножество `Config`), которыми обмениваются UI и окно цветов.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Colors {
    pub color: Rgb,
    pub opacity: f64,
    pub bar_color: Rgb,
    pub label_color: LabelColor,
}

/// Готовые цвета окна цветов (N 31–40).
pub const PRESETS: [(Rgb, &str); 8] = [
    (Rgb(0xFF, 0xD4, 0x00), "Жёлтый"),
    (Rgb(0xFF, 0x7A, 0x00), "Оранжевый"),
    (Rgb(0x34, 0xC7, 0x59), "Зелёный"),
    (Rgb(0x22, 0xC7, 0xE8), "Голубой"),
    (Rgb(0xFF, 0x5C, 0xA8), "Розовый"),
    (Rgb(0x9B, 0x6C, 0xFF), "Фиолетовый"),
    (Rgb(0xC8, 0xC8, 0xC8), "Светло-серый"),
    (Rgb(0xFF, 0xFF, 0xFF), "Белый"),
];

/// Плотность заливки, проценты (N 30).
pub const OPACITY_RANGE: (u32, u32) = (20, 60);

/// Светло-красный флажок ✕ (P 520).
pub const FLAG_COLOR: Rgb = Rgb(0xE5, 0x73, 0x73);

/// '#RRGGBB' из '#rrggbb', 'rrggbb' или '#rgb' (N 54–61).
pub fn normalize_color(s: &str) -> Result<Rgb, ()> {
    let text = py_strip(s).trim_start_matches('#');
    let digits: String = if text.chars().count() == 3 {
        text.chars().flat_map(|c| [c, c]).collect()
    } else {
        text.to_string()
    };
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    let channel = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| ());
    Ok(Rgb(channel(0)?, channel(2)?, channel(4)?))
}

fn linear(value: u8) -> f64 {
    let v = value as f64 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Порог 0.04045 (как в panel.py).
pub fn relative_luminance(c: Rgb) -> f64 {
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// Коэффициент контраста WCAG, 1.0 … 21.0.
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (relative_luminance(a), relative_luminance(b));
    let (light, dark) = if x >= y { (x, y) } else { (y, x) };
    (light + 0.05) / (dark + 0.05)
}

/// Чёрный или белый — что лучше читается на `background` (при равенстве — чёрный).
pub fn label_color(background: Rgb) -> Rgb {
    let black = contrast_ratio(background, Rgb(0, 0, 0));
    let white = contrast_ratio(background, Rgb(0xFF, 0xFF, 0xFF));
    if black >= white {
        Rgb(0, 0, 0)
    } else {
        Rgb(0xFF, 0xFF, 0xFF)
    }
}

/// `max(lo, v)` и `min(hi, v)` Python: NaN не проходит сравнение и даёт первый аргумент.
fn py_clamp(v: f64, lo: f64, hi: f64) -> f64 {
    let v = if v > lo { v } else { lo };
    if v < hi {
        v
    } else {
        hi
    }
}

/// `c`, сдвинутый к `other` на долю `share` (0.0 … 1.0); округление как `round()` Python.
pub fn mix(c: Rgb, other: Rgb, share: f64) -> Rgb {
    let share = py_clamp(share, 0.0, 1.0);
    let channel = |a: u8, b: u8| {
        let value = a as f64 + (b as f64 - a as f64) * share;
        value.round_ties_even().clamp(0.0, 255.0) as u8
    };
    Rgb(channel(c.0, other.0), channel(c.1, other.1), channel(c.2, other.2))
}

/// BGRA, `size × size × 4` байт.
pub fn icon_pixels(size: u32, color: Rgb) -> Vec<u8> {
    let size = size as usize;
    let mut data = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let u = (x as f64 + 0.5) / size as f64;
            let v = (y as f64 + 0.5) / size as f64;
            if crate::core::ribbon::inside(u, v) {
                let i = (y * size + x) * 4;
                data[i..i + 4].copy_from_slice(&[color.2, color.1, color.0, 0xFF]);
            }
        }
    }
    data
}

/// `round(x, 2)` Python: точное десятичное округление (формат с двумя знаками и обратно).
fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

/// Проверенные цвета (N 102–125). Цвета уже разобраны в `Rgb`; поправляется плотность:
/// больше 1 — проценты, затем пределы `OPACITY_RANGE`, округление до 0,01.
pub fn clean_colors(c: &Colors) -> Colors {
    let mut opacity = c.opacity;
    if opacity > 1.0 {
        opacity /= 100.0;
    }
    let (low, high) = OPACITY_RANGE;
    let opacity = round2(py_clamp(opacity, low as f64 / 100.0, high as f64 / 100.0));
    Colors { opacity, ..*c }
}
