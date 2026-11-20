//! Размещение окон меток (часть 4; P 2333–2427): `SetWindowPos(… SWP_ASYNCWINDOWPOS)` из tracker.
//!
//! Окна создаёт UI-поток (часть 6) заранее; здесь их только ставят, прячут и меняют альфу
//! подсветок. `SWP_ASYNCWINDOWPOS` не ждёт поток-владелец окна, поэтому метка едет за текстом
//! с частотой слежения, даже когда UI-поток занят.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindowVisible, SetLayeredWindowAttributes, SetWindowPos, ShowWindowAsync, HWND_TOPMOST, LWA_ALPHA,
    SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
};

use crate::types::{to_hwnd, Dir, Hwnd, OverlayWindows, PieceBindings, Rect, FLAGS};

/// NOACTIVATE | SHOWWINDOW | ASYNCWINDOWPOS (P 2333).
const SHOW: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0 | SWP_SHOWWINDOW.0 | SWP_ASYNCWINDOWPOS.0);
/// HIDEWINDOW | NOACTIVATE | NOSIZE | NOMOVE | NOZORDER | ASYNCWINDOWPOS (P 2334).
const HIDE: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(
    SWP_HIDEWINDOW.0 | SWP_NOACTIVATE.0 | SWP_NOSIZE.0 | SWP_NOMOVE.0 | SWP_NOZORDER.0 | SWP_ASYNCWINDOWPOS.0,
);

/// Полоса: `x = max(viewport.l, left - 7)`, ширина 4.
const BAR_GAP: i32 = 7;
const BAR_WIDTH: i32 = 4;
/// Флажок: `(bar_x + 2 - size/2, top - size + 4)` — над текстом слева, не поверх него.
const FLAG_SHIFT: i32 = 2;
const FLAG_LIFT: i32 = 4;
/// Плашка: `x = viewport.r - w - 28`, `y = viewport.t + 90` (вверх) или `viewport.b - h - 170` (вниз),
/// не выше `viewport.t + 4`.
const PILL_RIGHT: i32 = 28;
const PILL_UP_TOP: i32 = 90;
const PILL_DOWN_BOTTOM: i32 = 170;
const PILL_MIN_TOP: i32 = 4;
/// Мигание: период 0,12 с, альфа 0,75.
const PULSE_S: f64 = 0.12;
const PULSE_ALPHA: f64 = 0.75;

/// (x, y, ширина, высота) — как передано в `SetWindowPos`.
type Geometry = (i32, i32, i32, i32);

pub struct Overlay {
    windows: OverlayWindows,
    bindings: Arc<Mutex<PieceBindings>>,
    opacity: f64,
    /// hwnd -> поставлено (`Some`) или спрятано нами (`None`); нет записи — состояние неизвестно.
    shown: HashMap<Hwnd, Option<Geometry>>,
    /// hwnd -> байт альфы, заданный последним (только подсветки).
    alpha: HashMap<Hwnd, u8>,
    /// id закладки -> когда кончается мигание.
    flash: HashMap<String, f64>,
    /// Что уже записано в `bindings` (замок берётся только при изменении).
    bound_flags: [Option<String>; FLAGS],
    bound_up: Option<String>,
    bound_down: Option<String>,
}

impl Overlay {
    pub fn new(w: OverlayWindows, b: Arc<Mutex<PieceBindings>>, opacity: f64) -> Overlay {
        Overlay {
            windows: w,
            bindings: b,
            opacity,
            shown: HashMap::new(),
            alpha: HashMap::new(),
            flash: HashMap::new(),
            bound_flags: Default::default(),
            bound_up: None,
            bound_down: None,
        }
    }

