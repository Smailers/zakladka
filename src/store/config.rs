//! config.json (P 28–41, 426–463): те же ключи и проверки; клавиши и акцент по умолчанию — из профиля.
//! Файл пишется как в Python на Windows: отступ 2, `ensure_ascii=False`, строки CRLF, в конце перевод строки.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::app_id::Profile;
use crate::core::hotkey::safe_hotkey;
use crate::types::{Config, LabelColor, Rgb};

/// Акцент по умолчанию в Python (`DEFAULT_CONFIG["bar_color"]`), если в профиле что-то не то.
const FALLBACK_BAR: Rgb = Rgb(0xFF, 0x7A, 0x00);

/// `DEFAULT_CONFIG` из Python, но `hotkey_mark`, `hotkey_extra`, `bar_color` — из `profile`.
pub fn default_config(profile: &Profile) -> Config {
    Config {
        hotkey_mark: profile.hotkey_mark.to_string(),
        hotkey_extra: profile.hotkey_extra.to_string(),
        color: Rgb(0xFF, 0xD4, 0x00),
        opacity: 0.3,
        bar_color: Rgb::parse(profile.bar_color).unwrap_or(FALLBACK_BAR),
        label_color: LabelColor::Auto,
        max_age_days: 30,
        max_bookmarks: 300,
        hints: true,
        ocr: true,
        copy_fallback: true,
        debug: false,
    }
}

/// Целое JSON (не дробное и не bool) в пределах — как `isinstance(value, int)` в Python.
fn int_in(value: Option<&Value>, low: i64, high: i64) -> Option<u32> {
    let v = value?.as_i64()?;
    if (low..=high).contains(&v) {
        u32::try_from(v).ok()
    } else {
        None
    }
}

/// Все проверки `parse_config`; не прошедшие ключи — из `default_config(profile)`.
pub fn parse_config(payload: &serde_json::Value, profile: &Profile) -> Config {
    let mut config = default_config(profile);
    let Some(payload) = payload.as_object() else {
        return config;
    };
    for (key, slot) in [("hotkey_mark", &mut config.hotkey_mark), ("hotkey_extra", &mut config.hotkey_extra)] {
        if let Some(value) = payload.get(key).and_then(Value::as_str) {
            if safe_hotkey(value).is_ok() {
                *slot = value.to_string();
            }
        }
    }
    for (key, slot) in [("color", &mut config.color), ("bar_color", &mut config.bar_color)] {
        if let Some(color) = payload.get(key).and_then(Value::as_str).and_then(Rgb::parse) {
            *slot = color;
        }
    }
    // int или float, не bool: в serde_json bool — не число.
    if let Some(value) = payload.get("opacity").and_then(Value::as_f64) {
        if (0.1..=0.9).contains(&value) {
            config.opacity = value;
        }
    }
    if let Some(days) = int_in(payload.get("max_age_days"), 1, 3650) {
        config.max_age_days = days;
    }
    if let Some(count) = int_in(payload.get("max_bookmarks"), 10, 5000) {
        config.max_bookmarks = count;
    }
    for (key, slot) in [
        ("hints", &mut config.hints),
        ("ocr", &mut config.ocr),
        ("copy_fallback", &mut config.copy_fallback),
        ("debug", &mut config.debug),
    ] {
        if let Some(flag) = payload.get(key).and_then(Value::as_bool) {
            *slot = flag;
        }
    }
    match payload.get("label_color").and_then(Value::as_str) {
        Some("auto") => config.label_color = LabelColor::Auto,
        Some(text) => {
            if let Some(color) = Rgb::parse(text) {
                config.label_color = LabelColor::Custom(color);
            }
        }
        None => {}
    }
    config
}

/// Содержимое JSON-файла, BOM допустим (`encoding="utf-8-sig"`).
fn read_json_sig(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = std::str::from_utf8(bytes).ok()?;
    serde_json::from_str(text).ok()
}

/// BOM допустим; ошибка чтения или разбора -> `default_config(profile)`.
pub fn load_config(path: &Path, profile: &Profile) -> Config {
    match read_json_sig(path) {
        Some(payload) => parse_config(&payload, profile),
        None => default_config(profile),
    }
}

