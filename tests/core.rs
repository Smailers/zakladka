//! Часть 1, `core`: перенесённые тесты `test_text_bookmark.py` (38 из 51, остальные — в store.rs),
//! `ColorMathTests`, `TextTests`, `FitTextTests` из `test_panel.py` и сверка с эталонами Python
//! (`tests\golden\*.json`, их пишет `tools\gen_golden.py`).

use std::fmt::Debug;
use std::path::Path;

use serde_json::Value;

use zakladka::core::color::*;
use zakladka::core::flatten::*;
use zakladka::core::geometry::*;
use zakladka::core::hotkey::*;
use zakladka::core::matching::*;
use zakladka::core::rules::*;
use zakladka::core::text::*;
use zakladka::types::{Bookmark, Dir, Kind, LabelColor, Mode, Point, Rect, Rgb, Status};

// ---------------------------------------------------------------- помощники

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn flat(s: &str) -> Vec<char> {
    flatten(s).0
}

fn flat_str(s: &str) -> String {
    flatten(s).0.into_iter().collect()
}

fn r(l: i32, t: i32, rr: i32, b: i32) -> Rect {
    Rect { l, t, r: rr, b }
}

fn p(x: i32, y: i32) -> Point {
    Point { x, y }
}

fn rgb(s: &str) -> Rgb {
    Rgb::parse(s).unwrap_or_else(|| panic!("цвет {s}"))
}

fn bookmark() -> Bookmark {
    Bookmark {
        id: "a".into(),
        exe: "claude.exe".into(),
        kind: Kind::Pos,
        mode: Mode::Uia,
        quote: "цитата".into(),
        prefix: String::new(),
        suffix: String::new(),
        label: "Цитата".into(),
        title: String::new(),
        created: "2026-11-19T10:00:00".into(),
        page: String::new(),
        doc: String::new(),
        url: String::new(),
    }
}

fn bm(id: &str, quote: &str) -> Bookmark {
    Bookmark { id: id.into(), quote: quote.into(), ..bookmark() }
}

fn golden(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("ожидался массив: {v}"))
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("ожидалась строка: {v}"))
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("ожидалось число: {v}"))
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("ожидалось целое: {v}"))
}

fn idx(v: &Value) -> usize {
    int(v) as usize
}

fn opt_idx(v: &Value) -> Option<usize> {
    if v.is_null() {
        None
    } else {
        Some(idx(v))
    }
}

fn rect_of(v: &Value) -> Rect {
    let a = arr(v);
    r(int(&a[0]) as i32, int(&a[1]) as i32, int(&a[2]) as i32, int(&a[3]) as i32)
}

fn rects_of(v: &Value) -> Vec<Rect> {
    arr(v).iter().map(rect_of).collect()
}

/// Сверка с эталоном: копит расхождения и сообщает первые из них.
struct Check {
    name: &'static str,
    count: usize,
    failures: Vec<String>,
}

impl Check {
    fn new(name: &'static str) -> Check {
        Check { name, count: 0, failures: Vec::new() }
    }

    fn eq<T: PartialEq + Debug>(&mut self, case: impl Debug, got: T, want: T) {
        self.count += 1;
        if got != want {
            self.failures.push(format!("{case:?}\n    got:  {got:?}\n    want: {want:?}"));
        }
    }

    fn close(&mut self, case: impl Debug, got: f64, want: f64) {
        self.count += 1;
        if (got - want).abs() > 1e-9 {
            self.failures.push(format!("{case:?}\n    got:  {got:?}\n    want: {want:?}"));
        }
    }

    fn finish(self) -> usize {
        if !self.failures.is_empty() {
            let shown: Vec<&String> = self.failures.iter().take(12).collect();
            panic!(
                "{}: {} из {} расходятся с Python:\n{}",
                self.name,
                self.failures.len(),
                self.count,
                shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
            );
        }
        eprintln!("{}: {} эталонов совпали", self.name, self.count);
        self.count
    }
}

// ---------------------------------------------------------------- test_text_bookmark.py

mod flatten_tests {
    use super::*;

    #[test]
    fn test_whitespace_case_yo_nbsp_zero_width() {
        let (text, index) = flatten("Ёлка\u{a0} и\u{200b}ЛЕС\u{fffc}!");
        assert_eq!(text.iter().collect::<String>(), "елкаилес!");
        assert_eq!(index, vec![0, 1, 2, 3, 6, 8, 9, 10, 12]);
    }

    #[test]
    fn test_nfkc() {
        assert_eq!(flat_str("ﬁ①"), "fi1");
    }

    #[test]
    fn test_flatdoc_spans_cross_segments() {
        let doc = FlatDoc::new(&["Hello ", "big", " world"]);
        assert_eq!(doc.text.iter().collect::<String>(), "hellobigworld");
        let start = find(&doc.text, &chars("obigw"), 0).expect("есть") as isize;
        assert_eq!(doc.spans(start, start + 5), vec![(0, 4, 5), (1, 0, 3), (2, 1, 2)]);
    }

    #[test]
    fn test_spans_clamped() {
        let doc = FlatDoc::new(&["abc"]);
        assert_eq!(doc.spans(-5, 99), vec![(0, 0, 3)]);
    }
}

mod matching_tests {
    use super::*;

    #[test]
    fn test_occurrences() {
        assert_eq!(occurrences(&chars("abcabcab"), &chars("ab"), 50), vec![0, 3, 6]);
        assert_eq!(occurrences(&chars("abc"), &[], 50), Vec::<usize>::new());
    }

