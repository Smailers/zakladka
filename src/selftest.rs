//! `--selftest` (часть 5; P 3148–3227). JSON — в файл `--out <путь>` (у оконной программы нет консоли).
//!
//! `main` вызывает `run` до мьютекса и до DPI, передавая всё, что после "--selftest"
//! (например `find "<фраза>" claude.exe --out <файл>`). Как в Python, `run` сам включает DPI
//! (`Api::enable_dpi()`) и COM (`CoInitializeEx(COINIT_MULTITHREADED)` в своём потоке).
//!
//! Режимы (как в Python): `find "<фраза>" [exe]`, `auto [exe]`, `selection [exe]`; exe по умолчанию
//! `claude.exe`. Ничего не сохраняется и ничего не нажимается: только чтение UI Automation
//! (`ElementFromPoint`, дерево, прямоугольники) — без Invoke/Select/ScrollIntoView/фокуса/клавиш.
//! Самопроверка не зависит от цикла tracker: только `Uia`, `Api` и `core`.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Map, Value};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::core::flatten::{flatten, FlatDoc};
use crate::core::geometry::{center, direction, grow, intersect, merge_lines};
use crate::core::matching::{best_occurrence, occurrences};
use crate::core::text::one_line;
use crate::types::{Dir, Point, Rect};
use crate::uia::{RuntimeId, Uia};
use crate::win::{monotonic, Api};

/// Python: `time.sleep(1.2)` (выделение) и `time.sleep(1.5)` (Chromium ещё строит дерево).
const SELECTION_RETRY: Duration = Duration::from_millis(1200);
const WARMUP: Duration = Duration::from_millis(1500);
/// Значения Python по умолчанию: `segments(limit=30000)`, `occurrences(limit=50)`,
/// `best_occurrence(min_score=0.6)`, `merge_lines(gap=14)`, `hit_chain(depth=3, settle=0.05)`.
const SEGMENT_LIMIT: usize = 30_000;
const OCCURRENCES_LIMIT: usize = 50;
const MIN_SCORE: f64 = 0.6;
const LINE_GAP: i32 = 14;
const HIT_DEPTH: usize = 3;
const HIT_SETTLE_S: f64 = 0.05;

/// Разобранные аргументы (всё, что после `--selftest`).
#[derive(Debug, PartialEq)]
struct Options {
    mode: String,
    phrase: String,
    exe: String,
    out: Option<PathBuf>,
}

/// Как P 3153–3156; `--out <путь>` и `--dev` из позиционных аргументов убираются.
fn parse(args: &[String]) -> Options {
    let mut rest: Vec<&str> = Vec::new();
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = args.get(i + 1).map(PathBuf::from);
                i += 2;
                continue;
            }
            "--dev" => {}
            other => rest.push(other),
        }
        i += 1;
    }
    let mode = rest.first().copied().unwrap_or("find").to_string();
    let find = mode == "find";
    let phrase = if find { rest.get(1).copied().unwrap_or("") } else { "" }.to_string();
    let exe = if find { rest.get(2) } else { rest.get(1) }.copied().unwrap_or("claude.exe").to_lowercase();
    Options { mode, phrase, exe, out }
}

fn rect_json(r: Rect) -> Value {
    json!([r.l, r.t, r.r, r.b])
}

fn rects_json(rects: &[Rect]) -> Value {
    Value::Array(rects.iter().map(|r| rect_json(*r)).collect())
}

fn ms_since(started: f64) -> i64 {
    ((monotonic() - started) * 1000.0).round_ties_even() as i64 // round() Python
}