/// Строка JSON как `json.dumps(s, ensure_ascii=False)`: экранируются `"`, `\`, управляющие символы.
pub(crate) fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `float.__repr__` Python: кратчайшие цифры; экспонента при decpt <= -4 или > 16 («1e-05», «1e+16»).
pub(crate) fn py_float_repr(x: f64) -> String {
    if !x.is_finite() {
        // В JSON Python пишет NaN/Infinity; serde_json такие значения не читает и не хранит.
        return "null".to_string();
    }
    let sci = format!("{:e}", x); // «-1.2345e-7»: кратчайшие цифры, как у repr
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exponent + 1;
    if decpt <= -4 || decpt > 16 {
        let mut text = String::from(sign);
        text.push_str(&digits[..1]);
        if digits.len() > 1 {
            text.push('.');
            text.push_str(&digits[1..]);
        }
        text.push_str(&format!("e{}{:02}", if exponent < 0 { '-' } else { '+' }, exponent.abs()));
        text
    } else if decpt <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt as usize >= digits.len() {
        format!("{sign}{digits}{}.0", "0".repeat(decpt as usize - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    }
}

fn push_newline(out: &mut String, indent: &str, level: usize) {
    out.push_str("\r\n"); // текстовый режим Python на Windows
    for _ in 0..level {
        out.push_str(indent);
    }
}

fn push_value(out: &mut String, value: &Value, indent: &str, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                out.push_str(&n.to_string());
            } else {
                out.push_str(&py_float_repr(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => push_json_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                push_newline(out, indent, level + 1);
                push_value(out, item, indent, level + 1);
            }
            push_newline(out, indent, level);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (k, (key, item)) in map.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                push_newline(out, indent, level + 1);
                push_json_string(out, key);
                out.push_str(": ");
                push_value(out, item, indent, level + 1);
            }
            push_newline(out, indent, level);
            out.push('}');
        }
    }
}

/// `json.dump(value, ensure_ascii=False, indent=len(indent))` (+ `"\n"`), как пишет Python на
/// Windows в текстовом режиме: переводы строк CRLF. Тот же файл байт в байт.
pub(crate) fn to_python_json(value: &Value, indent: &str, trailing_newline: bool) -> Vec<u8> {
    let mut out = String::new();
    push_value(&mut out, value, indent, 0);
    if trailing_newline {
        out.push_str("\r\n");
    }
    out.into_bytes()
}

/// Временный файл рядом, сброс на диск, замена (`os.replace`).
pub(crate) fn write_replace(path: &Path, temporary_name: &str, data: &[u8], sync: bool) -> io::Result<()> {
    use std::io::Write;
    let temporary = path.with_file_name(temporary_name);
    {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.flush()?;
        if sync {
            file.sync_all()?; // FlushFileBuffers
        }
    }
    std::fs::rename(&temporary, path) // MoveFileExW(MOVEFILE_REPLACE_EXISTING)
}

/// Чужие ключи и их порядок сохраняются; tmp + replace.
pub fn save_config_keys(path: &Path, changes: &[(&str, serde_json::Value)]) -> io::Result<()> {
    let mut payload = match read_json_sig(path) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (key, value) in changes {
        payload.insert((*key).to_string(), value.clone()); // старый ключ остаётся на своём месте
    }
    let data = to_python_json(&Value::Object(payload), "  ", true);
    write_replace(path, "config.json.tmp", &data, false)
}

/// `DEFAULT_CONFIG` как JSON, ключи в порядке Python.
fn default_payload(profile: &Profile) -> Value {
    let c = default_config(profile);
    let mut map = Map::new();
    map.insert("hotkey_mark".into(), Value::from(c.hotkey_mark));
    map.insert("hotkey_extra".into(), Value::from(c.hotkey_extra));
    map.insert("color".into(), Value::from(c.color.hex()));
    map.insert("opacity".into(), Value::from(c.opacity));
    map.insert("bar_color".into(), Value::from(c.bar_color.hex()));
    map.insert("max_age_days".into(), Value::from(c.max_age_days));
    map.insert("max_bookmarks".into(), Value::from(c.max_bookmarks));
    map.insert("label_color".into(), Value::from("auto"));
    map.insert("hints".into(), Value::from(c.hints));
    map.insert("ocr".into(), Value::from(c.ocr));
    map.insert("copy_fallback".into(), Value::from(c.copy_fallback));
    map.insert("debug".into(), Value::from(c.debug));
    Value::Object(map)
}

/// Создать файл по умолчанию, если его нет (как `main` в Python: отступ 2 и перевод строки).
pub fn ensure_config(path: &Path, profile: &Profile) {
    use std::io::Write;
    if path.exists() {
        return;
    }
    let data = to_python_json(&default_payload(profile), "  ", true);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // open("x"): только если файла нет; уже есть — ничего не делать.
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        let _ = file.write_all(&data);
    }
}
