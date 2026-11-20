//! Прямоугольники и точки (P 260–315).

use crate::types::{Dir, Point, Rect};

/// `round()` Python для float -> int: половина — к чётному.
fn py_round(v: f64) -> i32 {
    v.round_ties_even() as i32
}

/// Двойки UIA `GetBoundingRectangles` (x, y, w, h)* -> [(l, t, r, b)] без тонких полосок.
pub fn to_rects(values: &[f64]) -> Vec<Rect> {
    values
        .chunks_exact(4)
        .filter(|v| v[2] > 2.0 && v[3] > 2.0)
        .map(|v| Rect { l: py_round(v[0]), t: py_round(v[1]), r: py_round(v[0] + v[2]), b: py_round(v[1] + v[3]) })
        .collect()
}

pub fn intersect(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    let (a, b) = (a?, b?);
    let (l, t) = (a.l.max(b.l), a.t.max(b.t));
    let (r, bottom) = (a.r.min(b.r), a.b.min(b.b));
    if r > l && bottom > t {
        Some(Rect { l, t, r, b: bottom })
    } else {
        None
    }
}

pub fn area(r: Option<Rect>) -> i64 {
    match r {
        Some(r) => (r.r as i64 - r.l as i64).max(0) * (r.b as i64 - r.t as i64).max(0),
        None => 0,
    }
}

pub fn overlap_area(a: Rect, b: Rect) -> i64 {
    area(intersect(Some(a), Some(b)))
}

pub fn size_of(r: Rect) -> (i32, i32) {
    (r.r.saturating_sub(r.l), r.b.saturating_sub(r.t))
}

pub fn shift(r: Rect, dx: i32, dy: i32) -> Rect {
    Rect { l: r.l.saturating_add(dx), t: r.t.saturating_add(dy), r: r.r.saturating_add(dx), b: r.b.saturating_add(dy) }
}

pub fn grow(r: Rect, px: i32) -> Rect {
    Rect { l: r.l.saturating_sub(px), t: r.t.saturating_sub(px), r: r.r.saturating_add(px), b: r.b.saturating_add(px) }
}

/// Целочисленная середина с округлением вниз (`//` Python).
pub fn center(r: Rect) -> Point {
    Point {
        x: ((r.l as i64 + r.r as i64).div_euclid(2)) as i32,
        y: ((r.t as i64 + r.b as i64).div_euclid(2)) as i32,
    }
}

pub fn distance(p: Point, r: Rect) -> f64 {
    let dx = (r.l as i64 - p.x as i64).max(0).max(p.x as i64 - r.r as i64);
    let dy = (r.t as i64 - p.y as i64).max(0).max(p.y as i64 - r.b as i64);
    ((dx * dx + dy * dy) as f64).sqrt()
}

/// Соединить прямоугольники одной строки; результат — в порядке чтения.
pub fn merge_lines(rects: &[Rect], gap: i32) -> Vec<Rect> {
    let mut sorted = rects.to_vec();
    sorted.sort_by_key(|r| (r.l, r.t)); // слева направо: соседи присоединяются по очереди
    let mut merged: Vec<Rect> = Vec::new();
    for rect in sorted {
        let mut joined = false;
        for line in merged.iter_mut() {
            let shared = line.b.min(rect.b) as i64 - line.t.max(rect.t) as i64;
            let shortest = (line.b as i64 - line.t as i64).min(rect.b as i64 - rect.t as i64);
            if shared as f64 >= 0.6 * shortest as f64
                && rect.l as i64 <= line.r as i64 + gap as i64
                && rect.r as i64 >= line.l as i64 - gap as i64
            {
                *line = Rect { l: line.l.min(rect.l), t: line.t.min(rect.t), r: line.r.max(rect.r), b: line.b.max(rect.b) };
                joined = true;
                break;
            }
        }
        if !joined {
            merged.push(rect);
        }
    }
    merged.sort_by_key(|r| (r.t, r.l));
    merged
}

/// `Up`/`Down`, когда все прямоугольники выше/ниже области просмотра.
pub fn direction(rects: &[Rect], viewport: Option<Rect>) -> Option<Dir> {
    let viewport = viewport?;
    let bottom = rects.iter().map(|r| r.b).max()?;
    if bottom <= viewport.t {
        return Some(Dir::Up);
    }
    let top = rects.iter().map(|r| r.t).min()?;
    if top >= viewport.b {
        return Some(Dir::Down);
    }
    None
}

/// Истина, когда у двух одинаковых BGRA-миниатюр отличается достаточно точек (зелёный канал).
pub fn thumb_moved(first: &[u8], second: &[u8], threshold: u8, pixels: usize) -> bool {
    if first.is_empty() || second.is_empty() || first.len() != second.len() {
        return true;
    }
    let mut changed = 0usize;
    for (x, y) in first.iter().skip(1).step_by(4).zip(second.iter().skip(1).step_by(4)) {
        if x.abs_diff(*y) > threshold {
            changed += 1;
            if changed >= pixels {
                return true;
            }
        }
    }
    false
}
