//! Плоский текст (P 42–45, 53–120): NFKC по одному символу, casefold, ё→е, без пробельных и
//! невидимых символов. Все смещения — в `char`.
//!
//! Совместимость с Python 3.13 (Unicode 15.1): таблицы crates новее (NFKC — Unicode 18,
//! casefold — Unicode 16). Для символов, которых Python 3.13 ещё не знает, Python ничего не меняет;
//! так же поступает и этот модуль (`NEWER_THAN_PYTHON`). Полное совпадение по всем кодовым точкам
//! проверяет тест `flatten_table_matches_python` (`tests\golden\flatten_table.json`).

use std::collections::BTreeMap;

use caseless::Caseless;
use unicode_normalization::UnicodeNormalization;

/// Короче — цитате нужно совпадение контекста.
pub const SHORT_QUOTE: usize = 25;
/// Плоских символов prefix/suffix, сохраняемых с цитатой.
pub const CONTEXT: usize = 64;
/// Плоских символов выделения, идущих в цитату.
pub const MAX_QUOTE: usize = 600;
/// `ZERO_WIDTH` из Python: U+FFFC, U+200B, U+200C, U+200D, U+2060, U+FEFF, U+00AD.
pub const ZERO_WIDTH: [char; 7] =
    ['\u{FFFC}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{00AD}'];

/// Кодовые точки, не назначенные в Unicode 15.1 (Python 3.13), для которых таблицы crates дают
/// NFKC, casefold или класс сочетания. Python оставляет их как есть — здесь так же.
/// Получено полным перебором против `tests\golden\flatten_table.json` (диапазоны склеены через
/// промежутки, тоже не назначенные в 15.1, — для них ответ тот же).
const NEWER_THAN_PYTHON: &[(u32, u32)] = &[
    (0x0558, 0x0558), (0x058B, 0x058C), (0x05C8, 0x05C9), (0x0897, 0x0897), (0x1ACF, 0x1AF0), (0x1C89, 0x1C89),
    (0x209D, 0x209F), (0xA7CB, 0xA7CC), (0xA7DA, 0xA7F1), (0x107BB, 0x107BF), (0x10D50, 0x10D6D), (0x10ECB, 0x10EFB),
    (0x113CE, 0x113D0), (0x1612F, 0x1612F), (0x1B123, 0x1B126), (0x1CCD6, 0x1CCF9), (0x1D127, 0x1D128), (0x1D250, 0x1D281),
    (0x1D6A6, 0x1D6A6), (0x1DF95, 0x1DFFF), (0x1E5EE, 0x1E6F5),
];

fn newer_than_python(c: char) -> bool {
    let c = c as u32;
    NEWER_THAN_PYTHON
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `str.isspace()` Python: двунаправленный класс WS, B, S или категория Zs.
/// Отличие от `char::is_whitespace`: Python считает пробельными ещё U+001C..U+001F.
pub(crate) fn py_isspace(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// `str.strip()` Python (без аргументов).
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// `" ".join(text.split())` Python.
pub(crate) fn py_split_join(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split(py_isspace).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn is_zero_width(c: char) -> bool {
    ZERO_WIDTH.contains(&c)
}

/// `c.casefold()` Python (полная свёртка CaseFolding.txt C+F) — по одному символу.
fn casefold_char(c: char, out: &mut impl FnMut(char)) {
    if c.is_ascii() {
        out(c.to_ascii_lowercase());
    } else if newer_than_python(c) {
        out(c);
    } else {
        std::iter::once(c).default_case_fold().for_each(out);
    }
}

/// `s.casefold()` Python.
pub(crate) fn casefold_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        casefold_char(c, &mut |d| out.push(d));
    }
    out
}

/// `unicodedata.combining(c) != 0` Python 3.13.
pub(crate) fn is_combining(c: char) -> bool {
    !c.is_ascii()
        && unicode_normalization::char::canonical_combining_class(c) != 0
        && !newer_than_python(c)
}

/// `unicodedata.normalize("NFKC", c).casefold()` для одного символа.
fn fold_char(c: char, out: &mut impl FnMut(char)) {
    if c.is_ascii() {
        out(c.to_ascii_lowercase());
    } else if newer_than_python(c) {
        out(c);
    } else {
        for d in std::iter::once(c).nfkc() {
            casefold_char(d, out);
        }
    }
}

/// `index[k]` = позиция (в `char`) исходного символа для плоского символа `k`.
pub fn flatten(text: &str) -> (Vec<char>, Vec<u32>) {
    let mut chars = Vec::with_capacity(text.len());
    let mut index = Vec::with_capacity(text.len());
    for (position, c) in text.chars().enumerate() {
        if is_zero_width(c) || py_isspace(c) {
            continue;
        }
        fold_char(c, &mut |part| {
            let part = if part == 'ё' { 'е' } else { part };
            if !py_isspace(part) && !is_zero_width(part) {
                chars.push(part);
                index.push(position as u32);
            }
        });
    }
    (chars, index)
}

/// Плоский текст документа из имён сегментов.
/// `seg[k]` — номер сегмента плоского символа `k`, `pos[k]` — его позиция (в `char`) в имени сегмента.
pub struct FlatDoc {
    pub text: Vec<char>,
    pub seg: Vec<u32>,
    pub pos: Vec<u32>,
}

impl FlatDoc {
    pub fn new<S: AsRef<str>>(names: &[S]) -> FlatDoc {
        let mut doc = FlatDoc { text: Vec::new(), seg: Vec::new(), pos: Vec::new() };
        for (number, name) in names.iter().enumerate() {
            let (flat, index) = flatten(name.as_ref());
            doc.seg.extend(std::iter::repeat(number as u32).take(flat.len()));
            doc.pos.extend(index);
            doc.text.extend(flat);
        }
        doc
    }

    /// (сегмент, первый, конец) — позиции в имени сегмента.
    pub fn spans(&self, start: isize, end: isize) -> Vec<(usize, usize, usize)> {
        let limit = self.text.len().min(self.seg.len()).min(self.pos.len());
        let low = start.max(0) as usize;
        let high = if end < 0 { 0 } else { (end as usize).min(limit) };
        let mut found: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
        for k in low..high {
            let (segment, char) = (self.seg[k], self.pos[k]);
            let entry = found.entry(segment).or_insert((char, char + 1));
            entry.0 = entry.0.min(char);
            entry.1 = entry.1.max(char + 1);
        }
        found
            .into_iter()
            .map(|(segment, (first, last))| (segment as usize, first as usize, last as usize))
            .collect()
    }
}

/// Сохранённые quote/prefix/suffix -> `Vec<char>`.
pub fn to_flat(s: &str) -> Vec<char> {
    s.chars().collect()
}

pub fn has_cyrillic(text: &str) -> bool {
    text.chars().any(|c| ('\u{400}'..='\u{4FF}').contains(&c))
}

/// Кодовые точки, которые UI Automation считает внутри одного символа (P 354–357).
pub fn grapheme_extra(text: &[char]) -> usize {
    text.iter()
        .map(|&c| {
            if c == '\u{200D}' {
                2
            } else if is_combining(c)
                || c == '\u{FE0E}'
                || c == '\u{FE0F}'
                || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
            {
                1
            } else {
                0
            }
        })
        .sum()
}