    #[test]
    fn test_best_occurrence_uses_context() {
        let text = chars("началоготовоконецxxxxготовоyyyy");
        let start = best_occurrence(&text, &chars("готово"), &chars("xxxx"), &chars("yyyy"), 0.6);
        assert_eq!(start, Some(21)); // text.rfind("готово")
    }

    #[test]
    fn test_short_quote_needs_context() {
        let text = chars("одинготоводругое");
        assert_eq!(best_occurrence(&text, &chars("готово"), &chars("совсемдругоеначало"), &chars("ипродолжение"), 0.6), None);
        assert_eq!(best_occurrence(&text, &chars("готово"), &chars("один"), &chars("друг"), 0.6), Some(4));
    }

    #[test]
    fn test_long_quote_without_context_is_accepted() {
        let quote = flat("это достаточно длинная цитата для доверия");
        let mut text = chars("zz");
        text.extend(&quote);
        assert_eq!(best_occurrence(&text, &quote, &chars("ничегообщего"), &chars("совсем"), 0.6), Some(2));
    }

    #[test]
    fn test_find_quote_pieces() {
        assert_eq!(find_quote(&chars("abcdefghijklmnopqrstuvwxyz"), &chars("defgh"), 12), Some(chars("defgh")));
        let text = chars("0123456789abcdefghijklmnopqrst");
        let piece = find_quote(&text, &chars("XX0123456789abcdefghQQQQ"), 12).expect("кусок");
        assert!(find(&text, &piece, 0).is_some() && piece.len() >= 12);
        assert_eq!(find_quote(&chars("abc"), &chars("zzzzzzzzzzzzzzzz"), 12), None);
        assert_eq!(find_quote(&chars("abc"), &[], 12), None);
    }

    #[test]
    fn test_choose_capture_prefers_selection_then_cursor() {
        let doc = FlatDoc::new(&["готово", "текст", "готово"]);
        let rects = [r(0, 0, 50, 20), r(0, 30, 50, 50), r(0, 100, 50, 120)];
        let q = chars("готово");
        assert_eq!(choose_capture(&doc, &q, &rects, &[r(5, 105, 20, 115)], p(0, 0)), Some(11));
        assert_eq!(choose_capture(&doc, &q, &rects, &[], p(10, 5)), Some(0));
        assert_eq!(choose_capture(&doc, &q, &rects, &[], p(10, 118)), Some(11));
        assert_eq!(choose_capture(&doc, &chars("нет"), &rects, &[], p(0, 0)), None);
    }
}

mod policy_tests {
    use super::*;

    #[test]
    fn test_replaced_only_positions_of_same_app_found_here() {
        let q = "перваяцитатаздесьдлиннаядлинная";
        let items = vec![
            bm("1", q),
            Bookmark { kind: Kind::Extra, ..bm("2", q) },
            Bookmark { exe: "chrome.exe".into(), ..bm("3", q) },
            bm("4", "другойчатсовсемдругаяцитатаоченьдлинная"),
        ];
        let text = chars(&format!("вначале{q}потом"));
        assert_eq!(replaced_ids(&items, "claude.exe", &text, Some("new")), vec!["1".to_string()]);
    }

    #[test]
    fn test_self_heal_keeps_newest_position_per_document() {
        let (doc, doc2): (ScopeKeyHash, ScopeKeyHash) = (1, 2);
        let located = vec![
            ("old".to_string(), Kind::Pos, "2026-11-18T10:00:00".to_string(), doc),
            ("new".to_string(), Kind::Pos, "2026-11-19T10:00:00".to_string(), doc),
            ("extra".to_string(), Kind::Extra, "2026-11-17T10:00:00".to_string(), doc),
            ("other".to_string(), Kind::Pos, "2026-11-10T10:00:00".to_string(), doc2),
        ];
        assert_eq!(self_heal(&located), vec!["old".to_string()]);
    }
}

mod geometry_tests {
    use super::*;

    #[test]
    fn test_to_rects_drops_slivers() {
        assert_eq!(to_rects(&[10.4, 20.0, 100.0, 30.0, 5.0, 5.0, 1.0, 30.0]), vec![r(10, 20, 110, 50)]);
        assert_eq!(to_rects(&[]), Vec::<Rect>::new());
    }

    #[test]
    fn test_intersect_area() {
        assert_eq!(intersect(Some(r(0, 0, 10, 10)), Some(r(5, 5, 20, 20))), Some(r(5, 5, 10, 10)));
        assert_eq!(intersect(Some(r(0, 0, 10, 10)), Some(r(10, 0, 20, 10))), None);
        assert_eq!(intersect(None, Some(r(0, 0, 1, 1))), None);
        assert_eq!(overlap_area(r(0, 0, 10, 10), r(5, 5, 20, 20)), 25);
    }

    #[test]
    fn test_merge_lines() {
        let rects = [r(64, 50, 200, 70), r(10, 10, 50, 30), r(52, 11, 90, 31), r(10, 50, 60, 70)];
        assert_eq!(merge_lines(&rects, 14), vec![r(10, 10, 90, 31), r(10, 50, 200, 70)]);
        let far = vec![r(10, 50, 60, 70), r(100, 50, 200, 70)]; // широкий промежуток — не одна строка
        assert_eq!(merge_lines(&far, 14), far);
        let words = [
            r(596, -520, 630, -508), r(635, -517, 686, -509), r(689, -520, 737, -508),
            r(740, -517, 771, -508), r(773, -512, 775, -509), r(778, -520, 816, -509),
            r(820, -520, 874, -508),
        ]; // слова OCR разной высоты, ONLYOFFICE
        assert_eq!(merge_lines(&words, 14), vec![r(596, -520, 874, -508)]);
    }