    /// marks: [(id, прямоугольники строк)] — самые новые первыми; pills: [(▲/▼, id)].
    /// Пределы: 24 строки на все закладки и 6 закладок (по числу полос и флажков).
    pub fn apply(&mut self, marks: &[(String, Vec<Rect>)], pills: &[(Dir, String)], viewport: Option<Rect>, now: f64) {
        self.flash.retain(|_, end| *end > now);
        let highlights = self.windows.highlights.len();
        let max_marks = self.windows.bars.len().min(self.windows.flags.len()).min(FLAGS);
        let (mut used_lines, mut used_marks) = (0usize, 0usize);
        for (id, lines) in marks {
            if used_marks >= max_marks {
                break;
            }
            let lines = &lines[..lines.len().min(highlights - used_lines)];
            if lines.is_empty() {
                break;
            }
            let pulse = match self.flash.get(id) {
                Some(&end) if end > now => (((end - now) / PULSE_S) as i64) % 2 == 0,
                _ => false,
            };
            let alpha = if pulse { PULSE_ALPHA } else { self.opacity };
            for line in lines {
                let piece = self.windows.highlights[used_lines];
                used_lines += 1;
                self.set_alpha(piece, alpha);
                self.place(piece, *line);
            }
            let (bar, flag) = mark_pieces(lines, viewport, self.windows.flag_size);
            self.place(self.windows.bars[used_marks], bar);
            self.bind_flag(used_marks, id);
            self.place(self.windows.flags[used_marks], flag);
            used_marks += 1;
        }
        for index in used_lines..highlights {
            self.drop_piece(self.windows.highlights[index]);
        }
        for index in used_marks..self.windows.bars.len() {
            self.drop_piece(self.windows.bars[index]);
        }
        for index in used_marks..self.windows.flags.len() {
            self.drop_piece(self.windows.flags[index]);
        }
        for (dir, piece) in [(Dir::Up, self.windows.pill_up), (Dir::Down, self.windows.pill_down)] {
            let id = pills.iter().find(|(d, _)| *d == dir).map(|(_, id)| id);
            match (id, viewport) {
                (Some(id), Some(view)) => {
                    self.bind_pill(dir, id);
                    self.place(piece, pill_rect(dir, view, self.windows.pill_size));
                }
                _ => self.drop_piece(piece),
            }
        }
    }

    pub fn hide_all(&mut self) {
        for piece in self.pieces() {
            self.drop_piece(piece);
        }
    }

    /// После `Command::Rerender`: UI мог сам спрятать окна — следующий `apply` ставит всё заново.
    pub fn forget_shown(&mut self) {
        self.shown.clear();
    }

    pub fn set_opacity(&mut self, v: f64) {
        self.opacity = v;
        self.alpha.clear(); // следующий `apply` задаст альфу каждой подсветке заново
    }

