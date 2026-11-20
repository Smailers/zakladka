//! Часть 1, `store`: перенесённые тесты `test_text_bookmark.py` (13 из 51, остальные — в core.rs),
//! эталоны Python (`tests\golden\*.json`) и совместимость bookmarks.json в обе стороны.
//! Все файлы — только во временных папках; настоящий %LOCALAPPDATA%\text-bookmark не трогается.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde_json::{json, Value};

use zakladka::app_id::{DEV, PROD};
use zakladka::store::*;
use zakladka::types::{Bookmark, Config, Dir, Kind, LabelColor, Mode, Rect, RenderState, Rgb, Shared};

// ---------------------------------------------------------------- помощники

/// Своя временная папка, удаляется в конце теста.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "zakladka-part1-{tag}-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("временная папка");
        TempDir(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name)
}

fn golden(name: &str) -> Value {
    let path = golden_path(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("ожидался массив: {v}"))
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("ожидалась строка: {v}"))
}

fn at(y: i32, m: u32, d: u32) -> LocalTime {
    LocalTime { y, m, d, h: 0, mi: 0, s: 0, us: 0 }
}

fn now() -> LocalTime {
    at(2026, 11, 20)
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

fn bm(id: &str) -> Bookmark {
    Bookmark { id: id.into(), ..bookmark() }
}

/// Закладка в виде JSON-объекта Python (`bookmark(**fields)` из test_text_bookmark.py).
fn bookmark_json(id: &str, extra: &[(&str, Value)]) -> Value {
    let mut v = json!({"id": id, "exe": "claude.exe", "kind": "pos", "quote": "цитата", "prefix": "",
                       "suffix": "", "label": "Цитата", "title": "", "created": "2026-11-19T10:00:00"});
    for (key, value) in extra {
        v[*key] = value.clone();
    }
    v
}

/// Закладка из словаря, который вернул `load_bookmarks` Python (ровно 13 ключей).
fn from_python(v: &Value) -> Bookmark {
    let map = v.as_object().expect("словарь");
    assert_eq!(map.len(), 13, "лишние или недостающие ключи: {v}");
    let f = |k: &str| text(&v[k]).to_string();
    Bookmark {
        id: f("id"),
        exe: f("exe"),
        kind: Kind::parse(&f("kind")).expect("kind"),
        mode: Mode::parse(&f("mode")).expect("mode"),
        quote: f("quote"),
        prefix: f("prefix"),
        suffix: f("suffix"),
        label: f("label"),
        title: f("title"),
        created: f("created"),
        page: f("page"),
        doc: f("doc"),
        url: f("url"),
    }
}

fn ids(items: &[Bookmark]) -> Vec<String> {
    items.iter().map(|b| b.id.clone()).collect()
}

fn shared(dir: &Path, debug: bool) -> Shared {
    Shared {
        config: RwLock::new(default_config(&PROD)),
        profile: &PROD,
        data_dir: dir.to_path_buf(),
        debug: AtomicBool::new(debug),
    }
}

/// Интерпретатор старой версии (для `gen_golden.py --check`), если он есть на этом ПК.
fn old_python() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut candidates: Vec<PathBuf> = std::env::var_os("ZAKLADKA_OLD_PYTHON").map(PathBuf::from).into_iter().collect();
    for up in [manifest.parent(), manifest.parent().and_then(Path::parent)].into_iter().flatten() {
        candidates.push(up.join("text-bookmark").join(".venv").join("Scripts").join("python.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------------- test_text_bookmark.py

mod config_tests {
    use super::*;

    #[test]
    fn test_parse_config_validates() {
        let config = parse_config(
            &json!({"hotkey_mark": "Ctrl+Alt+Q", "hotkey_extra": "Ctrl+C", "opacity": 2, "color": "#00ff00",
                    "bar_color": "red", "max_age_days": 7, "max_bookmarks": true, "debug": true, "unknown": 1}),
            &PROD,
        );
        let defaults = default_config(&PROD);
        assert_eq!(config.hotkey_mark, "Ctrl+Alt+Q");
        assert_eq!(config.hotkey_extra, defaults.hotkey_extra);
        assert_eq!(config.opacity, defaults.opacity);
        assert_eq!(config.color, Rgb(0x00, 0xFF, 0x00));
        assert_eq!(config.bar_color, defaults.bar_color);
        assert_eq!(config.max_age_days, 7);
        assert_eq!(config.max_bookmarks, defaults.max_bookmarks);
        assert!(config.debug);
        assert_eq!(parse_config(&json!([]), &PROD), defaults);
    }

    #[test]
    fn test_load_config_missing_or_broken() {
        let dir = TempDir::new("config");
        let path = dir.join("config.json");
        assert_eq!(load_config(&path, &PROD), default_config(&PROD));
        std::fs::write(&path, "{broken").expect("запись");
        assert_eq!(load_config(&path, &PROD), default_config(&PROD));
    }
}

mod storage_tests {
    use super::*;

    #[test]
    fn test_roundtrip_and_atomic_replace() {
        let dir = TempDir::new("roundtrip");
        let path = dir.0.join("sub").join("bookmarks.json");
        let items = vec![bm("1"), Bookmark { created: "2026-11-19T11:00:00".into(), ..bm("2") }];
        save_bookmarks(&path, &items).expect("запись");
        assert!(!dir.0.join("sub").join("bookmarks.json.tmp").exists());
        assert_eq!(load_bookmarks(&path, &now(), 30, 300), items); // mode="uia", page/doc/url = ""
    }

    #[test]
    fn test_expiry_and_limit() {
        let n = now();
        let mut items: Vec<Bookmark> = (0..5)
            .map(|i| Bookmark { created: format!("2026-11-{:02}T00:00:00", 20 - i), ..bm(&i.to_string()) })
            .collect();
        items.push(Bookmark { created: "not a date".into(), ..bm("bad") });
        let kept = prune(items.clone(), &n, 3, 300);
        let mut kept_ids = ids(&kept);
        kept_ids.sort();
        assert_eq!(kept_ids, vec!["0", "1", "2", "3"]);
        assert_eq!(ids(&prune(items, &n, 30, 2)), vec!["1", "0"]);
    }

    #[test]
    fn test_damaged_file_is_set_aside() {
        let dir = TempDir::new("damaged");
        let path = dir.join("bookmarks.json");
        std::fs::write(&path, "{oops").expect("запись");
        assert_eq!(load_bookmarks(&path, &now_local(), 30, 300), Vec::new());
        assert!(dir.join("bookmarks.json.bad").exists());
    }

    #[test]
    fn test_invalid_entries_dropped_and_extra_fields_stripped() {
        let dir = TempDir::new("invalid");
        let path = dir.join("bookmarks.json");
        let mut good = bookmark_json("ok", &[]);
        good["junk"] = json!("x");
        let payload = json!({"version": 1, "bookmarks": [good, {"id": 1}, bookmark_json("k", &[("kind", json!("other"))]),
                                                          bookmark_json("q", &[("quote", json!(""))])]});
        std::fs::write(&path, payload.to_string()).expect("запись");
        let loaded = load_bookmarks(&path, &now(), 30, 300);
        assert_eq!(ids(&loaded), vec!["ok"]);
        // Лишних полей в Bookmark нет по построению; при записи их тоже нет.
        save_bookmarks(&path, &loaded).expect("запись");
        assert!(!std::fs::read_to_string(&path).expect("чтение").contains("junk"));
    }

    #[test]
    fn test_missing_file() {
        let dir = TempDir::new("missing");
        assert_eq!(load_bookmarks(&dir.join("none.json"), &now_local(), 30, 300), Vec::new());
    }
}

mod review_fix_tests {
    use super::*;

    #[test]
    fn test_prune_survives_time_zone() {
        let items = vec![Bookmark { created: "2026-11-19T10:00:00+03:00".into(), ..bm("tz") }, bm("ok")];
        assert_eq!(ids(&prune(items, &now(), 30, 300)), vec!["ok"]);
    }

    #[test]
    fn test_mode_defaults_and_validation() {
        let dir = TempDir::new("mode");
        let path = dir.join("bookmarks.json");
        let payload = json!({"bookmarks": [bookmark_json("old", &[]), bookmark_json("ocr", &[("mode", json!("ocr"))]),
                                           bookmark_json("bad", &[("mode", json!("other"))])]});
        std::fs::write(&path, payload.to_string()).expect("запись");
        let loaded = load_bookmarks(&path, &now(), 30, 300);
        let got: Vec<(String, Mode)> = loaded.iter().map(|b| (b.id.clone(), b.mode)).collect();
        assert_eq!(got, vec![("old".to_string(), Mode::Uia), ("ocr".to_string(), Mode::Ocr)]);
    }

    #[test]
    fn test_config_with_bom() {
        let dir = TempDir::new("bom");
        let path = dir.join("config.json");
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(json!({"hotkey_mark": "Ctrl+Alt+Q"}).to_string().as_bytes());
        std::fs::write(&path, bytes).expect("запись");
        assert_eq!(load_config(&path, &PROD).hotkey_mark, "Ctrl+Alt+Q");
    }
}

mod v2_tests {
    use super::*;

    #[test]
    fn test_optional_fields_kept() {
        let dir = TempDir::new("optional");
        let path = dir.join("bookmarks.json");
        let item = bookmark_json("n", &[("page", json!("https://a/b")), ("doc", json!("Чат")), ("url", json!("https://a/b")), ("extra", json!(5))]);
        std::fs::write(&path, json!({"bookmarks": [item]}).to_string()).expect("запись");
        let loaded = load_bookmarks(&path, &now(), 30, 300);
        let first = &loaded[0];
        assert_eq!((first.page.as_str(), first.doc.as_str(), first.url.as_str()), ("https://a/b", "Чат", "https://a/b"));
    }

    #[test]
    fn test_config_hints_and_label() {
        let config = parse_config(&json!({"hints": false, "label_color": "#ABCDEF"}), &PROD);
        assert!(!config.hints);
        assert_eq!(config.label_color, LabelColor::Custom(Rgb(0xAB, 0xCD, 0xEF)));
        assert_eq!(parse_config(&json!({"label_color": "red"}), &PROD).label_color, LabelColor::Auto);
    }
}

mod misc_tests {
    use super::*;

    #[test]
    fn test_logs_never_contain_text() {
        let dir = TempDir::new("logs");
        let s = shared(&dir.0, true);
        // Код — только &'static str: текст пользователя сюда не передать (проверяет компилятор).
        log_error(&s, "VALUE_ERROR");
        log_timing(&s, "MARK", 12.4);
        log_timing(&s, "bad code with text", 1.0);
        let error = std::fs::read_to_string(dir.join("error.log")).expect("error.log");
        let timing = std::fs::read_to_string(dir.join("timing.log")).expect("timing.log");
        assert!(error.contains("VALUE_ERROR"));
        assert!(error.contains("store.rs:"), "при debug — файл:строка вызывающего: {error}");
        assert!(timing.contains("MARK 12"));
        assert!(!timing.contains("bad code"));
    }
}

// ---------------------------------------------------------------- прочее

#[test]
fn logs_without_debug_and_limit() {
    let dir = TempDir::new("logs2");
    let s = shared(&dir.0, false);
    log_error(&s, "MTA_USAGE");
    log_timing(&s, "MARK", 5.0); // без debug не пишется
    log_render(&s, &RenderState::default()); // без debug не пишется
    let error = std::fs::read_to_string(dir.join("error.log")).expect("error.log");
    let line = error.trim_end();
    assert!(line.ends_with(" MTA_USAGE"), "{line}");
    assert!(parse_iso(&line[..19]).is_some(), "время в начале: {line}");
    assert!(error.ends_with("\r\n"));
    assert!(!dir.join("timing.log").exists());
    assert!(!dir.join("debug-state.json").exists());
    // Больше 1 МБ — новые строки не дописываются.
    std::fs::write(dir.join("error.log"), vec![b'x'; 1024 * 1024 + 1]).expect("запись");
    log_error(&s, "MTA_USAGE");
    assert_eq!(std::fs::metadata(dir.join("error.log")).expect("размер").len(), 1024 * 1024 + 1);
    // round() Python: половина — к чётному.
    s.debug.store(true, Ordering::Relaxed);
    log_timing(&s, "SLOW_TICK", 12.5);
    log_timing(&s, "SLOW_TICK", 13.5);
    log_timing(&s, "SLOW_TICK", f64::NAN);
    let timing = std::fs::read_to_string(dir.join("timing.log")).expect("timing.log");
    let values: Vec<&str> = timing.lines().map(|l| l.rsplit(' ').next().unwrap_or("")).collect();
    assert_eq!(values, vec!["12", "14"]);
}

#[test]
fn golden_render_line() {
    let dir = TempDir::new("render");
    let s = shared(&dir.0, true);
    for case in arr(&golden("store.json")["render"]) {
        let c = arr(case);
        let marks = arr(&c[0])
            .iter()
            .map(|m| {
                let lines = arr(&m[1]).iter().map(|r| Rect { l: r[0].as_i64().unwrap_or(0) as i32, t: r[1].as_i64().unwrap_or(0) as i32, r: r[2].as_i64().unwrap_or(0) as i32, b: r[3].as_i64().unwrap_or(0) as i32 }).collect();
                (text(&m[0]).to_string(), lines)
            })
            .collect();
        let pills = arr(&c[1]).iter().map(|p| (if text(&p[0]) == "up" { Dir::Up } else { Dir::Down }, text(&p[1]).to_string())).collect();
        let viewport = c[2].as_array().map(|v| Rect { l: v[0].as_i64().unwrap_or(0) as i32, t: v[1].as_i64().unwrap_or(0) as i32, r: v[2].as_i64().unwrap_or(0) as i32, b: v[3].as_i64().unwrap_or(0) as i32 });
        log_render(&s, &RenderState { marks, pills, viewport });
        let line = std::fs::read_to_string(dir.join("debug-state.json")).expect("debug-state.json");
        // Время своё; остальное — байт в байт как json.dumps Python.
        let want = text(&c[3]);
        assert_eq!(line.len(), want.len(), "{line}");
        assert_eq!(&line[..10], &want[..10]);
        assert_eq!(&line[33..], &want[33..]);
    }
    let history = std::fs::read_to_string(dir.join("debug-render.log")).expect("debug-render.log");
    assert_eq!(history.matches("\r\n").count(), 3);
}

#[test]
fn panic_hook_logs_only_location() {
    let dir = TempDir::new("panic");
    let s = Arc::new(shared(&dir.0, false));
    let previous = std::panic::take_hook();
    install_panic_hook(s);
    let result = std::panic::catch_unwind(|| panic!("секретный текст пользователя"));
    std::panic::set_hook(previous);
    assert!(result.is_err());
    let error = std::fs::read_to_string(dir.join("error.log")).expect("error.log");
    assert!(error.contains(" PANIC at "), "{error}");
    assert!(error.contains("store.rs:"), "{error}");
    assert!(!error.contains("секрет"));
}

#[test]
fn clock_and_ids() {
    let t = now_local();
    assert!((2026..2200).contains(&t.y));
    let text = iso_seconds(&t);
    assert_eq!(text.len(), 19);
    assert_eq!(parse_iso(&text), Some(LocalTime { us: 0, ..t }));
    assert_eq!(iso_seconds(&LocalTime { y: 2026, m: 11, d: 19, h: 22, mi: 40, s: 7, us: 5 }), "2026-11-19T22:40:07");
    let mut seen = std::collections::HashSet::new();
    for _ in 0..200 {
        let id = new_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{id}");
        assert_eq!(&id[12..13], "4", "версия uuid4: {id}");
        assert!(seen.insert(id));
    }
}

#[test]
fn profile_defaults() {
    let dev = default_config(&DEV);
    assert_eq!((dev.hotkey_mark.as_str(), dev.hotkey_extra.as_str()), ("Ctrl+Alt+X", "Ctrl+Alt+Shift+X"));
    assert_eq!(dev.bar_color, Rgb(0x34, 0xC7, 0x59));
    let prod = default_config(&PROD);
    assert_eq!((prod.hotkey_mark.as_str(), prod.bar_color), ("Ctrl+Alt+Z", Rgb(0xFF, 0x7A, 0x00)));
    // Битые значения в config.json DEV-профиля -> значения DEV, а не Python.
    assert_eq!(parse_config(&json!({"hotkey_mark": "Ctrl+C", "bar_color": "x"}), &DEV), dev);
}

#[test]
fn save_config_keys_keeps_unknown_keys_and_order() {
    let dir = TempDir::new("savecfg");
    let path = dir.join("config.json");
    std::fs::write(&path, "\u{feff}{\"zeta\": {\"deep\": [1, 2.5, null]}, \"color\": \"#000000\", \"alpha\": \"текст\", \"debug\": true}").expect("запись");
    save_config_keys(&path, &[("opacity", json!(0.35)), ("color", json!("#ABCDEF"))]).expect("сохранение");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("чтение")).expect("JSON");
    let keys: Vec<&String> = saved.as_object().expect("объект").keys().collect();
    assert_eq!(keys, vec!["zeta", "color", "alpha", "debug", "opacity"]);
    assert_eq!(saved["zeta"], json!({"deep": [1, 2.5, null]}));
    assert_eq!(saved["alpha"], json!("текст"));
    assert_eq!(saved["color"], json!("#ABCDEF"));
    assert!(!dir.join("config.json.tmp").exists());
    assert_eq!(load_config(&path, &PROD).opacity, 0.35);
}

#[test]
fn golden_save_config_and_default_file() {
    let g = golden("store.json");
    let dir = TempDir::new("cfggold");
    let path = dir.join("config.json");
    // ensure_config: как main() Python — файл DEFAULT_CONFIG, отступ 2, CRLF, перевод строки в конце.
    ensure_config(&path, &PROD);
    assert_eq!(std::fs::read_to_string(&path).expect("чтение"), text(&g["default_config_text"]));
    std::fs::write(&path, "{}").expect("запись");
    ensure_config(&path, &PROD); // уже есть — не трогать
    assert_eq!(std::fs::read_to_string(&path).expect("чтение"), "{}");

    let saves = arr(&g["save_config"]);
    let changes: Vec<(&str, Value)> = saves[1].as_object().expect("изменения").iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    for case in arr(&saves[0]) {
        let _ = std::fs::remove_file(&path);
        if let Some(input) = case[0].as_str() {
            std::fs::write(&path, input).expect("запись");
        }
        save_config_keys(&path, &changes).expect("сохранение");
        assert_eq!(std::fs::read_to_string(&path).expect("чтение"), text(&case[1]), "вход: {}", case[0]);
    }
}

// ---------------------------------------------------------------- эталоны Python

fn expected_config(v: &Value) -> Config {
    let color = |k: &str| Rgb::parse(text(&v[k])).unwrap_or_else(|| panic!("цвет {k}: {v}"));
    Config {
        hotkey_mark: text(&v["hotkey_mark"]).to_string(),
        hotkey_extra: text(&v["hotkey_extra"]).to_string(),
        color: color("color"),
        opacity: v["opacity"].as_f64().expect("opacity"),
        bar_color: color("bar_color"),
        label_color: if v["label_color"] == "auto" { LabelColor::Auto } else { LabelColor::Custom(color("label_color")) },
        max_age_days: v["max_age_days"].as_u64().expect("days") as u32,
        max_bookmarks: v["max_bookmarks"].as_u64().expect("count") as u32,
        hints: v["hints"].as_bool().expect("hints"),
        ocr: v["ocr"].as_bool().expect("ocr"),
        copy_fallback: v["copy_fallback"].as_bool().expect("copy_fallback"),
        debug: v["debug"].as_bool().expect("debug"),
    }
}

#[test]
fn golden_parse_config() {
    let cases = golden("config.json");
    let cases = arr(&cases);
    for case in cases {
        assert_eq!(parse_config(&case[0], &PROD), expected_config(&case[1]), "{}", case[0]);
    }
    assert!(cases.len() >= 100);
    eprintln!("parse_config: {} эталонов совпали", cases.len());
}

#[test]
fn golden_parse_iso_and_prune() {
    let g = golden("misc.json");
    for case in arr(&g["iso"]) {
        let want = case[1].as_array().map(|v| {
            let n = |i: usize| v[i].as_u64().expect("число") as u32;
            LocalTime { y: n(0) as i32, m: n(1), d: n(2), h: n(3), mi: n(4), s: n(5), us: n(6) }
        });
        assert_eq!(parse_iso(text(&case[0])), want, "{}", case[0]);
    }
    for case in arr(&g["prune"]) {
        let items: Vec<Bookmark> =
            arr(&case[0]).iter().map(|p| Bookmark { created: text(&p[1]).to_string(), ..bm(text(&p[0])) }).collect();
        let now = parse_iso(text(&case[1])).expect("now");
        let kept = prune(items, &now, case[2].as_u64().expect("days") as u32, case[3].as_u64().expect("count") as usize);
        let want: Vec<String> = arr(&case[4]).iter().map(|v| text(v).to_string()).collect();
        assert_eq!(ids(&kept), want, "{case}");
    }
    eprintln!("parse_iso: {}, prune: {} эталонов совпали", arr(&g["iso"]).len(), arr(&g["prune"]).len());
}

/// Python пишет файл -> Rust читает без потерь и пишет его же байт в байт.
#[test]
fn python_file_reads_in_rust_and_rewrites_identically() {
    let dir = TempDir::new("pyfile");
    let path = dir.join("bookmarks.json");
    let original = std::fs::read(golden_path("bookmarks_py.json")).expect("bookmarks_py.json");
    std::fs::write(&path, &original).expect("копия");
    let loaded = load_bookmarks(&path, &now(), 3650, 1000); // prune сортирует по created
    let expected: Vec<Bookmark> = arr(&golden("bookmarks_py_expected.json")).iter().map(from_python).collect();
    assert_eq!(loaded, expected);
    // Те же записи в порядке файла -> Rust пишет файл байт в байт как Python.
    let raw: Value = serde_json::from_slice(&original).expect("JSON");
    let in_file_order: Vec<Bookmark> = arr(&raw["bookmarks"]).iter().map(|v| valid_bookmark(v).expect("годная")).collect();
    let copy = dir.join("again.json");
    save_bookmarks(&copy, &in_file_order).expect("запись");
    assert_eq!(std::fs::read(&copy).expect("чтение"), original, "файл Rust отличается от файла Python");

    // Смешанный файл: битые записи, лишние поля, mode, необязательные поля, срок.
    let mixed = dir.join("mixed.json");
    std::fs::copy(golden_path("bookmarks_mixed.json"), &mixed).expect("копия");
    let loaded = load_bookmarks(&mixed, &now(), 30, 300);
    let expected: Vec<Bookmark> = arr(&golden("bookmarks_mixed_expected.json")).iter().map(from_python).collect();
    assert_eq!(loaded, expected);
}

/// Rust пишет файл -> `load_bookmarks` Python (через `gen_golden.py --check`) читает без потерь.
#[test]
fn rust_file_reads_in_python() {
    let Some(python) = old_python() else {
        eprintln!("ПРОПУЩЕН: нет интерпретатора старой версии (задайте ZAKLADKA_OLD_PYTHON)");
        return;
    };
    let dir = TempDir::new("rsfile");
    let path = dir.join("bookmarks.json");
    // В порядке created: load_bookmarks (prune) сортирует по нему.
    let items = vec![
        Bookmark { id: new_id(), kind: Kind::Extra, mode: Mode::Ocr, exe: "editors.exe".into(), created: "2026-11-01T00:00:00".into(), ..bm("y") },
        Bookmark {
            id: new_id(),
            quote: "кавычки\"и\\слэш/\u{2028}\u{1}\u{7f}".into(),
            prefix: "\n\t\r\u{8}\u{c}".into(),
            suffix: "👨\u{200d}👩\u{200d}👧 ß İ ﬁ 漢字 \u{a0}".into(),
            label: "Подпись & <тег>".into(),
            title: "Чат — Claude".into(),
            page: "https://claude.ai/chat/1".into(),
            doc: "Чат".into(),
            url: "https://claude.ai/chat/1#x".into(),
            ..bm("x")
        },
    ];
    save_bookmarks(&path, &items).expect("запись");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools").join("gen_golden.py");
    let output = std::process::Command::new(&python)
        .arg("-B")
        .arg("-I")
        .arg(&script)
        .arg("--check")
        .arg(&path)
        .output()
        .expect("запуск Python");
    assert!(output.status.success(), "Python: {}", String::from_utf8_lossy(&output.stderr));
    let loaded: Value = serde_json::from_slice(&output.stdout).expect("JSON от Python");
    let loaded: Vec<Bookmark> = arr(&loaded).iter().map(from_python).collect();
    assert_eq!(loaded, items);
}
