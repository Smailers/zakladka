//! Постановка и переход по клавише (часть 5; P 1738–1816, 1831–1903).
//!
//! `do_mark`: выделение UIA (или Ctrl+C + OCR) -> закладка по элементам текста или по тексту
//! экрана; повторное нажатие на отмеченном тексте снимает закладку; без выделения — переход.

use std::time::Duration;

use crate::core::flatten::{flatten, py_isspace, FlatDoc, CONTEXT, MAX_QUOTE};
use crate::core::geometry::{distance, grow, intersect, overlap_area};
use crate::core::matching::{context_score, find_quote, fuzzy_best, fuzzy_candidates};
use crate::core::rules::{choose_capture, copy_allowed, cut_to_segments, ocr_replaced_ids, page_key, replaced_ids};
use crate::core::text::one_line;
use crate::store::{iso_seconds, log_timing, new_id, now_local};
use crate::tracker::state::{FlatEntry, Item, Worker};
use crate::types::{Bookmark, Config, Hwnd, Kind, Mode, Point, Rect};
use crate::uia::{ScopeKey, Selection};
use crate::win::monotonic;

/// Chromium строит дерево доступности по первому запросу (P 1747).
const CHROMIUM_WARMUP: Duration = Duration::from_millis(1200);
/// Ожидание ответа на Ctrl+C (`copy_selection(wait=0.8)` в Python).
const COPY_WAIT_S: f64 = 0.8;
/// Длина цитаты по тексту экрана (P 1833).
const SCREEN_QUOTE: usize = 200;
/// Пределы сохраняемых полей, в кодовых точках (P 1784–1786).
const LABEL_LEN: usize = 80;
const TITLE_LEN: usize = 120;
const DOC_LEN: usize = 200;
const URL_LEN: usize = 500;
/// Значения Python по умолчанию: `find_quote(min_len=12)`, `cut_to_segments(max_segments=12)`,
/// `fuzzy_candidates(min_ratio=0.75, gram=5, limit=30)`, `fuzzy_best(min_score=0.6)`.
const QUOTE_MIN: usize = 12;
const MAX_SEGMENTS: usize = 12;
const FUZZY: (f64, usize, usize) = (0.75, 5, 30);
const MIN_SCORE: f64 = 0.6;

/// Программе нужен OCR, а в Windows его нет (урезанная сборка, нет языка распознавания).
const OCR_MISSING: &str = "Распознавание текста недоступно: добавьте язык в Параметры → Время и язык → Язык";

/// Чем кончилась попытка привязки.
enum Made {
    Done { mode: Mode, replaced: bool },
    Failed,
    NoOcr, // нужно распознавание, а его нет
}

/// Python `not text.strip()`.
fn blank(text: &str) -> bool {
    text.chars().all(py_isspace)
}

/// Python `s[:n]` — первые `n` кодовых точек.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Python `seq[a:b]` для `0 <= a`: границы обрезаются по длине.
fn slice(text: &[char], a: usize, b: usize) -> &[char] {
    let b = b.min(text.len());
    &text[a.min(b)..b]
}

/// Новая закладка (P 1781–1786): label — одной строкой, первые 80 символов; длины в `char`.
#[allow(clippy::too_many_arguments)]
fn new_bookmark(
    exe: &str,
    extra: bool,
    mode: Mode,
    quote: &[char],
    prefix: &[char],
    suffix: &[char],
    text: &str,
    title: &str,
    url: &str,
    doc: &str,
) -> Item {
    let b = Bookmark {
        id: new_id(),
        exe: exe.to_string(),
        kind: if extra { Kind::Extra } else { Kind::Pos },
        mode,
        quote: quote.iter().collect(),
        prefix: prefix.iter().collect(),
        suffix: suffix.iter().collect(),
        label: head(&one_line(text), LABEL_LEN),
        title: head(title, TITLE_LEN),
        created: iso_seconds(&now_local()),
        page: if url.is_empty() && doc.is_empty() { String::new() } else { page_key(url, doc) },
        doc: head(doc, DOC_LEN),
        url: head(url, URL_LEN),
    };
    Item { b, quote: quote.to_vec(), prefix: prefix.to_vec(), suffix: suffix.to_vec() }
}

