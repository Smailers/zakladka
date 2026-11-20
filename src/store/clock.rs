//! Местное время без chrono и случайные id без uuid.

/// Местное время. Поля и их типы — на усмотрение части 1
/// (снаружи пользуются только функциями ниже); порядок полей даёт хронологический `Ord`.
/// `us` — микросекунды: `datetime.now()` и `fromisoformat` в Python их хранят, а срок `prune`
/// сравнивается с ними.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalTime {
    pub y: i32,
    pub m: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub us: u32,
}

fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Номер дня: 0001-01-01 = 1 (`date.toordinal()` Python).
fn ymd_to_ord(y: i32, m: u32, d: u32) -> i64 {
    let y1 = (y - 1) as i64;
    let before_year = y1 * 365 + y1 / 4 - y1 / 100 + y1 / 400;
    let before_month: i64 = (1..m).map(|month| days_in_month(y, month) as i64).sum();
    before_year + before_month + d as i64
}

/// Обратно к (год, месяц, день); алгоритм «days from civil» (Howard Hinnant).
fn ord_to_ymd(ordinal: i64) -> (i64, u32, u32) {
    let z = ordinal - 719_163 + 719_468; // 1970-01-01 = 719163
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

impl LocalTime {
    /// Микросекунды от 0001-01-01T00:00:00 — для сравнения и вычитания сроков.
    pub(crate) fn micros(&self) -> i64 {
        let days = ymd_to_ord(self.y, self.m, self.d) - 1;
        let seconds = days * 86_400 + self.h as i64 * 3600 + self.mi as i64 * 60 + self.s as i64;
        seconds * 1_000_000 + self.us as i64
    }

    /// Проверка, как у конструктора `datetime`.
    fn valid(&self) -> bool {
        (1..=9999).contains(&self.y)
            && (1..=12).contains(&self.m)
            && self.d >= 1
            && self.d <= days_in_month(self.y, self.m)
            && self.h <= 23
            && self.mi <= 59
            && self.s <= 59
            && self.us <= 999_999
    }
}

/// `GetLocalTime`.
pub fn now_local() -> LocalTime {
    // SAFETY: GetLocalTime только заполняет структуру SYSTEMTIME.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    LocalTime {
        y: t.wYear as i32,
        m: t.wMonth as u32,
        d: t.wDay as u32,
        h: t.wHour as u32,
        mi: t.wMinute as u32,
        s: t.wSecond as u32,
        us: t.wMilliseconds as u32 * 1000,
    }
}

/// "2026-11-19T22:40:07".
pub fn iso_seconds(t: &LocalTime) -> String {
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.y, t.m, t.d, t.h, t.mi, t.s)
}

/// "2026-11-19T22:40:07.123" (`isoformat(timespec="milliseconds")`).
pub(crate) fn iso_millis(t: &LocalTime) -> String {
    format!("{}.{:03}", iso_seconds(t), t.us / 1000)
}

/// Байт строки; за концом — 0, как NUL в C-строке.
fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// `parse_digits` из `_datetimemodule.c`: ровно `count` цифр ASCII с позиции `p`.
fn digits(b: &[u8], p: &mut usize, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..count {
        let c = at(b, *p);
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (c - b'0') as u32;
        *p += 1;
    }
    Some(value)
}

