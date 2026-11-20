//! Журналы в папке данных (P 548–587). Предел 1 МБ на файл.
//! В журналы — только числа, прямоугольники, первые 8 символов id и статические коды:
//! `code: &'static str` не даёт записать текст пользователя.
//! Строки кончаются CRLF — как пишет Python на Windows в те же файлы.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::store::clock::{iso_millis, iso_seconds, now_local};
use crate::types::{Dir, Rect, RenderState, Shared};

/// Предел файла журнала: больше — новые строки не пишутся.
const LOG_LIMIT: u64 = 1024 * 1024;

fn log_full(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.len() > LOG_LIMIT).unwrap_or(false)
}

/// Дописать строку в журнал папки данных, если он не переполнен. Ошибки молча теряются.
fn append(shared: &Shared, file: &str, line: &str) {
    let path = shared.data_dir.join(file);
    if log_full(&path) {
        return;
    }
    let _ = std::fs::create_dir_all(&shared.data_dir);
    if let Ok(mut stream) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = stream.write_all(format!("{line}\r\n").as_bytes());
    }
}

fn error_line(shared: &Shared, code: &str, location: Option<(&str, u32)>) {
    let mut line = format!("{} {code}", iso_seconds(&now_local()));
    if let Some((file, number)) = location {
        line.push_str(&format!(" at {file}:{number}"));
    }
    append(shared, "error.log", &line);
}

/// error.log: время, код, (при debug) файл:строка вызывающего.
#[track_caller]
pub fn log_error(shared: &Shared, code: &'static str) {
    let caller = std::panic::Location::caller();
    let location = shared.debug.load(Ordering::Relaxed).then(|| (caller.file(), caller.line()));
    error_line(shared, code, location);
}

/// Код из `[A-Z_]+`.
fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}

/// timing.log, только при debug; код `[A-Z_]+`.
pub fn log_timing(shared: &Shared, code: &'static str, ms: f64) {
    if !shared.debug.load(Ordering::Relaxed) || !valid_code(code) || !ms.is_finite() {
        return;
    }
    let rounded = ms.round_ties_even() as i64; // round() Python
    append(shared, "timing.log", &format!("{} {code} {rounded}", iso_seconds(&now_local())));
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn rect_json(r: &Rect) -> String {
    format!("[{}, {}, {}, {}]", r.l, r.t, r.r, r.b)
}

/// Строка состояния как `json.dumps` Python (разделители «, » и «: »): только id[:8] и числа.
pub(crate) fn render_line(time: &str, state: &RenderState) -> String {
    let quote = |s: &str| {
        let mut quoted = String::new();
        crate::store::config::push_json_string(&mut quoted, s);
        quoted
    };
    let marks: Vec<String> = state
        .marks
        .iter()
        .map(|(id, lines)| {
            let lines: Vec<String> = lines.iter().map(rect_json).collect();
            format!("[{}, [{}]]", quote(&short_id(id)), lines.join(", "))
        })
        .collect();
    let pills: Vec<String> = state
        .pills
        .iter()
        .map(|(dir, id)| {
            let side = match dir {
                Dir::Up => "\"up\"",
                Dir::Down => "\"down\"",
            };
            format!("[{side}, {}]", quote(&short_id(id)))
        })
        .collect();
    let viewport = state.viewport.as_ref().map_or("null".to_string(), rect_json);
    format!(
        "{{\"time\": {}, \"marks\": [{}], \"pills\": [{}], \"viewport\": {}}}",
        quote(time),
        marks.join(", "),
        pills.join(", "),
        viewport
    )
}

/// debug-state.json + debug-render.log: id[:8] и прямоугольники.
pub fn log_render(shared: &Shared, state: &RenderState) {
    if !shared.debug.load(Ordering::Relaxed) {
        return;
    }
    let line = render_line(&iso_millis(&now_local()), state);
    let _ = std::fs::create_dir_all(&shared.data_dir);
    if std::fs::write(shared.data_dir.join("debug-state.json"), line.as_bytes()).is_ok() {
        append(shared, "debug-render.log", &line);
    }
}

/// В журнал — только файл:строка паники, НИКОГДА не её текст.
pub fn install_panic_hook(shared: Arc<Shared>) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map(|l| (l.file(), l.line()));
        error_line(&shared, "PANIC", location);
    }));
}
