//! Правила: заголовки, страницы, где можно Ctrl+C, выбор вхождения, замена позиции (P 160–383).

use std::collections::HashSet;

use crate::core::flatten::{casefold_str, py_strip, FlatDoc};
use crate::core::geometry::{distance, overlap_area};
use crate::core::matching::{occurrences, strong_match};
use crate::types::{Bookmark, Kind, Mode, Point, Rect};

/// Хеш `uia::ScopeKey` (часть 1 не знает UIA): вызывающий считает его сам,
/// например `std::hash::DefaultHasher` от `ScopeKey`.
pub type ScopeKeyHash = u64;

/// Как `BROWSERS` в Python (P 207).
pub const BROWSERS: &[&str] = &[
    "chrome.exe", "msedge.exe", "brave.exe", "firefox.exe", "opera.exe",
    "vivaldi.exe", "browser.exe", "arc.exe", "chromium.exe",
];

/// Никогда не слать Ctrl+C сюда: в терминалах и удалённых рабочих столах он прерывает программы.
const NO_COPY_CLASSES: &[&str] = &[
    "Chrome_WidgetWin", "MozillaWindowClass", "ConsoleWindowClass",
    "CASCADIA_HOSTING_WINDOW_CLASS", "PseudoConsoleWindow", "mintty", "PuTTY",
    "VirtualConsoleClass", "TMobaXtermForm", "SunAwtFrame", "Emacs", "Vim",
];

const NO_COPY_EXES: &[&str] = &[
    "windowsterminal.exe", "wt.exe", "openconsole.exe", "conhost.exe", "cmd.exe", "powershell.exe",
    "pwsh.exe", "wsl.exe", "bash.exe", "mintty.exe", "putty.exe", "kitty.exe", "alacritty.exe",
    "wezterm-gui.exe", "mobaxterm.exe", "termius.exe", "tabby.exe", "hyper.exe", "warp.exe",
    "anydesk.exe", "hoptodesk.exe", "rustdesk.exe", "teamviewer.exe", "mstsc.exe", "msrdc.exe",
    "vmconnect.exe", "virtualboxvm.exe", "vmware.exe", "vmplayer.exe", "vncviewer.exe",
    "tvnviewer.exe", "parsecd.exe", "rdcman.exe", "mremoteng.exe", "royalts.exe",
    "code.exe", "antigravity.exe", "cursor.exe", "windsurf.exe", "zed.exe", "devenv.exe",
    "idea64.exe", "pycharm64.exe", "webstorm64.exe", "goland64.exe", "clion64.exe", "rider64.exe",
    "datagrip64.exe", "phpstorm64.exe", "rubymine64.exe", "rustrover64.exe", "studio64.exe",
    "emacs.exe", "gvim.exe", "excel.exe", "python.exe", "pythonw.exe",
];

/// `re.sub(r"[*●•]", "", title).strip().casefold()`.
pub fn title_key(title: &str) -> String {
    let cleaned: String = title.chars().filter(|c| !matches!(c, '*' | '●' | '•')).collect();
    casefold_str(py_strip(&cleaned))
}

/// Тот же заголовок окна без отметок «изменён», которые добавляют редакторы.
pub fn same_title(a: &str, b: &str) -> bool {
    let key = title_key(a);
    !key.is_empty() && key == title_key(b)
}

/// Опознание страницы или чата: адрес, если он настоящий, иначе заголовок.
pub fn page_key(url: &str, name: &str) -> String {
    let url = py_strip(url);
    let lower = url.to_lowercase();
    if ["http://", "https://", "file://"].iter().any(|p| lower.starts_with(p)) {
        return url.split('#').next().unwrap_or("").to_string();
    }
    title_key(name)
}

/// `\w` модуля `re` Python для строки: буква или цифра Unicode, или «_».
fn word_char(c: char) -> bool {
    c == '_' || (c.is_alphanumeric() && !unicode_normalization::char::is_combining_mark(c))
}

/// Буква образца `re.IGNORECASE`: ASCII без учёта регистра; «s» совпадает и с «ſ» (U+017F).
fn letter_eq(c: char, pattern: char) -> bool {
    c.to_ascii_lowercase() == pattern || (pattern == 's' && c == '\u{17F}')
}

/// `re.search(r"\.(xlsx?|xlsm|xlsb|ods|csv)\b", title, re.IGNORECASE)`.
fn spreadsheet_title(title: &str) -> bool {
    const EXTENSIONS: [&str; 6] = ["xlsx", "xls", "xlsm", "xlsb", "ods", "csv"];
    let chars: Vec<char> = title.chars().collect();
    for (dot, _) in chars.iter().enumerate().filter(|(_, &c)| c == '.') {
        for ext in EXTENSIONS {
            let end = dot + 1 + ext.len();
            if end > chars.len() {
                continue;
            }
            let fits = chars[dot + 1..end].iter().zip(ext.chars()).all(|(&c, p)| letter_eq(c, p));
            if fits && chars.get(end).map_or(true, |&next| !word_char(next)) {
                return true;
            }
        }
    }
    false
}

