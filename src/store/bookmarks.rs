//! bookmarks.json (P 47–48, 466–517): `{"version": 1, "bookmarks": [...]}`, формат не меняется.
//! Rust пишет файл байт в байт как Python на Windows (отступ 1, CRLF); ключи — в порядке, в каком
//! их отдаёт `load_bookmarks` Python: обязательные, затем `mode`, затем необязательные.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::store::clock::{parse_iso, LocalTime};
use crate::store::config::{to_python_json, write_replace};
use crate::types::{Bookmark, Kind, Mode};

/// Закладка годна: все обязательные поля — строки, `kind` и `mode` известны, `quote` и `exe` не пусты.
pub fn valid_bookmark(item: &serde_json::Value) -> Option<Bookmark> {
    let item = item.as_object()?;
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
    let optional = |key: &str| text(key).unwrap_or_default();
    let mode = match item.get("mode") {
        None => Mode::Uia,
        Some(value) => Mode::parse(value.as_str()?)?,
    };
    let bookmark = Bookmark {
        id: text("id")?,
        exe: text("exe")?,
        kind: Kind::parse(&text("kind")?)?,
        mode,
        quote: text("quote")?,
        prefix: text("prefix")?,
        suffix: text("suffix")?,
        label: text("label")?,
        title: text("title")?,
        created: text("created")?,
        page: optional("page"),
        doc: optional("doc"),
        url: optional("url"),
    };
    (!bookmark.quote.is_empty() && !bookmark.exe.is_empty()).then_some(bookmark)
}

/// Срок по `created` и предел числа: остаются самые новые (P 472–482).
pub fn prune(items: Vec<Bookmark>, now: &LocalTime, max_age_days: u32, max_count: usize) -> Vec<Bookmark> {
    let cutoff = now.micros() - max_age_days as i64 * 86_400 * 1_000_000;
    // Неразборная дата и дата со смещением зоны (TypeError в Python) отбрасываются.
    let mut kept: Vec<Bookmark> =
        items.into_iter().filter(|item| parse_iso(&item.created).is_some_and(|t| t.micros() >= cutoff)).collect();
    kept.sort_by(|a, b| a.created.cmp(&b.created)); // устойчиво, по строке, как в Python
    // kept[-max_count:]; при 0 Python отдаёт всё.
    if max_count > 0 && kept.len() > max_count {
        kept.drain(..kept.len() - max_count);
    }
    kept
}

/// Битый файл -> `*.bad`, никогда не фатально.
pub fn load_bookmarks(path: &Path, now: &LocalTime, max_age_days: u32, max_count: usize) -> Vec<Bookmark> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => {
            set_aside(path);
            return Vec::new();
        }
    };
    // Как `read_text(encoding="utf-8")` + `json.loads`: BOM здесь — ошибка разбора.
    let payload: Value = match std::str::from_utf8(&bytes).ok().and_then(|text| serde_json::from_str(text).ok()) {
        Some(payload) => payload,
        None => {
            set_aside(path);
            return Vec::new();
        }
    };
    let Some(items) = payload.get("bookmarks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let clean: Vec<Bookmark> = items.iter().filter_map(valid_bookmark).collect();
    prune(clean, now, max_age_days, max_count)
}

/// `path.replace(path + ".bad")`; ошибка не важна.
fn set_aside(path: &Path) {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".bad");
    let _ = std::fs::rename(path, path.with_file_name(name));
}

fn bookmark_value(item: &Bookmark) -> Value {
    let mut map = Map::new();
    for (key, value) in [
        ("id", item.id.as_str()),
        ("exe", item.exe.as_str()),
        ("kind", item.kind.as_str()),
        ("quote", item.quote.as_str()),
        ("prefix", item.prefix.as_str()),
        ("suffix", item.suffix.as_str()),
        ("label", item.label.as_str()),
        ("title", item.title.as_str()),
        ("created", item.created.as_str()),
        ("mode", item.mode.as_str()),
        ("page", item.page.as_str()),
        ("doc", item.doc.as_str()),
        ("url", item.url.as_str()),
    ] {
        map.insert(key.to_string(), Value::from(value));
    }
    Value::Object(map)
}

/// Отступ 1, `ensure_ascii=False`; tmp + FlushFileBuffers + replace.
pub fn save_bookmarks(path: &Path, items: &[Bookmark]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut payload = Map::new();
    payload.insert("version".to_string(), Value::from(1));
    payload.insert("bookmarks".to_string(), Value::Array(items.iter().map(bookmark_value).collect()));
    let data = to_python_json(&Value::Object(payload), " ", false);
    let mut temporary = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    temporary.push(".tmp");
    write_replace(path, &temporary.to_string_lossy(), &data, true)
}