/// Python `a // b` для `b > 0`.
fn floor_div(a: i64, b: i64) -> i32 {
    a.div_euclid(b).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Код выхода процесса.
pub fn run(args: &[String]) -> i32 {
    let options = parse(args);
    Api::enable_dpi();
    // SAFETY: обычный вызов COM в начале потока; парный CoUninitialize — ниже.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    // Все объекты COM живут только внутри `check` и отпускаются до CoUninitialize.
    let (report, code) = check(&options);
    if com {
        // SAFETY: парный вызов к успешному CoInitializeEx этого потока.
        unsafe { CoUninitialize() };
    }
    match emit(&options, &report) {
        true => code,
        false => 1,
    }
}

/// Записать JSON одной строкой (как `print(json.dumps(…, ensure_ascii=False))`).
fn emit(options: &Options, report: &Value) -> bool {
    let line = format!("{report}\n");
    match &options.out {
        Some(path) => std::fs::write(path, line.as_bytes()).is_ok(),
        None => {
            // Без консоли (оконная программа) запись молча теряется — для этого и есть `--out`.
            let _ = std::io::stdout().write_all(line.as_bytes());
            true
        }
    }
}

/// Отчёт и код выхода (P 3157–3227).
fn check(options: &Options) -> (Value, i32) {
    let api = Api::new();
    let exe = options.exe.as_str();
    let Some(hwnd) = api.find_window(exe) else {
        return (json!({ "error": format!("no window of {exe}") }), 2);
    };
    let Ok(uia) = Uia::new() else {
        return (json!({ "error": "UI Automation is not available" }), 1);
    };
    let pid = api.process(hwnd).0;
    let window = api.window_rect(hwnd);
    let mut report = Map::new();
    report.insert("exe".into(), json!(exe));
    report.insert("window".into(), window.map_or(Value::Null, rect_json));
    report.insert("foreground".into(), json!(api.foreground() == hwnd));

    if options.mode == "selection" {
        let mut selection = uia.selection(pid, api.cursor());
        if selection.text.is_empty() {
            std::thread::sleep(SELECTION_RETRY);
            selection = uia.selection(pid, api.cursor());
        }
        report.insert("text".into(), json!(selection.text.chars().take(120).collect::<String>()));
        report.insert("rects".into(), rects_json(&selection.rects));
        report.insert("any_pattern".into(), json!(selection.any_pattern));
        return (Value::Object(report), 0);
    }

    let started = monotonic();
    let mut scopes = uia.shown_scopes(hwnd, pid, window);
    if scopes.first().map_or(true, |(_, scope)| uia.document_of(scope).is_none()) {
        std::thread::sleep(WARMUP); // Chromium ещё строит дерево доступности
        scopes = uia.shown_scopes(hwnd, pid, window);
    }
    report.insert("scopes_ms".into(), json!(ms_since(started)));
    report.insert("scopes".into(), json!(scopes.len()));

    let mut phrase = options.phrase.clone();
    if options.mode == "auto" {
        if let (Some((_, scope)), Some(w)) = (scopes.first(), window) {
            let at = Point {
                x: floor_div(i64::from(w.l) + i64::from(w.r), 2),
                y: w.t + floor_div((i64::from(w.b) - i64::from(w.t)) * 2, 5),
            };
            phrase = uia
                .text_pattern(scope)
                .and_then(|pattern| uia.range_text_at(&pattern, at, 200))
                .map(|line| one_line(&line).chars().skip(5).take(40).collect())
                .unwrap_or_default();
            report.insert("phrase".into(), json!(phrase));
        }
    }

    let (quote, _) = flatten(&phrase);
    let mut results = Vec::with_capacity(scopes.len());
    for (_, scope) in &scopes {
        let started = monotonic();
        let mut segs = uia.segments(scope, SEGMENT_LIMIT).unwrap_or_default();
        if segs.len() < 5 {
            std::thread::sleep(WARMUP);
            segs = uia.segments(scope, SEGMENT_LIMIT).unwrap_or_default();
        }
        let flat = FlatDoc::new(&segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
        let mut entry = Map::new();
        entry.insert("segments".into(), json!(segs.len()));
        entry.insert("flatten_ms".into(), json!(ms_since(started)));
        entry.insert("occurrences".into(), json!(occurrences(&flat.text, &quote, OCCURRENCES_LIMIT).len()));
        if let Some(start) = best_occurrence(&flat.text, &quote, &[], &[], MIN_SCORE) {
            let pattern = uia.text_pattern(scope);
            let mut lines = Vec::new();
            let mut allowed: HashSet<RuntimeId> = HashSet::new();
            for (segment, first, last) in flat.spans(start as isize, (start + quote.len()) as isize) {
                let Some(seg) = segs.get(segment) else { continue };
                allowed.extend(uia.rid(&seg.element));
                allowed.extend(uia.parent_rid(&seg.element));
                let mut pieces = uia.piece_rects(pattern.as_ref(), &seg.element, &seg.name, first, last);
                if pieces.is_empty() {
                    pieces.push(seg.rect);
                }
                lines.extend(pieces.into_iter().filter_map(|piece| intersect(Some(piece), Some(grow(seg.rect, 2)))));
            }
            let lines = merge_lines(&lines, LINE_GAP);
            let scope_rect = uia.current_rect(scope).or(window);
            let viewport = intersect(scope_rect, window);
            let inside: Vec<Rect> = lines.iter().filter_map(|line| intersect(Some(*line), viewport)).collect();
            let hit_ok: Vec<bool> = inside
                .iter()
                .take(6)
                .map(|r| uia.hit_chain(center(*r), HIT_DEPTH, HIT_SETTLE_S).iter().any(|key| allowed.contains(key)))
                .collect();
            entry.insert("lines".into(), rects_json(&lines));
            entry.insert("inside".into(), rects_json(&inside));
            entry.insert(
                "direction".into(),
                match direction(&lines, viewport) {
                    Some(Dir::Up) => json!("up"),
                    Some(Dir::Down) => json!("down"),
                    None => Value::Null,
                },
            );
            entry.insert("hit_ok".into(), json!(hit_ok));
        }
        results.push(Value::Object(entry));
    }
    report.insert("results".into(), Value::Array(results));
    (Value::Object(report), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_like_python() {
        let o = parse(&strings(&["find", "две фразы", "Claude.EXE", "--out", "C:\\t\\r.json"]));
        assert_eq!(o.mode, "find");
        assert_eq!(o.phrase, "две фразы");
        assert_eq!(o.exe, "claude.exe");
        assert_eq!(o.out, Some(PathBuf::from("C:\\t\\r.json")));

        let o = parse(&strings(&["--out", "x.json", "find", "p"]));
        assert_eq!((o.phrase.as_str(), o.exe.as_str()), ("p", "claude.exe"));

        let o = parse(&strings(&["selection", "Brave.exe", "--dev"]));
        assert_eq!((o.mode.as_str(), o.phrase.as_str(), o.exe.as_str(), o.out), ("selection", "", "brave.exe", None));

        let o = parse(&strings(&["auto"]));
        assert_eq!((o.mode.as_str(), o.exe.as_str()), ("auto", "claude.exe"));

        let o = parse(&[]);
        assert_eq!((o.mode.as_str(), o.phrase.as_str(), o.exe.as_str()), ("find", "", "claude.exe"));

        assert_eq!(parse(&strings(&["find", "p", "a.exe", "--out"])).out, None);
    }

    #[test]
    fn floor_division_and_json() {
        assert_eq!(floor_div(-1921, 2), -961);
        assert_eq!(rect_json(Rect { l: -1, t: 2, r: 3, b: 4 }).to_string(), "[-1,2,3,4]");
    }
}