impl Worker {
    pub fn do_mark(&mut self, hwnd: Hwnd, cursor: Point, extra: bool) {
        if hwnd == 0 {
            return;
        }
        let started = monotonic();
        let (pid, exe) = self.api.process(hwnd);
        if pid == self.api.own_pid() || exe.is_empty() {
            return;
        }
        let mut selection = self.uia.selection(pid, cursor);
        if blank(&selection.text) && self.api.is_chromium(hwnd) && !selection.any_pattern {
            std::thread::sleep(CHROMIUM_WARMUP); // Chromium строит дерево доступности по первому запросу
            selection = self.uia.selection(pid, cursor);
        }
        let mut text = selection.text.clone();
        let mut copied = false;
        if blank(&text) && self.copy_fallback() && self.ocr_available && self.copy_allowed_for(hwnd, &exe) && self.ensure_ocr() {
            self.render_empty(); // во время Ctrl+C метка не может следовать за текстом
            text = self.api.copy_selection(self.owner, hwnd, COPY_WAIT_S).unwrap_or_default();
            copied = !text.is_empty();
        }
        if blank(&text) {
            if extra {
                let hint = if self.needs_missing_ocr(hwnd, &exe, selection.any_pattern) { OCR_MISSING } else { "Нет выделения" };
                return self.toast(hint.to_string(), Some(cursor), true);
            }
            return self.do_jump(hwnd, &exe, cursor, selection.any_pattern);
        }
        if hwnd != self.target {
            self.tick();
        }
        let probe = if selection.rects.is_empty() {
            vec![Rect { l: cursor.x - 2, t: cursor.y - 2, r: cursor.x + 2, b: cursor.y + 2 }]
        } else {
            selection.rects.clone()
        };
        for (id, rects) in self.visible_marks() {
            if probe.iter().any(|a| rects.iter().any(|b| overlap_area(*a, grow(*b, 4)) > 0)) {
                self.delete(&[id], true);
                return self.toast("Снята".to_string(), Some(cursor), false);
            }
        }
        let mut made = Made::Failed;
        if selection.scope.is_some() {
            made = self.mark_by_elements(hwnd, &exe, &text, &selection, cursor, extra);
        }
        if !matches!(made, Made::Done { .. }) {
            made = if self.ocr_available {
                self.mark_by_screen(hwnd, &exe, &text, &selection.rects, cursor, extra, copied)
            } else if self.ocr_missing() {
                Made::NoOcr
            } else {
                Made::Failed
            };
        }
        match made {
            Made::Done { mode, replaced } => {
                let code = if mode == Mode::Ocr { "MARK_OCR" } else { "MARK" };
                log_timing(&self.shared, code, (monotonic() - started) * 1000.0);
                self.toast(if replaced { "Перенесена" } else { "Поставлена" }.to_string(), Some(cursor), false);
            }
            Made::NoOcr => self.toast(OCR_MISSING.to_string(), Some(cursor), true),
            Made::Failed => self.toast("Не получилось привязаться к этому тексту".to_string(), Some(cursor), true),
        }
    }