    /// Чужие программы могут свернуть или показать наши окна: вернуть каждому его состояние.
    pub fn guard(&mut self) {
        for piece in self.pieces() {
            let hwnd = to_hwnd(piece);
            // SAFETY: вызовы с дескриптором окна (своего; уничтоженное окно даёт ошибку, не сбой).
            unsafe {
                match self.shown.get(&piece) {
                    Some(Some(g)) => {
                        if IsIconic(hwnd).as_bool() {
                            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
                            let _ = SetWindowPos(hwnd, HWND_TOPMOST, g.0, g.1, g.2, g.3, SHOW);
                        }
                    }
                    _ => {
                        if IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
                            let _ = SetWindowPos(hwnd, HWND::default(), 0, 0, 0, 0, HIDE);
                        }
                    }
                }
            }
        }
    }

    pub fn flash(&mut self, id: &str, until: f64) {
        self.flash.insert(id.to_string(), until);
    }

    pub fn flashing(&self, now: f64) -> bool {
        self.flash.values().any(|&end| end > now)
    }

    fn pieces(&self) -> Vec<Hwnd> {
        let w = &self.windows;
        let mut all: Vec<Hwnd> = Vec::with_capacity(w.highlights.len() + w.bars.len() + w.flags.len() + 2);
        all.extend_from_slice(&w.highlights);
        all.extend_from_slice(&w.bars);
        all.extend_from_slice(&w.flags);
        all.push(w.pill_up);
        all.push(w.pill_down);
        all
    }

    fn place(&mut self, piece: Hwnd, rect: Rect) {
        let geometry = (rect.l, rect.t, rect.r.saturating_sub(rect.l).max(1), rect.b.saturating_sub(rect.t).max(1));
        if self.shown.get(&piece) != Some(&Some(geometry)) {
            // SAFETY: вызов с дескриптором окна; асинхронно — поток-владелец окна не ждём.
            unsafe {
                let _ = SetWindowPos(to_hwnd(piece), HWND_TOPMOST, geometry.0, geometry.1, geometry.2, geometry.3, SHOW);
            }
            self.shown.insert(piece, Some(geometry));
        }
    }

    fn drop_piece(&mut self, piece: Hwnd) {
        if self.shown.get(&piece) != Some(&None) {
            // SAFETY: вызов с дескриптором окна; асинхронно.
            unsafe {
                let _ = SetWindowPos(to_hwnd(piece), HWND::default(), 0, 0, 0, 0, HIDE);
            }
            self.shown.insert(piece, None);
        }
    }

    fn set_alpha(&mut self, piece: Hwnd, value: f64) {
        let byte = ((value * 255.0).round_ties_even().clamp(1.0, 255.0) as u8).max(1); // NaN -> 1
        if self.alpha.get(&piece) != Some(&byte) {
            // SAFETY: вызов с дескриптором слоёного окна.
            unsafe {
                let _ = SetLayeredWindowAttributes(to_hwnd(piece), COLORREF(0), byte, LWA_ALPHA);
            }
            self.alpha.insert(piece, byte);
        }
    }

    fn bind_flag(&mut self, index: usize, id: &str) {
        if index >= self.bound_flags.len() || self.bound_flags[index].as_deref() == Some(id) {
            return;
        }
        self.bound_flags[index] = Some(id.to_string());
        let mut bindings = self.bindings.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(slot) = bindings.flags.get_mut(index) {
            *slot = Some(id.to_string());
        }
    }

    fn bind_pill(&mut self, dir: Dir, id: &str) {
        let bound = match dir {
            Dir::Up => &mut self.bound_up,
            Dir::Down => &mut self.bound_down,
        };
        if bound.as_deref() == Some(id) {
            return;
        }
        *bound = Some(id.to_string());
        let mut bindings = self.bindings.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match dir {
            Dir::Up => bindings.pill_up = Some(id.to_string()),
            Dir::Down => bindings.pill_down = Some(id.to_string()),
        }
    }
}

/// Полоса и флажок одной закладки по её строкам (`lines` не пуст).
fn mark_pieces(lines: &[Rect], viewport: Option<Rect>, size: i32) -> (Rect, Rect) {
    let left = lines.iter().map(|r| r.l).min().unwrap_or(0);
    let top = lines.iter().map(|r| r.t).min().unwrap_or(0);
    let bottom = lines.iter().map(|r| r.b).max().unwrap_or(0);
    let bar_x = match viewport {
        Some(view) => view.l.max(left.saturating_sub(BAR_GAP)),
        None => left.saturating_sub(BAR_GAP),
    };
    let bar = Rect { l: bar_x, t: top, r: bar_x.saturating_add(BAR_WIDTH), b: bottom };
    let mut fx = bar_x.saturating_add(FLAG_SHIFT).saturating_sub(size.div_euclid(2));
    let mut fy = top.saturating_sub(size).saturating_add(FLAG_LIFT);
    if let Some(view) = viewport {
        fx = fx.max(view.l);
        fy = fy.max(view.t);
    }
    (bar, Rect { l: fx, t: fy, r: fx.saturating_add(size), b: fy.saturating_add(size) })
}

/// Место плашки «▲/▼ Закладка» у правого края области просмотра.
fn pill_rect(dir: Dir, view: Rect, (width, height): (i32, i32)) -> Rect {
    let x = view.r.saturating_sub(width).saturating_sub(PILL_RIGHT);
    let y = match dir {
        Dir::Up => view.t.saturating_add(PILL_UP_TOP),
        Dir::Down => view.b.saturating_sub(height).saturating_sub(PILL_DOWN_BOTTOM),
    };
    let y = y.max(view.t.saturating_add(PILL_MIN_TOP));
    Rect { l: x, t: y, r: x.saturating_add(width), b: y.saturating_add(height) }
}
