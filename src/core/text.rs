//! Тексты интерфейса (P 26, 538–540, 606–619; N 41–48, 130–155).

use crate::core::flatten::{py_split_join, py_strip};
use crate::types::Status;

/// Понятные имена программ (N 41–46).
const PROGRAM_NAMES: &[(&str, &str)] = &[
    ("claude", "Claude"), ("chatgpt", "ChatGPT"), ("codex", "Codex"), ("antigravity", "Antigravity"),
    ("brave", "Brave"), ("chrome", "Chrome"), ("msedge", "Edge"), ("firefox", "Firefox"),
    ("browser", "Яндекс"), ("editors", "ONLYOFFICE"), ("winword", "Word"), ("excel", "Excel"),
    ("powerpnt", "PowerPoint"), ("notepad", "Блокнот"), ("code", "VS Code"), ("anydesk", "AnyDesk"),
];

/// `HELP_TEXT` (P 606–619) с местами `{mark}`, `{extra}`, `{days}`.
const HELP_TEXT: &str = concat!(
    "Поставить: выделите текст и нажмите {mark}. На тексте появится подсветка, ",
    "полоска слева и красный ✕ над ней.\n\n",
    "Закладка держится за сам текст: едет вместе с ним при прокрутке, прячется в другом ",
    "чате или окне и возвращается, когда этот текст снова на экране. Закладок сколько угодно.\n\n",
    "Новая закладка в том же чате переносит старую — это место, где вы остановились. ",
    "{extra} — дополнительная закладка, старая остаётся.\n\n",
    "Перейти: щёлкните значок «Закладка» в трее и выберите закладку — откроется её окно и ",
    "вкладка, текст прокрутится и мигнёт. {mark} без выделения — к закладке этого окна. ",
    "Плашка ▲/▼ у края значит: закладка выше или ниже, щёлкните по ней.\n\n",
    "Снять: щёлкнуть красный ✕; или выделить отмеченный текст и нажать {mark}; ",
    "или ✕ в списке. «Вернуть» отменяет удаление. Серые строки — окно или вкладка закрыты: ",
    "откройте их снова, и закладка вернётся. Закладки старше {days} дней удаляются сами.",
);

/// Первые `n` символов (`text[:n]` Python).
fn take_chars(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// `text` с многоточием, чтобы поместиться в `width` пикселей (N 142–155).
/// `measure` — ширина строки в пикселях выбранным шрифтом; `ui::gfx::fit_text` — то же.
pub fn fit_text(measure: &dyn Fn(&str) -> i32, text: &str, width: i32) -> String {
    if width <= 0 {
        return String::new();
    }
    if measure(text) <= width {
        return text.to_string();
    }
    let cut = |n: usize| format!("{}…", take_chars(text, n).trim_end_matches(crate::core::flatten::py_isspace));
    let (mut low, mut high) = (0usize, text.chars().count());
    while low < high {
        let middle = (low + high + 1) / 2;
        if measure(&cut(middle)) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    cut(low)
}

pub const APP_TITLE: &str = "Закладка";

/// Пояснение к серой строке «закрыто» (N 47).
pub const CLOSED_HINT: &str = "Окно или вкладка закрыты. Откройте их снова — закладка вернётся.";

/// `HELP_TEXT` дословно (P 606–619) с подставленными клавишами и сроком.
pub fn help_text(mark: &str, extra: &str, days: u32) -> String {
    // Один проход, как `str.format`: подставленный текст повторно не разбирается.
    let days = days.to_string();
    let mut out = String::with_capacity(HELP_TEXT.len() + 64);
    let mut rest = HELP_TEXT;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let (value, name_len) = if rest.starts_with("{mark}") {
            (mark, 6)
        } else if rest.starts_with("{extra}") {
            (extra, 7)
        } else if rest.starts_with("{days}") {
            (days.as_str(), 6)
        } else {
            ("{", 1)
        };
        out.push_str(value);
        rest = &rest[name_len..];
    }
    out.push_str(rest);
    out
}

/// Понятное имя программы из имени exe или пути: «msedge.exe» -> «Edge» (N 130–135).
pub fn program_name(exe: &str) -> String {
    let unified = exe.replace('/', "\\");
    let mut name = py_strip(unified.rsplit('\\').next().unwrap_or(""));
    if name.to_lowercase().ends_with(".exe") {
        let count = name.chars().count();
        name = take_chars(name, count.saturating_sub(4));
    }
    let lower = name.to_lowercase();
    if let Some((_, readable)) = PROGRAM_NAMES.iter().find(|(key, _)| *key == lower) {
        return readable.to_string();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Одной строкой, пробелы между словами одиночные.
pub fn one_line(text: &str) -> String {
    py_split_join(text)
}

/// Подпись пункта меню: одной строкой, «&» удвоены, не длиннее `limit` (P 538–540).
pub fn menu_label(text: &str, limit: usize) -> String {
    let text = py_split_join(text).replace('&', "&&");
    let count = text.chars().count();
    if count <= limit {
        return text;
    }
    // text[:limit - 1]; при limit = 0 Python берёт text[:-1].
    let keep = if limit == 0 { count - 1 } else { limit - 1 };
    format!("{}…", take_chars(&text, keep))
}

/// "закрыто" / "текст не найден" / "".
pub fn status_tag(s: Status) -> &'static str {
    match s {
        Status::Closed => "закрыто",
        Status::NotFound => "текст не найден",
        Status::Shown => "",
    }
}
