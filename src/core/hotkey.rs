//! Разбор сочетаний клавиш (P 386–423, P 2824–2844).

use crate::core::flatten::py_strip;

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;

fn modifier(part: &str) -> Option<u32> {
    match part {
        "ctrl" | "control" => Some(MOD_CONTROL),
        "alt" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        "win" => Some(MOD_WIN),
        _ => None,
    }
}

fn named_key(part: &str) -> Option<u32> {
    match part {
        "space" => Some(0x20),
        "insert" | "ins" => Some(0x2D),
        "delete" | "del" => Some(0x2E),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" => Some(0x21),
        "pagedown" => Some(0x22),
        "pause" => Some(0x13),
        _ => None,
    }
}

/// `re.fullmatch(r"f([1-9]|1[0-9]|2[0-4])", key)` -> номер F-клавиши.
fn function_key(key: &str) -> Option<u32> {
    let digits = key.strip_prefix('f')?;
    let bytes = digits.as_bytes();
    let valid = match bytes {
        [d] => (b'1'..=b'9').contains(d),
        [b'1', d] => d.is_ascii_digit(),
        [b'2', d] => (b'0'..=b'4').contains(d),
        _ => false,
    };
    if valid {
        digits.parse().ok()
    } else {
        None
    }
}

/// Ошибки разбора — те же случаи, что `ValueError` в Python.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyError {
    /// "empty part": пустая часть между «+».
    EmptyPart,
    /// "two keys": две не-модификаторные клавиши.
    TwoKeys,
    /// "modifier and key required".
    ModifierAndKeyRequired,
    /// "unknown key".
    UnknownKey,
    /// "needs two modifiers including Alt or Win" (`safe_hotkey`).
    NotSafe,
}

/// (модификаторы | MOD_NOREPEAT, vk).
pub fn parse_hotkey(text: &str) -> Result<(u32, u32), HotkeyError> {
    let mut modifiers = 0u32;
    let mut key: Option<String> = None;
    for piece in text.split('+') {
        let part = py_strip(piece).to_lowercase();
        if part.is_empty() {
            return Err(HotkeyError::EmptyPart);
        }
        if let Some(m) = modifier(&part) {
            modifiers |= m;
        } else if key.is_none() {
            key = Some(part);
        } else {
            return Err(HotkeyError::TwoKeys);
        }
    }
    let key = match key {
        Some(key) if modifiers != 0 => key,
        _ => return Err(HotkeyError::ModifierAndKeyRequired),
    };
    let mut chars = key.chars();
    let vk = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
        _ => match function_key(&key) {
            Some(n) => 0x6F + n,
            None => named_key(&key).ok_or(HotkeyError::UnknownKey)?,
        },
    };
    Ok((modifiers | MOD_NOREPEAT, vk))
}

/// `parse_hotkey` + не меньше двух модификаторов, среди них Alt или Win.
pub fn safe_hotkey(text: &str) -> Result<(u32, u32), HotkeyError> {
    let (modifiers, vk) = parse_hotkey(text)?;
    let held = modifiers & 0xF;
    if held.count_ones() < 2 || held & (MOD_ALT | MOD_WIN) == 0 {
        return Err(HotkeyError::NotSafe);
    }
    Ok((modifiers, vk))
}

/// Имя клавиши для окна клавиш (P 2824–2832).
pub fn vk_name(vk: u32) -> Option<String> {
    if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
        return char::from_u32(vk).map(String::from);
    }
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x6F));
    }
    let name = match vk {
        0x20 => "Space",
        0x2D => "Insert",
        0x2E => "Delete",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x13 => "Pause",
        _ => return None,
    };
    Some(name.to_string())
}

/// Нажатое сочетание словами: «Ctrl+Alt+Shift+Win+Z» (P 2834–2844); без клавиши или
/// без модификаторов — `None`.
pub fn combo_text(ctrl: bool, alt: bool, shift: bool, win: bool, vk: u32) -> Option<String> {
    let key = vk_name(vk)?;
    let mut parts: Vec<&str> = [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (win, "Win")]
        .iter()
        .filter(|(held, _)| *held)
        .map(|(_, name)| *name)
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.push(&key);
    Some(parts.join("+"))
}
