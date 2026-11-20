//! Поиск цитаты в плоском тексте (P 120–260). Все смещения — в `char`.

use std::collections::HashMap;

use crate::core::flatten::{to_flat, SHORT_QUOTE};
use crate::types::Bookmark;

/// Срез `text[start:end]` с правилами Python: границы обрезаются по длине, пустой при start >= end.
fn py_slice(text: &[char], start: usize, end: usize) -> &[char] {
    let end = end.min(text.len());
    if start >= end {
        &[]
    } else {
        &text[start..end]
    }
}

/// Как `str.find(needle, from)` Python: пустая строка находится в `from`, если `from <= len`.
pub fn find(text: &[char], needle: &[char], from: usize) -> Option<usize> {
    let n = text.len();
    let m = needle.len();
    if from > n {
        return None;
    }
    if m == 0 {
        return Some(from);
    }
    if m > n - from {
        return None;
    }
    let first = needle[0];
    let last_start = n - m;
    let mut i = from;
    while i <= last_start {
        match text[i..=last_start].iter().position(|&c| c == first) {
            None => return None,
            Some(p) => i += p,
        }
        if text[i..i + m] == *needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Начала вхождений (с перекрытием), не больше `limit`; пустая цитата — пусто.
pub fn occurrences(text: &[char], quote: &[char], limit: usize) -> Vec<usize> {
    let mut found = Vec::new();
    if quote.is_empty() {
        return found;
    }
    let mut start = find(text, quote, 0);
    while let Some(at) = start {
        if found.len() >= limit {
            break;
        }
        found.push(at);
        start = find(text, quote, at + 1);
    }
    found
}

/// Самый длинный общий блок в `a[alo..ahi]` и `b[blo..bhi]` — `find_longest_match` difflib без
/// «мусора»: при первом встреченном максимуме (наименьшие i, затем j). Расширения по краям в
/// difflib при пустом `isjunk` и `autojunk=False` ничего не меняют: DP уже даёт полные блоки.
struct Longest<'a> {
    a: &'a [char],
    b2j: HashMap<char, Vec<usize>>,
    row: Vec<u32>,
    next_row: Vec<u32>,
    touched: Vec<usize>,
    next_touched: Vec<usize>,
}

impl<'a> Longest<'a> {
    fn new(a: &'a [char], b: &[char]) -> Longest<'a> {
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (j, &c) in b.iter().enumerate() {
            b2j.entry(c).or_default().push(j);
        }
        Longest {
            a,
            b2j,
            row: vec![0; b.len() + 1],
            next_row: vec![0; b.len() + 1],
            touched: Vec::new(),
            next_touched: Vec::new(),
        }
    }

    /// (i, j, k). `row[j + 1]` — длина общего блока, кончающегося на `a[i - 1]`, `b[j]`.
    fn find(&mut self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        for i in alo..ahi {
            if let Some(js) = self.b2j.get(&self.a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = self.row[j] as usize + 1;
                    self.next_row[j + 1] = k as u32;
                    self.next_touched.push(j + 1);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            for &t in &self.touched {
                self.row[t] = 0;
            }
            self.touched.clear();
            std::mem::swap(&mut self.row, &mut self.next_row);
            std::mem::swap(&mut self.touched, &mut self.next_touched);
        }
        for &t in &self.touched {
            self.row[t] = 0;
        }
        self.touched.clear();
        (besti, bestj, bestsize)
    }
}

/// Точный порт `difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()`.
pub fn similarity(left: &[char], right: &[char]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let mut longest = Longest::new(left, right);
    let mut matches = 0usize;
    let mut queue = vec![(0, left.len(), 0, right.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest.find(alo, ahi, blo, bhi);
        if k > 0 {
            matches += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    2.0 * matches as f64 / (left.len() + right.len()) as f64
}

pub fn context_score(text: &[char], start: usize, end: usize, prefix: &[char], suffix: &[char]) -> f64 {
    let mut total = 0.0;
    let mut count = 0usize;
    if !prefix.is_empty() {
        total += similarity(py_slice(text, start.saturating_sub(prefix.len()), start), prefix);
        count += 1;
    }
    if !suffix.is_empty() {
        total += similarity(py_slice(text, end, end.saturating_add(suffix.len())), suffix);
        count += 1;
    }
    if count == 0 {
        1.0
    } else {
        total / count as f64
    }
}

pub fn best_occurrence(text: &[char], quote: &[char], prefix: &[char], suffix: &[char], min_score: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for start in occurrences(text, quote, 1000) {
        let score = context_score(text, start, start + quote.len(), prefix, suffix);
        if best.map_or(true, |(_, top)| score > top) {
            best = Some((start, score));
        }
    }
    match best {
        Some((start, score)) if !(quote.len() < SHORT_QUOTE && score < min_score) => Some(start),
        _ => None,
    }
}

/// Множество целых Python (`set()` из CPython 3.13): тот же порядок обхода, что у
/// `for x in s`, — от него зависит выбор среди равных `ratio` в `fuzzy_candidates`.
/// Хеш неотрицательного int — само число; открытая адресация: 9 линейных проб, затем
/// `i = (i * 5 + 1 + perturb) & mask`, `perturb >>= 5`; рост при `fill * 5 >= mask * 3`
/// до наименьшей степени двойки > `used * 4`.
pub(crate) struct PyIntSet {
    table: Vec<Option<usize>>,
    mask: usize,
    used: usize,
}

const LINEAR_PROBES: usize = 9;
const PERTURB_SHIFT: u32 = 5;

impl PyIntSet {
    pub(crate) fn new() -> PyIntSet {
        PyIntSet { table: vec![None; 8], mask: 7, used: 0 }
    }

    pub(crate) fn add(&mut self, value: usize) {
        let mask = self.mask;
        let mut i = value & mask;
        let mut perturb = value;
        loop {
            let probes = if i + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
            for slot in i..=i + probes {
                match self.table[slot] {
                    None => {
                        self.table[slot] = Some(value);
                        self.used += 1;
                        // Удалений нет, поэтому fill == used.
                        if self.used * 5 >= mask * 3 {
                            let minused = if self.used > 50000 { self.used * 2 } else { self.used * 4 };
                            self.resize(minused);
                        }
                        return;
                    }
                    Some(present) if present == value => return,
                    Some(_) => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
        }
    }

    fn resize(&mut self, minused: usize) {
        let mut size = 8usize;
        while size <= minused {
            size <<= 1;
        }
        let old = std::mem::replace(&mut self.table, vec![None; size]);
        self.mask = size - 1;
        for value in old.into_iter().flatten() {
            self.insert_clean(value);
        }
    }

    fn insert_clean(&mut self, value: usize) {
        let mask = self.mask;
        let mut i = value & mask;
        let mut perturb = value;
        loop {
            if self.table[i].is_none() {
                self.table[i] = Some(value);
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for slot in i + 1..=i + LINEAR_PROBES {
                    if self.table[slot].is_none() {
                        self.table[slot] = Some(value);
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
        }
    }

    /// Порядок `for x in s`.
    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.table.iter().filter_map(|v| *v)
    }
}

/// [(start, ratio)] мест, похожих на цитату (ошибки OCR допускаются).
pub fn fuzzy_candidates(text: &[char], quote: &[char], min_ratio: f64, gram: usize, limit: usize) -> Vec<(usize, f64)> {
    let size = quote.len();
    if size == 0 || text.is_empty() {
        return Vec::new();
    }
    if size <= gram {
        return occurrences(text, quote, limit).into_iter().map(|start| (start, 1.0)).collect();
    }
    let mut offsets = vec![0, size / 4, size / 2, (3 * size) / 4, size - gram];
    offsets.sort_unstable();
    offsets.dedup();
    let mut starts = PyIntSet::new();
    for offset in offsets {
        for position in occurrences(text, py_slice(quote, offset, offset + gram), limit) {
            starts.add(position.saturating_sub(offset));
        }
    }
    let mut scored: Vec<(usize, f64)> = starts
        .iter()
        .map(|start| (start, similarity(py_slice(text, start, start + size), quote)))
        .collect();
    // sorted(key=-ratio): устойчивая сортировка по убыванию.
    scored.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut picked: Vec<(usize, f64)> = Vec::new();
    for (start, ratio) in scored {
        if ratio >= min_ratio && picked.iter().all(|&(other, _)| start.abs_diff(other) > 3) {
            picked.push((start, ratio));
        }
    }
    picked
}

/// Как `best_occurrence`, для текста OCR. Неточному совпадению нужен и контекст.
pub fn fuzzy_best(text: &[char], quote: &[char], prefix: &[char], suffix: &[char], min_score: f64) -> Option<usize> {
    // (start, ratio + fit, fit, ratio)
    let mut best: Option<(usize, f64, f64, f64)> = None;
    for (start, ratio) in fuzzy_candidates(text, quote, 0.75, 5, 30) {
        let fit = context_score(text, start, start + quote.len(), prefix, suffix);
        if best.map_or(true, |b| ratio + fit > b.1) {
            best = Some((start, ratio + fit, fit, ratio));
        }
    }
    let (start, _, fit, ratio) = best?;
    if fit < min_score && (quote.len() < SHORT_QUOTE || ratio < 0.95) {
        return None;
    }
    Some(start)
}

/// Сама цитата, или её самый длинный из проверенных кусков, который есть в тексте, или `None`.
pub fn find_quote(text: &[char], quote: &[char], min_len: usize) -> Option<Vec<char>> {
    if quote.is_empty() {
        return None;
    }
    if find(text, quote, 0).is_some() {
        return Some(quote.to_vec());
    }
    let total = quote.len();
    let mut size = total;
    while size > min_len {
        size = min_len.max(size * 3 / 4); // int(size * 0.75)
        for start in [0, total - size, (total - size) / 2] {
            let piece = py_slice(quote, start, start + size);
            if find(text, piece, 0).is_some() {
                return Some(piece.to_vec());
            }
        }
    }
    None
}

/// Найдена в тексте вместе со своим окружением (а не просто те же слова).
pub fn strong_match(text: &[char], item: &Bookmark, min_score: f64) -> bool {
    let quote = to_flat(&item.quote);
    let prefix = to_flat(&item.prefix);
    let suffix = to_flat(&item.suffix);
    match best_occurrence(text, &quote, &prefix, &suffix, 0.6) {
        Some(start) => context_score(text, start, start + quote.len(), &prefix, &suffix) >= min_score,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::PyIntSet;

    /// Порядок обхода `set()` CPython 3.13 на эталонах `tests\golden\pyset.json`.
    #[test]
    fn py_set_order_matches_cpython() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/pyset.json");
        let text = std::fs::read_to_string(path).expect("pyset.json");
        let cases: serde_json::Value = serde_json::from_str(&text).expect("pyset.json: JSON");
        let as_list = |v: &serde_json::Value| -> Vec<usize> {
            v.as_array().expect("массив").iter().map(|x| x.as_u64().expect("число") as usize).collect()
        };
        let cases = cases.as_array().expect("массив");
        for case in cases {
            let mut set = PyIntSet::new();
            for value in as_list(&case[0]) {
                set.add(value);
            }
            assert_eq!(set.iter().collect::<Vec<_>>(), as_list(&case[1]), "{}", case[0]);
        }
        assert!(cases.len() >= 100);
    }
}