/// Можно слать Ctrl+C: не терминал, не удалённый экран, не IDE, не браузер, не таблица.
pub fn copy_allowed(class_name: &str, exe: &str, title: &str) -> bool {
    !NO_COPY_EXES.contains(&exe)
        && !NO_COPY_CLASSES.iter().any(|prefix| class_name.starts_with(prefix))
        && !spreadsheet_title(title)
}

/// `segment_rects[s]` — прямоугольник сегмента `s`; в Python там мог быть `None` —
/// здесь «нет прямоугольника» = пустой `Rect::default()` (договорённость каркаса).
pub fn choose_capture(flat: &FlatDoc, quote: &[char], segment_rects: &[Rect], selection_rects: &[Rect], cursor: Point) -> Option<usize> {
    let starts = occurrences(&flat.text, quote, 100_000);
    if starts.len() <= 1 {
        return starts.first().copied();
    }
    let score = |start: usize| -> (i64, f64) {
        let rects: Vec<Rect> = flat
            .spans(start as isize, (start + quote.len()) as isize)
            .into_iter()
            .filter_map(|(s, _, _)| segment_rects.get(s).copied())
            .filter(|r| *r != Rect::default())
            .collect();
        let covered: i64 = rects.iter().map(|a| selection_rects.iter().map(|b| overlap_area(*a, *b)).sum::<i64>()).sum();
        let near = rects.iter().map(|r| distance(cursor, *r)).fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.min(d))));
        (covered, -near.unwrap_or(1e9))
    };
    // max(): первый из наибольших.
    let mut best: Option<(usize, (i64, f64))> = None;
    for start in starts {
        let s = score(start);
        let better = match best {
            None => true,
            Some((_, top)) => s.0 > top.0 || (s.0 == top.0 && s.1 > top.1),
        };
        if better {
            best = Some((start, s));
        }
    }
    best.map(|(start, _)| start)
}

/// Позиции чтения этой программы, найденные вместе со своим контекстом в тексте документа.
pub fn replaced_ids(items: &[Bookmark], exe: &str, flat_text: &[char], keep_id: Option<&str>) -> Vec<String> {
    items
        .iter()
        .filter(|item| {
            item.kind == Kind::Pos
                && item.exe == exe
                && Some(item.id.as_str()) != keep_id
                && item.mode == Mode::Uia
                && strong_match(flat_text, item, 0.6)
        })
        .map(|item| item.id.clone())
        .collect()
}

/// Позиции чтения по тексту экрана той же программы и того же заголовка окна (тот же документ).
pub fn ocr_replaced_ids(items: &[Bookmark], exe: &str, title: &str, keep_id: Option<&str>) -> Vec<String> {
    items
        .iter()
        .filter(|item| {
            item.kind == Kind::Pos
                && item.exe == exe
                && Some(item.id.as_str()) != keep_id
                && item.mode == Mode::Ocr
                && same_title(&item.title, title)
        })
        .map(|item| item.id.clone())
        .collect()
}

/// Укоротить цитату так, чтобы она шла не больше чем через `max_segments` сегментов.
pub fn cut_to_segments(flat: &FlatDoc, start: usize, length: usize, max_segments: usize) -> usize {
    let spans = flat.spans(start as isize, start.saturating_add(length) as isize);
    if spans.len() <= max_segments {
        return length;
    }
    // spans[max_segments - 1]; при 0 Python берёт spans[-1].
    let last_allowed = match max_segments.checked_sub(1) {
        Some(i) => spans[i].0,
        None => spans[spans.len() - 1].0,
    } as u32;
    let stop = start.saturating_add(length);
    let mut end = start;
    while end < stop && end < flat.seg.len() && flat.seg[end] <= last_allowed {
        end += 1;
    }
    end - start
}

/// `located`: (id, kind, created, хеш ScopeKey) -> id более старых позиций, делящих документ с новой.
pub fn self_heal(located: &[(String, Kind, String, ScopeKeyHash)]) -> Vec<String> {
    let mut order: Vec<&(String, Kind, String, ScopeKeyHash)> = located.iter().collect();
    // sorted(key=created, reverse=True) устойчив: равные сохраняют исходный порядок.
    order.sort_by(|a, b| b.2.cmp(&a.2));
    let mut newest: HashSet<ScopeKeyHash> = HashSet::new();
    let mut drop = Vec::new();
    for (id, kind, _, scope) in order {
        if *kind != Kind::Pos {
            continue;
        }
        if !newest.insert(*scope) {
            drop.push(id.clone());
        }
    }
    drop
}