/// `_find_isoformat_datetime_separator` (CPython 3.13).
fn separator_location(b: &[u8]) -> Option<usize> {
    let len = b.len();
    if len == 7 {
        return Some(7);
    }
    if at(b, 4) == b'-' {
        if at(b, 5) == b'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && at(b, 8) == b'-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && at(b, 10).is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if at(b, 4) == b'W' {
        let mut idx = 7;
        while idx < len && at(b, idx).is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// Понедельник первой ISO-недели года (номер дня).
fn iso_week1_monday(year: i32) -> i64 {
    let first_day = ymd_to_ord(year, 1, 1);
    let first_weekday = (first_day + 6) % 7; // 0 — понедельник
    let mut monday = first_day - first_weekday;
    if first_weekday > 3 {
        monday += 7;
    }
    monday
}

/// `iso_to_ymd` (CPython 3.13).
fn iso_to_ymd(year: i32, week: u32, day: u32) -> Option<(i32, u32, u32)> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    if week == 0 || week >= 53 {
        let mut out_of_range = true;
        if week == 53 {
            let first_weekday = (ymd_to_ord(year, 1, 1) + 6) % 7;
            if first_weekday == 3 || (first_weekday == 2 && is_leap(year)) {
                out_of_range = false;
            }
        }
        if out_of_range {
            return None;
        }
    }
    if day == 0 || day >= 8 {
        return None;
    }
    let ordinal = iso_week1_monday(year) + (week as i64 - 1) * 7 + day as i64 - 1;
    let (y, m, d) = ord_to_ymd(ordinal);
    Some((i32::try_from(y).ok()?, m, d))
}

/// `parse_isoformat_date` (CPython 3.13); `len` — место разделителя.
fn parse_date(b: &[u8], len: Option<usize>) -> Option<(i32, u32, u32)> {
    let mut p = 0usize;
    let year = digits(b, &mut p, 4)? as i32;
    let uses_separator = at(b, p) == b'-';
    if uses_separator {
        p += 1;
    }
    if at(b, p) == b'W' {
        p += 1;
        let week = digits(b, &mut p, 2)?;
        // Разделитель не найден (-1 в C) — длина «бесконечна».
        let day = if len.map_or(true, |len| p < len) {
            if uses_separator {
                let c = at(b, p);
                p += 1;
                if c != b'-' {
                    return None;
                }
            }
            digits(b, &mut p, 1)?
        } else {
            1
        };
        return iso_to_ymd(year, week, day);
    }
    let month = digits(b, &mut p, 2)?;
    if uses_separator {
        let c = at(b, p);
        p += 1;
        if c != b'-' {
            return None;
        }
    }
    let day = digits(b, &mut p, 2)?;
    Some((year, month, day))
}

/// `parse_hh_mm_ss_ff` (CPython 3.13) на `b[start..end]`; `true` — после времени что-то осталось.
fn parse_time_part(b: &[u8], start: usize, end: usize) -> Option<([u32; 4], bool)> {
    let mut values = [0u32; 4];
    let mut p = start;
    let mut has_separator = true;
    for i in 0..3 {
        values[i] = digits(b, &mut p, 2)?;
        let c = at(b, p);
        p += 1;
        if i == 0 {
            has_separator = c == b':';
        }
        if p >= end {
            return Some((values, c != 0));
        } else if has_separator && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            break;
        } else if !has_separator {
            p -= 1;
        } else {
            return None;
        }
    }
    let remains = end.saturating_sub(p);
    let to_parse = remains.min(6);
    let mut micro = digits(b, &mut p, to_parse)?;
    const CORRECTION: [u32; 5] = [100_000, 10_000, 1000, 100, 10];
    if to_parse < 6 {
        // to_parse >= 1: при p >= end функция уже вернулась выше.
        micro *= CORRECTION.get(to_parse.wrapping_sub(1)).copied().unwrap_or(1);
    }
    values[3] = micro;
    while at(b, p).is_ascii_digit() {
        p += 1;
    }
    Some((values, at(b, p) != 0))
}

/// Как `datetime.fromisoformat` (C-реализация CPython 3.13); со смещением зоны -> `None`
/// (в Python такая дата «aware», и сравнение в `prune` её отбрасывает).
pub fn parse_iso(s: &str) -> Option<LocalTime> {
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    // Разделитель даты и времени — любой один символ (позиция 7, 8 или 10); не-ASCII на этом месте
    // CPython заменяет на 'T' до разбора байтов UTF-8. Остальной не-ASCII даёт ошибку разбора.
    if let Some(pos) = [7usize, 8, 10].into_iter().take_while(|&pos| pos <= chars.len()).find(|&pos| chars.get(pos).is_some_and(|c| !c.is_ascii())) {
        chars[pos] = 'T';
    }
    let clean: String = chars.into_iter().collect();
    let b = clean.as_bytes();
    let separator = separator_location(b);
    let (y, m, d) = parse_date(b, separator)?;
    let mut time = [0u32; 4];
    if let Some(sep) = separator {
        if b.len() > sep {
            let start = sep + 1;
            let end = b.len();
            // Поиск 'Z', '+', '-' как в C: хотя бы один символ проверяется всегда.
            let mut tz = start;
            loop {
                let c = at(b, tz);
                if c == b'Z' || c == b'+' || c == b'-' {
                    break;
                }
                tz += 1;
                if tz >= end {
                    break;
                }
            }
            let (values, rest) = parse_time_part(b, start, tz)?;
            if tz < end || rest {
                return None; // смещение зоны или лишнее в конце
            }
            time = values;
        }
    }
    let t = LocalTime { y, m, d, h: time[0], mi: time[1], s: time[2], us: time[3] };
    t.valid().then_some(t)
}

/// 32 hex, случайные (BCryptGenRandom / RtlGenRandom), как `uuid.uuid4().hex`.
pub fn new_id() -> String {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut bytes = [0u8; 16];
    // SAFETY: буфер свой, размер передаётся вместе с ним.
    let status = unsafe { BCryptGenRandom(BCRYPT_ALG_HANDLE(std::ptr::null_mut()), &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.is_err() {
        // Запасной путь: RandomState берёт ключи из системного генератора процесса.
        use std::hash::{BuildHasher, Hasher};
        for (k, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(k);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // версия 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // вариант RFC 4122
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