    #[test]
    fn test_direction() {
        let view = Some(r(0, 100, 500, 900));
        assert_eq!(direction(&[r(0, -300, 10, -280)], view), Some(Dir::Up));
        assert_eq!(direction(&[r(0, 950, 10, 970)], view), Some(Dir::Down));
        assert_eq!(direction(&[r(0, 500, 10, 520)], view), None);
        assert_eq!(direction(&[], view), None);
    }

    #[test]
    fn test_distance_center_grow() {
        assert_eq!(distance(p(5, 5), r(0, 0, 10, 10)), 0.0);
        assert_eq!(distance(p(13, 14), r(0, 0, 10, 10)), 5.0);
        assert_eq!(center(r(0, 0, 10, 20)), p(5, 10));
        assert_eq!(grow(r(5, 5, 6, 6), 2), r(3, 3, 8, 8));
    }
}

mod config_tests {
    use super::*;

    #[test]
    fn test_parse_hotkey() {
        assert_eq!(parse_hotkey("Ctrl+Alt+M"), Ok((0x4003, 'M' as u32)));
        assert_eq!(parse_hotkey("ctrl + shift + f5"), Ok((0x4006, 0x74)));
        assert_eq!(parse_hotkey("Win+Space"), Ok((0x4008, 0x20)));
        for bad in ["M", "Ctrl+", "Ctrl+M+N", "Ctrl+Ж", "Ctrl+F25", ""] {
            assert!(parse_hotkey(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_safe_hotkey() {
        assert_eq!(safe_hotkey("Ctrl+Alt+Z"), Ok((0x4003, 'Z' as u32)));
        assert_eq!(safe_hotkey("Win+Shift+B").map(|x| x.1), Ok('B' as u32));
        for risky in ["Ctrl+C", "Ctrl+Z", "Shift+A", "Alt+F4", "Ctrl+Shift+Z"] {
            assert!(safe_hotkey(risky).is_err(), "{risky}");
        }
    }

    #[test]
    fn test_menu_label() {
        assert_eq!(menu_label("A & B", 48), "A && B");
        assert_eq!(menu_label(&"x".repeat(100), 10).chars().count(), 10);
    }
}

mod review_fix_tests {
    use super::*;

    #[test]
    fn test_long_quote_with_foreign_context_is_not_replaced() {
        let quote = "длиннаяфразакотораяестьвдвухчатахсразу";
        let items = vec![Bookmark { prefix: "контекстчатаа".into(), suffix: "хвостчатаа".into(), ..bm("a", quote) }];
        let other_chat = chars(&format!("совсемдругоеначало{quote}иинойконец"));
        assert_eq!(replaced_ids(&items, "claude.exe", &other_chat, None), Vec::<String>::new());
        let same_chat = chars(&format!("контекстчатаа{quote}хвостчатаа"));
        assert_eq!(replaced_ids(&items, "claude.exe", &same_chat, None), vec!["a".to_string()]);
    }

    #[test]
    fn test_screen_bookmarks_not_replaced_by_element_rule() {
        let quote = "длиннаяфразакотораяестьвдвухчатахсразу";
        let items = vec![Bookmark { mode: Mode::Ocr, ..bm("o", quote) }];
        assert_eq!(replaced_ids(&items, "claude.exe", &chars(quote), None), Vec::<String>::new());
    }

    #[test]
    fn test_ocr_replaced_by_title() {
        let title = |id: &str, t: &str, kind: Kind| Bookmark { mode: Mode::Ocr, title: t.into(), kind, ..bm(id, "цитата") };
        let items = vec![
            title("1", "Инвойс.docx - ONLYOFFICE", Kind::Pos),
            title("2", "Другой файл.docx - ONLYOFFICE", Kind::Pos),
            title("3", "Инвойс.docx - ONLYOFFICE", Kind::Extra),
        ];
        assert_eq!(ocr_replaced_ids(&items, "claude.exe", "Инвойс.docx - ONLYOFFICE", None), vec!["1".to_string()]);
        assert_eq!(ocr_replaced_ids(&items, "claude.exe", "", None), Vec::<String>::new());
    }

    #[test]
    fn test_choose_capture_beyond_fifty_occurrences() {
        let names = vec!["итог"; 80];
        let doc = FlatDoc::new(&names);
        let rects: Vec<Rect> = (0..80).map(|i| r(0, i * 20, 50, i * 20 + 15)).collect();
        let start = choose_capture(&doc, &chars("итог"), &rects, &[r(5, 61 * 20 + 2, 20, 61 * 20 + 10)], p(0, 0));
        assert_eq!(start, Some(61 * 4));
    }

    #[test]
    fn test_cut_to_segments() {
        let doc = FlatDoc::new(&vec!["ab"; 20]);
        assert_eq!(cut_to_segments(&doc, 0, 40, 12), 24);
        assert_eq!(cut_to_segments(&doc, 1, 10, 12), 10);
    }

    #[test]
    fn test_grapheme_extra() {
        assert_eq!(grapheme_extra(&chars("abc")), 0);
        assert_eq!(grapheme_extra(&chars("⚠\u{fe0f} x")), 1);
        assert_eq!(grapheme_extra(&chars("👍🏻")), 1);
        assert_eq!(grapheme_extra(&chars("👨\u{200d}👩")), 2);
        assert_eq!(grapheme_extra(&chars("e\u{301}")), 1);
    }
}

mod screen_text_tests {
    use super::*;

    #[test]
    fn test_fuzzy_candidates_tolerate_misreads() {
        let text = chars("началострокиразницамежду3Oи6Odwpdконец");
        let found = fuzzy_candidates(&text, &chars("разницамежду30и60dwpd"), 0.75, 5, 30);
        assert!(!found.is_empty());
        assert_eq!(found[0].0, "началостроки".chars().count());
        assert!(fuzzy_candidates(&chars("совсемдругойтекст"), &chars("разницамежду30и60dwpd"), 0.75, 5, 30).is_empty());
        assert!(fuzzy_candidates(&chars("abc"), &[], 0.75, 5, 30).is_empty());
    }

    #[test]
    fn test_fuzzy_best_uses_context() {
        let text = chars("первыйготовоэтоконецвторыйготовоэтохвост");
        assert_eq!(fuzzy_best(&text, &chars("готово"), &chars("второй"), &chars("этохвост"), 0.6), Some(26));
        assert_eq!(fuzzy_best(&text, &chars("готово"), &chars("нетакого"), &chars("нетакого"), 0.6), None);
    }

    #[test]
    fn test_inexact_long_match_needs_context() {
        let quote = flat("Срок оплаты счёта: до 15 ноября 2026 года");
        let other = flat("Срок оплаты счёта: до 25 декабря 2026 года");
        let around = |middle: &[char]| {
            let mut t = chars("шапка");
            t.extend(middle);
            t.extend(chars("подвал"));
            t
        };
        assert_eq!(fuzzy_best(&around(&other), &quote, &chars("своязаголовок"), &chars("свойхвост"), 0.6), None);
        assert_eq!(fuzzy_best(&around(&quote), &quote, &chars("своязаголовок"), &chars("свойхвост"), 0.6), Some(5));
    }

    #[test]
    fn test_thumb_moved() {
        let a = vec![10u8; 400];
        assert!(!thumb_moved(&a, &a, 6, 20));
        let mut few = a.clone();
        for i in (1..40).step_by(4) {
            few[i] = 200; // 10 изменённых точек: мигающий курсор
        }
        assert!(!thumb_moved(&a, &few, 6, 20));
        assert!(thumb_moved(&a, &[60u8; 400], 6, 20));
        assert!(thumb_moved(&a, b"x", 6, 20));
        assert!(thumb_moved(&[], &a, 6, 20));
    }

    #[test]
    fn test_copy_allowed() {
        assert!(!copy_allowed("Chrome_WidgetWin_1", "claude.exe", ""));
        assert!(!copy_allowed("MozillaWindowClass", "firefox.exe", ""));
        assert!(!copy_allowed("CASCADIA_HOSTING_WINDOW_CLASS", "windowsterminal.exe", ""));
        assert!(!copy_allowed("Qt5QWindowIcon", "anydesk.exe", ""));
        assert!(!copy_allowed("ConsoleWindowClass", "something.exe", ""));
        assert!(!copy_allowed("SunAwtFrame", "phpstorm64.exe", ""));
        assert!(!copy_allowed("Qt5152QWindowIcon", "editors.exe", "Бюджет.xlsx - ONLYOFFICE"));
        assert!(copy_allowed("Qt5152QWindowIcon", "editors.exe", "Инвойс.docx - ONLYOFFICE"));
    }

    #[test]
    fn test_has_cyrillic_and_same_title() {
        assert!(has_cyrillic("abc ж"));
        assert!(!has_cyrillic("abc"));
        assert!(same_title("Инвойс.docx", "*Инвойс.docx"));
        assert!(!same_title("", ""));
        assert!(!same_title("Инвойс.docx", "Резюме.pdf"));
        assert!(!same_title("Договор 12.docx - ONLYOFFICE", "Договор 13.docx - ONLYOFFICE"));
        assert!(!same_title("a.txt - Блокнот", "b.txt - Блокнот"));
    }
}

mod v2_tests {
    use super::*;

    #[test]
    fn test_page_key() {
        assert_eq!(page_key("https://claude.ai/chat/1#x", "Чат"), "https://claude.ai/chat/1");
        assert_eq!(page_key("app://-/index.html", "*Мой чат"), "мой чат");
        assert_eq!(page_key("", ""), "");
    }

    #[test]
    fn test_shift_and_size() {
        assert_eq!(shift(r(1, 2, 3, 4), 10, -5), r(11, -3, 13, -1));
        assert_eq!(size_of(r(10, 20, 15, 50)), (5, 30));
    }

    #[test]
    fn test_contrast() {
        assert!((contrast_ratio(rgb("#000000"), rgb("#FFFFFF")) - 21.0).abs() < 0.05);
        assert_eq!(label_color(rgb("#FFD400")), rgb("#000000"));
        assert_eq!(label_color(rgb("#1F3A93")), rgb("#FFFFFF"));
    }

    #[test]
    fn test_icon_color() {
        let pixels = icon_pixels(32, rgb("#112233"));
        let i = (5 * 32 + 16) * 4;
        assert_eq!(&pixels[i..i + 4], &[0x33, 0x22, 0x11, 0xFF]);
    }
}

mod misc_tests {
    use super::*;

    #[test]
    fn test_icon_pixels() {
        let pixels = icon_pixels(32, rgb("#FF7A00"));
        assert_eq!(pixels.len(), 32 * 32 * 4);
        assert_eq!(pixels[(5 * 32 + 16) * 4 + 3], 0xFF); // лента
        assert_eq!(pixels[(30 * 32 + 16) * 4 + 3], 0); // вырез
        assert_eq!(pixels[(5 * 32 + 2) * 4 + 3], 0); // снаружи
    }

    #[test]
    fn test_help_text_formats() {
        let text = help_text("Ctrl+Alt+M", "Ctrl+Alt+Shift+M", 30);
        assert!(text.contains("Ctrl+Alt+M"));
        assert!(text.contains("30"));
    }
}

// ---------------------------------------------------------------- test_panel.py

mod color_math_tests {
    use super::*;

    const AUTO_EXTRA: [&str; 3] = ["#777777", "#0067C0", "#E81123"];

    #[test]
    fn test_contrast_extremes_and_symmetry() {
        let close = |a: &str, b: &str, want: f64, places: i32| {
            let got = contrast_ratio(rgb(a), rgb(b));
            assert!((got - want).abs() < 0.5 * 10f64.powi(-places), "{a} {b}: {got}");
        };
        close("#000000", "#FFFFFF", 21.0, 6);
        close("#FFFFFF", "#000000", 21.0, 6);
        close("#FFD400", "#ffd400", 1.0, 6);
        close("#777777", "#FFFFFF", 4.48, 2);
        close("#FF7A00", "#000000", 8.0, 1);
    }

    #[test]
    fn test_label_color() {
        assert_eq!(label_color(rgb("#FFD400")), rgb("#000000"));
        assert_eq!(label_color(rgb("#FF7A00")), rgb("#000000"));
        assert_eq!(label_color(rgb("#FFFFFF")), rgb("#000000"));
        assert_eq!(label_color(rgb("#1E1E1E")), rgb("#FFFFFF"));
        assert_eq!(label_color(rgb("#9B6CFF")), rgb("#000000"));
        assert_eq!(label_color(rgb("#5A2DA0")), rgb("#FFFFFF"));
    }

    #[test]
    fn test_auto_label_always_readable() {
        for color in PRESETS.iter().map(|(c, _)| *c).chain(AUTO_EXTRA.iter().map(|s| rgb(s))) {
            assert!(contrast_ratio(color, label_color(color)) >= 4.5, "{}", color.hex());
        }
    }

    #[test]
    fn test_mix() {
        assert_eq!(mix(rgb("#000000"), rgb("#FFFFFF"), 0.5), rgb("#808080"));
        assert_eq!(mix(rgb("#FFD400"), rgb("#000000"), 0.0), rgb("#FFD400"));
        assert_eq!(mix(rgb("#ffd400"), rgb("#000000"), 1.0), rgb("#000000"));
        assert_eq!(mix(rgb("#FF0000"), rgb("#0000FF"), 2.0), rgb("#0000FF")); // доля ограничена
        assert_eq!(mix(rgb("#204060"), rgb("#FFFFFF"), 0.25), rgb("#587088"));
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize_color("ffd400"), Ok(rgb("#FFD400")));
        assert_eq!(normalize_color(" #fd0 "), Ok(rgb("#FFDD00")));
        for bad in ["", "#12345", "#GGGGGG", "auto"] {
            assert_eq!(normalize_color(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn test_presets() {
        assert_eq!(PRESETS.len(), 8);
        assert_eq!(PRESETS[0], (rgb("#FFD400"), "Жёлтый"));
        let colors: Vec<Rgb> = PRESETS.iter().map(|(c, _)| *c).collect();
        assert!(colors.contains(&rgb("#FF7A00")));
        assert!(colors.contains(&rgb("#FFFFFF")));
        let unique: std::collections::HashSet<Rgb> = colors.iter().copied().collect();
        assert_eq!(unique.len(), 8);
        for (color, name) in PRESETS {
            assert_eq!(normalize_color(&color.hex()), Ok(color));
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn test_clean_colors() {
        let defaults = Colors { color: rgb("#FFD400"), opacity: 0.3, bar_color: rgb("#FF7A00"), label_color: LabelColor::Auto };
        assert_eq!(clean_colors(&defaults), defaults);
        // Цвета в Rust уже разобраны в Rgb (битые строки отсеивает normalize_color); здесь — плотность.
        let cleaned = clean_colors(&Colors { opacity: 90.0, bar_color: rgb("#00ff00"), label_color: LabelColor::Custom(rgb("#FFFFFF")), ..defaults });
        assert_eq!(cleaned, Colors { color: rgb("#FFD400"), opacity: 0.6, bar_color: rgb("#00FF00"), label_color: LabelColor::Custom(rgb("#FFFFFF")) });
        assert_eq!(normalize_color("#fff"), Ok(rgb("#FFFFFF")));
        assert_eq!(normalize_color("nope"), Err(()));
        assert_eq!(clean_colors(&Colors { opacity: 0.05, ..defaults }).opacity, 0.2);
    }
}

mod text_tests {
    use super::*;

    #[test]
    fn test_program_names() {
        let cases = [
            ("claude.exe", "Claude"), ("ChatGPT.exe", "ChatGPT"), ("antigravity.exe", "Antigravity"),
            ("brave.exe", "Brave"), ("chrome.exe", "Chrome"), ("msedge.exe", "Edge"),
            ("editors.exe", "ONLYOFFICE"), ("WINWORD.EXE", "Word"), ("notepad.exe", "Блокнот"),
            (r"C:\Apps\telegram.exe", "Telegram"), ("codex", "Codex"), ("", ""),
        ];
        for (exe, name) in cases {
            assert_eq!(program_name(exe), name, "{exe}");
        }
    }

    #[test]
    fn test_one_line() {
        assert_eq!(one_line("  a\n\tb  c "), "a b c");
        assert_eq!(one_line(""), "");
    }
}

mod fit_text_tests {
    use super::*;

    /// Подставной шрифт: кириллица 8 px, пробел 4 px, «…» 12 px, остальное 7 px.
    fn measure(s: &str) -> i32 {
        s.chars()
            .map(|c| match c {
                ' ' => 4,
                '…' => 12,
                c if ('\u{400}'..='\u{4FF}').contains(&c) => 8,
                _ => 7,
            })
            .sum()
    }

    #[test]
    fn test_fit_text() {
        assert_eq!(fit_text(&measure, "коротко", 500), "коротко");
        let cut = fit_text(&measure, &"очень длинный текст ".repeat(10), 120);
        assert!(cut.ends_with('…'));
        assert!(measure(&cut) <= 120);
        assert_eq!(fit_text(&measure, "текст", 0), "");
    }
}

// ---------------------------------------------------------------- прочее

#[test]
fn status_tags_and_constants() {
    assert_eq!(status_tag(Status::Closed), "закрыто");
    assert_eq!(status_tag(Status::NotFound), "текст не найден");
    assert_eq!(status_tag(Status::Shown), "");
    assert_eq!(APP_TITLE, "Закладка");
    assert_eq!((SHORT_QUOTE, CONTEXT, MAX_QUOTE), (25, 64, 600));
}

/// Порог яркости 0,03928 (text_bookmark.pyw) и 0,04045 (panel.py) для 8-битных цветов одинаковы:
/// одинаковы линейные значения всех 256 каналов, а значит и контраст, и выбор чёрного/белого.
#[test]
fn luminance_thresholds_agree_for_all_channels() {
    for value in 0..=255u8 {
        let v = value as f64 / 255.0;
        let old = if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let grey = Rgb(value, value, value);
        let expected = (0.2126 + 0.7152 + 0.0722) * old;
        assert!((relative_luminance(grey) - expected).abs() < 1e-12, "канал {value}");
    }
    for (color, _) in PRESETS {
        assert_eq!(label_color(color), if contrast_ratio(color, Rgb(0, 0, 0)) >= contrast_ratio(color, Rgb(255, 255, 255)) { Rgb(0, 0, 0) } else { Rgb(255, 255, 255) });
    }
}

#[test]
fn hotkey_names_and_combo() {
    assert_eq!(combo_text(true, true, false, false, 'Z' as u32), Some("Ctrl+Alt+Z".into()));
    assert_eq!(combo_text(true, true, true, true, 0x70), Some("Ctrl+Alt+Shift+Win+F1".into()));
    assert_eq!(combo_text(false, false, false, false, 'Z' as u32), None);
    assert_eq!(combo_text(true, false, false, false, 0x10), None);
    for text in ["Ctrl+Alt+Z", "Ctrl+Alt+Shift+X", "Win+Alt+PageDown"] {
        let (mods, vk) = parse_hotkey(text).expect("разбор");
        let shown = combo_text(mods & 2 != 0, mods & 1 != 0, mods & 4 != 0, mods & 8 != 0, vk).expect("имя");
        assert_eq!(parse_hotkey(&shown), Ok((mods, vk)), "{text} -> {shown}");
    }
}

// ---------------------------------------------------------------- эталоны Python

#[test]
fn golden_flatten() {
    let mut check = Check::new("flatten");
    for case in arr(&golden("flatten.json")) {
        let c = arr(case);
        let (got, index) = flatten(text(&c[0]));
        let want_index: Vec<u32> = arr(&c[2]).iter().map(|v| int(v) as u32).collect();
        check.eq(text(&c[0]), (got.iter().collect::<String>(), index), (text(&c[1]).to_string(), want_index));
    }
    assert!(check.finish() >= 200);
}

/// Все кодовые точки: flatten, casefold (через title_key), str.isspace (через one_line),
/// unicodedata.combining (через grapheme_extra) — как в Python 3.13 (Unicode 15.1).
#[test]
fn flatten_table_matches_python() {
    let table = golden("flatten_table.json");
    let lookup = |key: &str, code: u32| table[key].get(format!("{code:x}")).map(|v| text(v).to_string());
    let spaces: std::collections::HashSet<u32> = arr(&table["isspace"]).iter().map(|v| int(v) as u32).collect();
    let combining: Vec<(u32, u32)> = arr(&table["combining"]).iter().map(|v| (int(&v[0]) as u32, int(&v[1]) as u32)).collect();
    let is_combining = |code: u32| combining.iter().any(|&(lo, hi)| lo <= code && code <= hi);
    let mut bad: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for code in 0..=0x10FFFFu32 {
        let Some(c) = char::from_u32(code) else { continue };
        checked += 1;
        let own = c.to_string();
        let got = flat_str(&own);
        let want = lookup("flatten", code).unwrap_or_else(|| own.clone());
        if got != want {
            bad.push(format!("{code:04X} flatten {got:?} != {want:?}"));
        }
        let want_key = if spaces.contains(&code) || matches!(c, '*' | '●' | '•') {
            String::new()
        } else {
            lookup("casefold", code).unwrap_or_else(|| own.clone())
        };
        let got_key = title_key(&own);
        if got_key != want_key {
            bad.push(format!("{code:04X} casefold {got_key:?} != {want_key:?}"));
        }
        let spaced = format!("a{c}b");
        let want_line = if spaces.contains(&code) { "a b".to_string() } else { spaced.clone() };
        if one_line(&spaced) != want_line {
            bad.push(format!("{code:04X} isspace"));
        }
        let want_extra = if c == '\u{200D}' {
            2
        } else if is_combining(code) || c == '\u{FE0E}' || c == '\u{FE0F}' || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c) {
            1
        } else {
            0
        };
        if grapheme_extra(&[c]) != want_extra {
            bad.push(format!("{code:04X} combining {} != {want_extra}", grapheme_extra(&[c])));
        }
    }
    // После обновления crates: ZAKLADKA_DUMP_FLATTEN=<файл> — полный список расхождений
    // (кодовые точки для NEWER_THAN_PYTHON в src\core\flatten.rs).
    if let Some(dump) = std::env::var_os("ZAKLADKA_DUMP_FLATTEN") {
        let _ = std::fs::write(dump, bad.join("\n"));
    }
    assert!(bad.is_empty(), "{} расхождений из {checked}:\n{}", bad.len(), bad.iter().take(60).cloned().collect::<Vec<_>>().join("\n"));
    eprintln!("flatten_table: {checked} кодовых точек совпали (Unicode {})", text(&table["unicode"]));
}

#[test]
fn golden_similarity() {
    let mut check = Check::new("similarity");
    for case in arr(&golden("similarity.json")) {
        let c = arr(case);
        let (a, b) = (chars(text(&c[0])), chars(text(&c[1])));
        check.close((text(&c[0]), text(&c[1])), similarity(&a, &b), num(&c[2]));
    }
    assert!(check.finish() >= 200);
}

#[test]
fn golden_matching() {
    let g = golden("matching.json");
    let mut check = Check::new("best_occurrence");
    for case in arr(&g["best_occurrence"]) {
        let c = arr(case);
        let got = best_occurrence(&chars(text(&c[0])), &chars(text(&c[1])), &chars(text(&c[2])), &chars(text(&c[3])), num(&c[4]));
        check.eq(case, got, opt_idx(&c[5]));
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("fuzzy_best");
    for case in arr(&g["fuzzy_best"]) {
        let c = arr(case);
        let got = fuzzy_best(&chars(text(&c[0])), &chars(text(&c[1])), &chars(text(&c[2])), &chars(text(&c[3])), num(&c[4]));
        check.eq(case, got, opt_idx(&c[5]));
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("fuzzy_candidates");
    for case in arr(&g["fuzzy_candidates"]) {
        let c = arr(case);
        let got = fuzzy_candidates(&chars(text(&c[0])), &chars(text(&c[1])), num(&c[2]), idx(&c[3]), idx(&c[4]));
        let want: Vec<(usize, f64)> = arr(&c[5]).iter().map(|x| (idx(&x[0]), num(&x[1]))).collect();
        let same = got.len() == want.len() && got.iter().zip(&want).all(|(g, w)| g.0 == w.0 && (g.1 - w.1).abs() <= 1e-9);
        check.eq(case, same.then_some(()).ok_or(format!("{got:?}")), Ok(()));
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("find_quote");
    for case in arr(&g["find_quote"]) {
        let c = arr(case);
        let got = find_quote(&chars(text(&c[0])), &chars(text(&c[1])), idx(&c[2])).map(|v| v.into_iter().collect::<String>());
        check.eq(case, got, c[3].as_str().map(str::to_string));
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("occurrences");
    for case in arr(&g["occurrences"]) {
        let c = arr(case);
        let got = occurrences(&chars(text(&c[0])), &chars(text(&c[1])), idx(&c[2]));
        check.eq(case, got, arr(&c[3]).iter().map(idx).collect::<Vec<_>>());
    }
    check.finish();

    let mut check = Check::new("context_score");
    for case in arr(&g["context_score"]) {
        let c = arr(case);
        let got = context_score(&chars(text(&c[0])), idx(&c[1]), idx(&c[2]), &chars(text(&c[3])), &chars(text(&c[4])));
        check.close(case, got, num(&c[5]));
    }
    check.finish();
}

#[test]
fn golden_geometry() {
    let g = golden("geometry.json");
    let mut check = Check::new("merge_lines");
    for case in arr(&g["merge_lines"]) {
        let c = arr(case);
        check.eq(case, merge_lines(&rects_of(&c[0]), int(&c[1]) as i32), rects_of(&c[2]));
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("to_rects");
    for case in arr(&g["to_rects"]) {
        let c = arr(case);
        let values: Vec<f64> = arr(&c[0]).iter().map(num).collect();
        check.eq(case, to_rects(&values), rects_of(&c[1]));
    }
    check.finish();

    let names_of = |v: &Value| -> Vec<String> { arr(v).iter().map(|s| text(s).to_string()).collect() };
    let mut check = Check::new("spans");
    for case in arr(&g["spans"]) {
        let c = arr(case);
        let doc = FlatDoc::new(&names_of(&c[0]));
        let want: Vec<(usize, usize, usize)> = arr(&c[3]).iter().map(|s| (idx(&s[0]), idx(&s[1]), idx(&s[2]))).collect();
        check.eq(case, doc.spans(int(&c[1]) as isize, int(&c[2]) as isize), want);
    }
    check.finish();

    let mut check = Check::new("cut_to_segments");
    for case in arr(&g["cut_to_segments"]) {
        let c = arr(case);
        let doc = FlatDoc::new(&names_of(&c[0]));
        check.eq(case, cut_to_segments(&doc, idx(&c[1]), idx(&c[2]), idx(&c[3])), idx(&c[4]));
    }
    check.finish();

    let mut check = Check::new("choose_capture");
    for case in arr(&g["choose_capture"]) {
        let c = arr(case);
        let doc = FlatDoc::new(&names_of(&c[0]));
        let rects: Vec<Rect> = arr(&c[2]).iter().map(|v| if v.is_null() { Rect::default() } else { rect_of(v) }).collect();
        let cursor = p(int(&c[4][0]) as i32, int(&c[4][1]) as i32);
        check.eq(case, choose_capture(&doc, &chars(text(&c[1])), &rects, &rects_of(&c[3]), cursor), opt_idx(&c[5]));
    }
    check.finish();
}

#[test]
fn golden_color() {
    let g = golden("color.json");
    let mut check = Check::new("contrast_ratio");
    for case in arr(&g["contrast"]) {
        let c = arr(case);
        let got = contrast_ratio(rgb(text(&c[0])), rgb(text(&c[1])));
        check.close(case, got, num(&c[2])); // panel.py
        check.close(case, got, num(&c[3])); // text_bookmark.pyw (порог 0,03928)
    }
    assert!(check.finish() >= 100);

    let mut check = Check::new("label_color");
    for case in arr(&g["label"]) {
        let c = arr(case);
        let got = label_color(rgb(text(&c[0])));
        check.eq(case, got, rgb(text(&c[1])));
        check.eq(case, got, rgb(text(&c[2])));
    }
    check.finish();

    let mut check = Check::new("mix");
    for case in arr(&g["mix"]) {
        let c = arr(case);
        check.eq(case, mix(rgb(text(&c[0])), rgb(text(&c[1])), num(&c[2])), rgb(text(&c[3])));
    }
    check.finish();

    let mut check = Check::new("normalize_color");
    for case in arr(&g["normalize"]) {
        let c = arr(case);
        check.eq(case, normalize_color(text(&c[0])).ok(), c[1].as_str().map(rgb));
    }
    check.finish();

    let mut check = Check::new("clean_colors.opacity");
    let base = Colors { color: rgb("#FFD400"), opacity: 0.3, bar_color: rgb("#FF7A00"), label_color: LabelColor::Auto };
    for case in arr(&g["opacity"]) {
        let c = arr(case);
        check.eq(case, clean_colors(&Colors { opacity: num(&c[0]), ..base }).opacity, num(&c[1]));
    }
    check.finish();
}

#[test]
fn golden_misc_core() {
    let g = golden("misc.json");
    let error_of = |message: &str| match message {
        "empty part" => HotkeyError::EmptyPart,
        "two keys" => HotkeyError::TwoKeys,
        "modifier and key required" => HotkeyError::ModifierAndKeyRequired,
        "unknown key" => HotkeyError::UnknownKey,
        "needs two modifiers including Alt or Win" => HotkeyError::NotSafe,
        other => panic!("неизвестная ошибка {other}"),
    };
    let result_of = |v: &Value| -> Result<(u32, u32), HotkeyError> {
        match v.get("ok") {
            Some(ok) => Ok((int(&ok[0]) as u32, int(&ok[1]) as u32)),
            None => Err(error_of(text(&v["error"]))),
        }
    };
    let mut check = Check::new("hotkey");
    for case in arr(&g["hotkey"]) {
        let c = arr(case);
        check.eq(case, parse_hotkey(text(&c[0])), result_of(&c[1]));
        check.eq(case, safe_hotkey(text(&c[0])), result_of(&c[2]));
    }
    check.finish();

    let mut check = Check::new("text");
    for c in arr(&g["program_name"]).iter().map(arr) {
        check.eq(&c, program_name(text(&c[0])), text(&c[1]).to_string());
    }
    for c in arr(&g["one_line"]).iter().map(arr) {
        check.eq(&c, one_line(text(&c[0])), text(&c[1]).to_string());
    }
    for c in arr(&g["menu_label"]).iter().map(arr) {
        check.eq(&c, menu_label(text(&c[0]), idx(&c[1])), text(&c[2]).to_string());
    }
    for c in arr(&g["title_key"]).iter().map(arr) {
        check.eq(&c, title_key(text(&c[0])), text(&c[1]).to_string());
    }
    for c in arr(&g["same_title"]).iter().map(arr) {
        check.eq(&c, same_title(text(&c[0]), text(&c[1])), c[2].as_bool() == Some(true));
    }
    for c in arr(&g["page_key"]).iter().map(arr) {
        check.eq(&c, page_key(text(&c[0]), text(&c[1])), text(&c[2]).to_string());
    }
    for c in arr(&g["copy_allowed"]).iter().map(arr) {
        check.eq(&c, copy_allowed(text(&c[0]), text(&c[1]), text(&c[2])), c[3].as_bool() == Some(true));
    }
    for c in arr(&g["grapheme_extra"]).iter().map(arr) {
        check.eq(&c, grapheme_extra(&chars(text(&c[0]))), idx(&c[1]));
    }
    for c in arr(&g["has_cyrillic"]).iter().map(arr) {
        check.eq(&c, has_cyrillic(text(&c[0])), c[1].as_bool() == Some(true));
    }
    for c in arr(&g["help_text"]).iter().map(arr) {
        check.eq(&c[..3], help_text(text(&c[0]), text(&c[1]), int(&c[2]) as u32), text(&c[3]).to_string());
    }
    for c in arr(&g["vk_name"]).iter().map(arr) {
        check.eq(&c, vk_name(int(&c[0]) as u32), c[1].as_str().map(str::to_string));
    }
    check.finish();
}