    pub fn do_jump(&mut self, hwnd: Hwnd, exe: &str, cursor: Point, any_pattern: bool) {
        if !self.items.iter().any(|item| item.b.exe == exe) {
            let hint = if any_pattern || self.ocr_available {
                format!("Выделите текст и нажмите {}", self.setting(|c| c.hotkey_mark.clone()))
            } else if self.needs_missing_ocr(hwnd, exe, any_pattern) {
                OCR_MISSING.to_string()
            } else {
                "Это окно не отдаёт выделенный текст".to_string()
            };
            return self.toast(hint, Some(cursor), true);
        }
        if hwnd != self.target {
            self.tick();
        }
        // sorted(key=created, reverse=True), затем sort(key=kind != "pos"): позиции чтения первыми,
        // новые раньше старых; при равном времени — по id, чтобы порядок не зависел от HashMap.
        let mut order: Vec<(&str, &str, bool)> =
            self.tracks.values().map(|t| (t.id.as_str(), t.created.as_str(), t.kind != Kind::Pos)).collect();
        order.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| b.1.cmp(a.1)).then_with(|| a.0.cmp(b.0)));
        if order.is_empty() {
            let hint = if self.items.iter().any(|item| item.b.exe == exe && item.b.mode == Mode::Ocr) {
                "Прокрутить сюда нельзя: подсветится, когда текст будет на экране"
            } else {
                "В этом окне закладки не видно — откройте её из списка (значок в трее)"
            };
            return self.toast(hint.to_string(), Some(cursor), true);
        }
        let id = order[self.jump_cycle % order.len()].0.to_string();
        self.jump_cycle += 1;
        let scrolled = self
            .tracks
            .get(&id)
            .and_then(|track| track.segs.first().map(|seg| self.uia.scroll_to(&seg.element, track.pattern.as_ref())))
            .unwrap_or(false);
        if !scrolled {
            self.toast("Не получилось прокрутить к закладке".to_string(), Some(cursor), false);
        }
    }

    // ------------------------------------------------------------ приватные помощники

    /// Значение из настроек (замок, отравленный паникой другого потока, тоже читается).
    fn setting<T>(&self, read: impl FnOnce(&Config) -> T) -> T {
        match self.shared.config.read() {
            Ok(config) => read(&config),
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    fn copy_fallback(&self) -> bool {
        self.setting(|c| c.copy_fallback)
    }

    /// Распознавание включено в настройках, но в этой Windows его нет (раздел 10 плана).
    fn ocr_missing(&self) -> bool {
        self.setting(|c| c.ocr) && !self.ocr_available
    }

    fn copy_allowed_for(&self, hwnd: Hwnd, exe: &str) -> bool {
        copy_allowed(&self.api.class_name(hwnd), exe, &self.api.title(hwnd))
    }

    /// Выделение можно было бы взять только через Ctrl+C + OCR, а распознавания в Windows нет.
    fn needs_missing_ocr(&self, hwnd: Hwnd, exe: &str, any_pattern: bool) -> bool {
        !any_pattern && self.ocr_missing() && self.copy_fallback() && self.copy_allowed_for(hwnd, exe)
    }

    /// Привязка к текстовым элементам UI Automation (P 1788–1816).
    fn mark_by_elements(&mut self, hwnd: Hwnd, exe: &str, text: &str, selection: &Selection, cursor: Point, extra: bool) -> Made {
        let Some(scope) = selection.scope.clone() else { return Made::Failed };
        let key = self.uia.rid(&scope);
        let segs = match self.uia.segments(&scope, 30_000) {
            Ok(segs) => segs,
            Err(()) => {
                crate::store::log_error(&self.shared, "MARK_SEGMENTS");
                return Made::Failed;
            }
        };
        let flat = FlatDoc::new(&segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
        let (wanted, _) = flatten(text);
        let Some(piece) = find_quote(&flat.text, slice(&wanted, 0, MAX_QUOTE), QUOTE_MIN) else { return Made::Failed };
        let Some(key) = key else { return Made::Failed };
        let rects: Vec<Rect> = segs.iter().map(|s| s.rect).collect();
        let Some(start) = choose_capture(&flat, &piece, &rects, &selection.rects, cursor) else { return Made::Failed };
        let piece = slice(&piece, 0, cut_to_segments(&flat, start, piece.len(), MAX_SEGMENTS)).to_vec(); // цена слежения ограничена
        let end = start + piece.len();
        let (url, doc) = self.uia.document_info(&scope);
        let item = new_bookmark(
            exe,
            extra,
            Mode::Uia,
            &piece,
            slice(&flat.text, start.saturating_sub(CONTEXT), start),
            slice(&flat.text, end, end + CONTEXT),
            text,
            &self.api.title(hwnd),
            &url,
            &doc,
        );
        let removed = if extra {
            Vec::new()
        } else {
            let books: Vec<Bookmark> = self.items.iter().map(|item| item.b.clone()).collect();
            replaced_ids(&books, exe, &flat.text, None)
        };
        if !removed.is_empty() {
            self.delete(&removed, false); // перенос позиции чтения, а не удаление
        }
        let id = item.b.id.clone();
        self.items.push(item);
        self.save();
        if hwnd == self.target {
            let key = ScopeKey::Doc(key);
            let pattern = self.text_pattern_for(&key, &scope);
            if let Some(item) = self.items.iter().find(|item| item.b.id == id) {
                let track = self.make_track(item, &key, pattern, &flat, &segs, start);
                self.flats.insert(key, FlatEntry { flat, segs, next: monotonic() + 2.0, dirty: false, delay: 2.0 });
                self.tracks.insert(id, track);
            }
        }
        Made::Done { mode: Mode::Uia, replaced: !removed.is_empty() }
    }

    /// Привязка к распознанному на экране тексту (P 1831–1882).
    #[allow(clippy::too_many_arguments)]
    fn mark_by_screen(
        &mut self,
        hwnd: Hwnd,
        exe: &str,
        text: &str,
        selection_rects: &[Rect],
        cursor: Point,
        extra: bool,
        _copied: bool, // как в Python: принимается, но не влияет на выбор
    ) -> Made {
        if !self.ensure_ocr() {
            return Made::NoOcr;
        }
        let (wanted, _) = flatten(text);
        let quote = slice(&wanted, 0, SCREEN_QUOTE);
        let Some((flat, rects)) = self.screen_words(hwnd, text) else {
            crate::store::log_error(&self.shared, "MARK_SCREEN");
            return Made::Failed;
        };
        // (start, ratio, covered, near) — только места, у которых видна хотя бы одна строка.
        let mut best: Option<(usize, f64, i64, f64)> = None;
        for (candidate, ratio) in fuzzy_candidates(&flat.text, quote, FUZZY.0, FUZZY.1, FUZZY.2) {
            let lines = self.screen_lines(&flat, &rects, candidate, quote.len());
            if !lines.iter().any(|line| self.uncovered(*line)) {
                continue;
            }
            // min(key=(-covered, near, -ratio)): больше перекрытие с выделением, ближе к курсору,
            // выше сходство; при равенстве — первое.
            let covered: i64 = lines.iter().map(|a| selection_rects.iter().map(|b| overlap_area(*a, *b)).sum::<i64>()).sum();
            let near = lines.iter().map(|line| distance(cursor, *line)).fold(1e9_f64, f64::min);
            let better = match best {
                None => true,
                Some((_, top_ratio, top_covered, top_near)) => {
                    covered > top_covered
                        || (covered == top_covered && (near < top_near || (near == top_near && ratio > top_ratio)))
                }
            };
            if better {
                best = Some((candidate, ratio, covered, near));
            }
        }
        let Some((start, ..)) = best else { return Made::Failed };
        let end = start + quote.len();
        let title = self.api.title(hwnd);
        let item = new_bookmark(
            exe,
            extra,
            Mode::Ocr,
            slice(&flat.text, start, end),
            slice(&flat.text, start.saturating_sub(CONTEXT), start),
            slice(&flat.text, end, end + CONTEXT),
            text,
            &title,
            "",
            "",
        );
        let mut removed = Vec::new();
        if !extra {
            // Только позиции, видные прямо сейчас на этом экране, вместе со своим контекстом.
            let books: Vec<Bookmark> = self.items.iter().map(|item| item.b.clone()).collect();
            let same = ocr_replaced_ids(&books, exe, &title, None);
            for old in self.items.iter().filter(|item| same.contains(&item.b.id)) {
                let Some(found) = fuzzy_best(&flat.text, &old.quote, &old.prefix, &old.suffix, MIN_SCORE) else { continue };
                if context_score(&flat.text, found, found + old.quote.len(), &old.prefix, &old.suffix) >= MIN_SCORE {
                    removed.push(old.b.id.clone());
                }
            }
        }
        if !removed.is_empty() {
            self.delete(&removed, true); // догадка по тексту экрана: можно «Вернуть»
        }
        let id = item.b.id.clone();
        self.items.push(item);
        self.save();
        if hwnd == self.target {
            let lines: Vec<Rect> = self
                .screen_lines(&flat, &rects, start, end - start)
                .into_iter()
                .filter_map(|line| intersect(Some(line), self.window))
                .filter(|r| self.uncovered(*r))
                .collect();
            if !lines.is_empty() {
                self.ocr_visible.insert(id, lines);
            }
            self.ocr_thumb = self.window.and_then(|w| self.api.capture(w, Some((96, 60)))).map(|bmp| bmp.bgra);
            self.ocr_window = self.window;
            self.ocr_stable = 2;
            self.ocr_dirty = false;
            self.ocr_next = monotonic() + 10.0;
        }
        Made::Done { mode: Mode::Ocr, replaced: !removed.is_empty() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn new_bookmark_limits_in_chars() {
        let text = format!("  {}\n\t{}  ", "ё".repeat(50), "я".repeat(50));
        let title = "т".repeat(130);
        let doc = "д".repeat(210);
        let url = format!("https://example.org/{}#part", "u".repeat(600));
        let item = new_bookmark("a.exe", false, Mode::Uia, &chars("abc"), &chars("p"), &chars("s"), &text, &title, &url, &doc);
        let b = &item.b;
        assert_eq!(b.label.chars().count(), 80);
        assert_eq!(b.label, format!("{} {}", "ё".repeat(50), "я".repeat(29)));
        assert_eq!(b.title.chars().count(), 120);
        assert_eq!(b.doc.chars().count(), 200);
        assert_eq!(b.url.chars().count(), 500);
        assert_eq!(b.page, page_key(&url, &doc), "страница — по полному адресу");
        assert!(!b.page.contains('#'));
        assert_eq!(b.kind, Kind::Pos);
        assert_eq!((b.quote.as_str(), b.prefix.as_str(), b.suffix.as_str()), ("abc", "p", "s"));
        assert_eq!(item.quote, chars("abc"));
        assert_eq!(b.id.len(), 32);
        assert_eq!(b.created.len(), 19, "isoformat(timespec=seconds)");
    }

    #[test]
    fn new_bookmark_without_page() {
        let item = new_bookmark("a.exe", true, Mode::Ocr, &[], &[], &[], "x", "", "", "");
        assert_eq!(item.b.page, "");
        assert_eq!(item.b.kind, Kind::Extra);
        assert_eq!(item.b.mode, Mode::Ocr);
    }

    #[test]
    fn python_slices() {
        let t = chars("abcdef");
        assert_eq!(slice(&t, 0, 3), &t[..3]);
        assert_eq!(slice(&t, 4, 100), &t[4..]);
        assert!(slice(&t, 10, 20).is_empty());
        assert_eq!(head("абв", 2), "аб");
        assert!(blank(" \u{a0}\t\n") && !blank(" x "));
    }
}
