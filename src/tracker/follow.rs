//! Слежение (часть 4; P 2000–2328): follow_screen, refresh_scopes, expected, locate, apply_scan,
//! follow, layout, names_unchanged, in_band, check_cover, read_rects, viewport, render.
//! Здесь — закрытые (`pub(crate)` или приватные) методы `impl Worker`; публичные — в mod.rs.
//! Открыты только чистые `tick_wait`/`wait_left` и `apply_scan` — для tests\tracker.rs.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;

use crate::core::flatten::FlatDoc;
use crate::core::geometry::{center, direction, grow, intersect, merge_lines, shift, size_of, thumb_moved};
use crate::core::matching::{best_occurrence, context_score, fuzzy_best};
use crate::core::rules::{page_key, self_heal, ScopeKeyHash};
use crate::store::{log_error, log_timing};
use crate::tracker::locator::Request;
use crate::tracker::state::{FlatEntry, Item, Scope, Track, Worker};
use crate::tracker::MERGE_GAP;
use crate::types::{Dir, Kind, Rect, RenderState, UiEvent};
use crate::uia::{ScopeKey, Segment, TextPattern};
use crate::win::monotonic;

/// Шаг при движении текста (< 0,5 с) или вводе пользователя (< 0,3 с).
pub const STEP_FAST: f64 = 0.016;
/// Метка видна, всё спокойно.
pub const STEP_SHOWN: f64 = 0.1;
/// Ничего не видно.
pub const STEP_REST: f64 = 0.25;
/// Пауза, нет закладок для окна, окно свёрнуто.
pub const STEP_IDLE: f64 = 0.3;
/// Идёт мигание или ждёт мигания.
pub const STEP_FLASH: f64 = 0.05;
/// OCR-закладки: 10 проверок в секунду сразу после ввода, иначе одна.
pub const STEP_SCREEN_ACTIVE: f64 = 0.1;
pub const STEP_SCREEN_IDLE: f64 = 1.0;
/// «Движется» — последний сдвиг текста моложе этого.
const MOVING_S: f64 = 0.5;
/// «Пользователь работает» — последний ввод моложе этого.
const INPUT_S: f64 = 0.3;
/// Для OCR «недавний ввод».
const SCREEN_INPUT_S: f64 = 2.0;
/// «Спокойно» — столько без движения.
const CALM_S: f64 = 0.15;
/// Проверка перекрытия; под точкой наше окно — повторить раньше.
const COVER_EVERY_S: f64 = 1.0;
const COVER_OWN_S: f64 = 0.3;
/// Ответ hit test Chromium готов не раньше чем через столько после `prime`.
const PROBE_SETTLE_S: f64 = 0.05;
/// Проверка, что текст элементов не изменился.
const NAMES_EVERY_S: f64 = 1.0;
/// Пересканирование документа: при работе пользователя / мало сегментов, для нового, потолок.
const RESCAN_ACTIVE_S: f64 = 0.25;
const RESCAN_NEW_S: f64 = 2.0;
const RESCAN_MAX_S: f64 = 5.0;
const RESCAN_GROWTH: f64 = 1.5;
const RESCAN_COST_SHARE: f64 = 3.0;
const RESCAN_IDLE_S: f64 = 1.0;
const RESCAN_FEW_SEGMENTS: usize = 5;
/// Порог доверия контексту (Python `min_score` по умолчанию).
const MIN_SCORE: f64 = 0.6;
/// Миниатюра для OCR: размер, порог и число изменившихся точек, спокойных кадров до чтения.
const THUMB: (i32, i32) = (96, 60);
const THUMB_THRESHOLD: u8 = 6;
const THUMB_PIXELS: usize = 20;
const OCR_CALM_FRAMES: u32 = 2;
/// Страховочное перечитывание; после ошибки — раньше.
const OCR_REREAD_S: f64 = 10.0;
const OCR_RETRY_S: f64 = 3.0;
/// Мигание при появлении.
const FLASH_S: f64 = 0.75;
/// Предок элемента для `lineage` (Python по умолчанию).
const LINEAGE_LIMIT: usize = 40;
/// Глубина цепочки hit test.
const HIT_DEPTH: usize = 3;

/// Шаг ожидания после обычного тика (P 2047–2056).
/// `tracks` — (`last_move`, метка видна) каждой найденной закладки окна.
pub fn tick_wait(now: f64, idle: f64, tracks: &[(f64, bool)], screen_marks: bool, flashing: bool) -> f64 {
    let mut wait = STEP_REST;
    if !tracks.is_empty() {
        let moving = tracks.iter().any(|&(last_move, _)| now - last_move < MOVING_S);
        let showing = tracks.iter().any(|&(_, shown)| shown);
        let step = if moving || idle < INPUT_S {
            STEP_FAST
        } else if showing {
            STEP_SHOWN
        } else {
            STEP_REST
        };
        wait = wait.min(step);
    }
    if screen_marks {
        wait = wait.min(if idle < SCREEN_INPUT_S { STEP_SCREEN_ACTIVE } else { STEP_SCREEN_IDLE });
    }
    if flashing {
        wait = wait.min(STEP_FLASH);
    }
    wait
}

/// Сколько ещё ждать до следующего тика (шаг считается от конца прошлого тика, как в Python).
pub fn wait_left(last_tick: f64, wait: f64, now: f64) -> f64 {
    (last_tick + wait - now).max(0.0)
}

/// Хеш ключа документа для `self_heal` (одинаков в пределах процесса).
fn scope_hash(key: &ScopeKey) -> ScopeKeyHash {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Может ли закладка быть в документе с этой страницей? (Страница неизвестна — может.)
fn expected(item: &Item, page: &str) -> bool {
    item.b.page.is_empty() || page.is_empty() || item.b.page == page
}

impl Worker {
    /// Метки по тексту экрана: прячутся, пока картинка меняется; когда она успокоилась — OCR.
    pub(crate) fn follow_screen(&mut self, marks: &[String], now: f64) {
        if marks.is_empty() {
            self.ocr_visible.clear();
            return;
        }
        let Some(window) = self.window else {
            self.ocr_visible.clear();
            return;
        };
        // Снимок ждёт кадр DWM (~16,7 мс) — он идёт только при OCR-закладках в активном окне.
        let Some(thumb) = self.api.capture(window, Some(THUMB)) else {
            log_error(&self.shared, "OCR_THUMB");
            self.ocr_visible.clear();
            return;
        };
        let moving = match &self.ocr_thumb {
            None => true,
            Some(previous) => {
                self.window != self.ocr_window || thumb_moved(&thumb.bgra, previous, THUMB_THRESHOLD, THUMB_PIXELS)
            }
        };
        self.ocr_thumb = Some(thumb.bgra);
        self.ocr_window = self.window;
        if moving {
            self.ocr_stable = 0;
            self.ocr_dirty = true;
            self.ocr_visible.clear();
            return;
        }
        self.ocr_stable = self.ocr_stable.saturating_add(1);
        let mut visible = std::mem::take(&mut self.ocr_visible);
        for lines in visible.values_mut() {
            lines.retain(|r| self.uncovered(*r));
        }
        visible.retain(|_, lines| !lines.is_empty());
        self.ocr_visible = visible;
        if self.ocr_stable < OCR_CALM_FRAMES || !(self.ocr_dirty || now >= self.ocr_next) {
            return;
        }
        let started = monotonic();
        let hint: Vec<&str> = marks.iter().filter_map(|id| self.item_by_id(id)).map(|item| item.b.quote.as_str()).collect();
        let hint = hint.join(" ");
        let read = self.screen_words(self.target, &hint);
        let after = read.as_ref().and_then(|_| self.api.capture(window, Some(THUMB)));
        let (Some((flat, rects)), Some(after)) = (read, after) else {
            log_error(&self.shared, "OCR");
            self.ocr_next = now + OCR_RETRY_S;
            self.ocr_visible.clear();
            return;
        };
        let changed = match &self.ocr_thumb {
            Some(before) => thumb_moved(&after.bgra, before, THUMB_THRESHOLD, THUMB_PIXELS),
            None => true,
        };
        if changed || self.api.window_rect(self.target) != self.window {
            self.ocr_dirty = true; // картинка изменилась, пока её читали: прочитать снова
            return;
        }
        let mut found = std::collections::HashMap::new();
        for id in marks {
            let Some(item) = self.item_by_id(id) else { continue };
            let Some(start) = fuzzy_best(&flat.text, &item.quote, &item.prefix, &item.suffix, MIN_SCORE) else {
                continue;
            };
            let lines: Vec<Rect> = self
                .screen_lines(&flat, &rects, start, item.quote.len())
                .into_iter()
                .filter_map(|line| intersect(Some(line), self.window))
                .filter(|r| self.uncovered(*r))
                .collect();
            if !lines.is_empty() {
                found.insert(id.clone(), lines);
            }
        }
        self.ocr_visible = found;
        self.ocr_dirty = false;
        self.ocr_next = monotonic() + OCR_REREAD_S; // изменения ловят миниатюры
        log_timing(&self.shared, "OCR", (monotonic() - started) * 1000.0);
    }

    /// Документы, показанные сейчас в окне; слежение за документами, которых больше нет, — прочь.
    pub(crate) fn refresh_scopes(&mut self) {
        let found = self.uia.shown_scopes(self.target, self.pid, self.window);
        let old = std::mem::take(&mut self.scopes);
        let mut scopes = Vec::with_capacity(found.len());
        for (key, element) in found {
            let pattern = match old.iter().find(|s| s.key == key) {
                Some(known) => known.pattern.clone(),
                None => self.uia.text_pattern(&element),
            };
            let rect = self.uia.current_rect(&element);
            let (url, name) = self.uia.document_info(&element);
            scopes.push(Scope { key, element, pattern, rect, page: page_key(&url, &name) });
        }
        self.scopes = scopes;
        let scopes = &self.scopes;
        self.tracks.retain(|_, track| scopes.iter().any(|s| s.key == track.scope_key));
        self.flats.retain(|key, _| scopes.iter().any(|s| &s.key == key));
    }

    /// Применить готовые сканирования и заказать новые у locator (слежение не ждёт сканирования).
    pub(crate) fn locate(&mut self, marks: &[String], now: f64) {
        while let Ok(result) = self.locator.results.try_recv() {
            self.scanning.remove(&result.key);
            let Some(scope) = self.scopes.iter().find(|s| s.key == result.key) else { continue };
            let pattern = scope.pattern.clone();
            self.apply_scan(&result.key, pattern, result.segs, result.cost_s, marks);
        }
        let mut wanted = Vec::new();
        for scope in &self.scopes {
            if self.scanning.contains(&scope.key) {
                continue;
            }
            let due = match self.flats.get(&scope.key) {
                None => true,
                Some(entry) => {
                    entry.dirty
                        || (now >= entry.next
                            && marks.iter().any(|id| {
                                !self.tracks.contains_key(id)
                                    && self.item_by_id(id).is_some_and(|item| expected(item, &scope.page))
                            }))
                }
            };
            if due {
                wanted.push((scope.key.clone(), scope.element.clone()));
            }
        }
        for (key, element) in wanted {
            if self.locator.requests.send(Request::Scan { key: key.clone(), element }).is_ok() {
                self.scanning.insert(key);
            }
        }
    }

    /// Результат сканирования документа `key`: найти в нём закладки `marks` (id), обновить их
    /// слежение, назначить следующее сканирование и убрать устаревшие позиции (`self_heal`).
    /// Открыт для tests\tracker.rs; в программе зовётся только из `locate`.
    pub fn apply_scan(&mut self, key: &ScopeKey, pattern: Option<TextPattern>, segs: Vec<Segment>, cost: f64, marks: &[String]) {
        log_timing(&self.shared, "FLATTEN", cost * 1000.0);
        let previous = self.flats.get(key).map(|entry| (entry.dirty, entry.delay));
        let names: Vec<&str> = segs.iter().map(|s| s.name.as_str()).collect();
        let flat = FlatDoc::new(&names);
        let mut found_new = false;
        for id in marks {
            let Some(item) = self.items.iter().find(|item| &item.b.id == id) else { continue };
            let Some(start) = best_occurrence(&flat.text, &item.quote, &item.prefix, &item.suffix, MIN_SCORE) else {
                if self.tracks.get(id).is_some_and(|old| &old.scope_key == key) {
                    self.tracks.remove(id);
                }
                continue;
            };
            let strong = context_score(&flat.text, start, start + item.quote.len(), &item.prefix, &item.suffix) >= MIN_SCORE;
            let mut track = self.make_track(item, key, pattern.clone(), &flat, &segs, start);
            track.strong = strong;
            match self.tracks.get(id).map(|old| old.signature_eq(&track)) {
                Some(true) => {
                    if let Some(old) = self.tracks.get_mut(id) {
                        old.strong = strong;
                    }
                }
                other => {
                    found_new |= other.is_none();
                    self.tracks.insert(id.clone(), track);
                }
            }
        }
        // Пока пользователь работает — смотреть снова скоро (прокрученный список может принести
        // текст); в покое — замедляться до 5 с. Сканированию — не больше трети времени.
        let delay = if self.api.idle_seconds() < RESCAN_IDLE_S || segs.len() < RESCAN_FEW_SEGMENTS {
            RESCAN_ACTIVE_S
        } else {
            match previous {
                Some((false, delay)) if !found_new => (delay * RESCAN_GROWTH).max(RESCAN_NEW_S).min(RESCAN_MAX_S),
                _ => RESCAN_NEW_S,
            }
        };
        let delay = delay.max(RESCAN_COST_SHARE * cost);
        self.flats.insert(key.clone(), FlatEntry { flat, segs, next: monotonic() + delay, dirty: false, delay });
        // Порядок — как у списка закладок (в Python — порядок словаря слежения).
        let located: Vec<(String, Kind, String, ScopeKeyHash)> = self
            .items
            .iter()
            .filter_map(|item| self.tracks.get(&item.b.id))
            .filter(|track| track.strong)
            .map(|track| (track.id.clone(), track.kind, track.created.clone(), scope_hash(&track.scope_key)))
            .collect();
        let stale = self_heal(&located);
        if !stale.is_empty() {
            self.delete(&stale, false);
        }
    }

    /// Каждый тик: где сейчас элементы цитаты; строки едут вместе с ними.
    /// Тот же размер, новое место (прокрутка) — сдвиг строк; новый размер (перенос) — пересчёт.
    pub(crate) fn follow(&mut self, now: f64) {
        let ids: Vec<String> = self.tracks.keys().cloned().collect();
        for id in ids {
            let Some(mut track) = self.tracks.remove(&id) else { continue };
            if self.follow_one(&mut track, now) {
                self.tracks.insert(id, track);
            }
        }
    }

    /// `false` — элемент исчез: слежение снять (документ уже помечен для пересканирования).
    fn follow_one(&mut self, track: &mut Track, now: f64) -> bool {
        let Some(rects) = self.read_rects(track) else { return false };
        match track.el_rects.take() {
            Some(old) if !track.lines.is_empty() => {
                if old != rects {
                    track.last_move = now;
                    track.probe = None;
                    let same_size = old.iter().zip(&rects).all(|(a, b)| size_of(*a) == size_of(*b));
                    let first = old.first().copied().zip(rects.first().copied());
                    track.el_rects = Some(rects);
                    match first {
                        Some((before, after)) if same_size => {
                            let (dx, dy) = (after.l.saturating_sub(before.l), after.t.saturating_sub(before.t));
                            for line in track.lines.iter_mut() {
                                *line = shift(*line, dx, dy);
                            }
                        }
                        _ => self.layout(track),
                    }
                } else {
                    track.el_rects = Some(old);
                }
            }
            _ => {
                track.el_rects = Some(rects);
                track.last_move = now;
                self.layout(track);
            }
        }
        if now >= track.next_names {
            track.next_names = now + NAMES_EVERY_S;
            if !self.names_unchanged(track) {
                // текст правили или он ещё пишется: пересканировать
                if let Some(entry) = self.flats.get_mut(&track.scope_key) {
                    entry.dirty = true;
                }
                track.visible.clear();
                return true;
            }
        }
        let viewport = self.viewport(track);
        let inside: Vec<Rect> = track.lines.iter().filter_map(|line| intersect(Some(*line), viewport)).collect();
        let calm = now - track.last_move > CALM_S;
        if inside.is_empty() {
            track.visible.clear();
            if calm {
                track.direction = if matches!(track.scope_key, ScopeKey::Root(_)) {
                    None
                } else if !track.lines.is_empty() {
                    direction(&track.lines, viewport)
                } else {
                    direction(track.el_rects.as_deref().unwrap_or(&[]), viewport)
                };
            }
            return true;
        }
        track.direction = None;
        if calm && now >= track.next_check {
            self.check_cover(track, &inside, now);
        }
        track.visible = inside.into_iter().filter(|r| !self.in_band(*r) && self.uncovered(*r)).collect();
        track.stable = if calm { 2 } else { 0 };
        true
    }

    /// Прямоугольники строк цитаты для текущих прямоугольников элементов (5–10 мс).
    fn layout(&self, track: &mut Track) {
        let mut lines = Vec::new();
        if let Some(el_rects) = &track.el_rects {
            for (seg, el_rect) in track.segs.iter().zip(el_rects) {
                let mut pieces = self.uia.piece_rects(track.pattern.as_ref(), &seg.element, &seg.name, seg.first, seg.last);
                let whole = seg.first == 0 && seg.last >= seg.name.chars().count();
                if pieces.is_empty() && (track.pattern.is_none() || whole) {
                    pieces = vec![*el_rect]; // цитата — весь элемент, или TextPattern нет вовсе
                }
                let bound = Some(grow(*el_rect, 2));
                lines.extend(pieces.into_iter().filter_map(|piece| intersect(Some(piece), bound)));
            }
        }
        track.lines = merge_lines(&lines, MERGE_GAP);
    }

    fn names_unchanged(&self, track: &Track) -> bool {
        track.segs.iter().all(|seg| self.uia.current_name(&seg.element).as_deref() == Some(seg.name.as_str()))
    }

    /// Середина строки в выученной полосе, где программа рисует поверх своего текста.
    fn in_band(&self, r: Rect) -> bool {
        let middle = (f64::from(r.t) + f64::from(r.b)) / 2.0;
        self.bands.iter().any(|&(top, bottom)| f64::from(top) - 2.0 <= middle && middle <= f64::from(bottom) + 2.0)
    }

    /// Перекрытие собственным интерфейсом программы (поле ввода, заголовок): одна строка за раз,
    /// для Chromium в два шага (подготовить hit test, прочитать ответ на следующем тике, без сна).
    /// Перекрытая строка учит «полосу» окна, где метки не рисуются.
    fn check_cover(&mut self, track: &mut Track, inside: &[Rect], now: f64) {
        let rect = inside[track.sample % inside.len()];
        let point = center(rect);
        if self.api.own_window_at(point) {
            track.next_check = now + COVER_OWN_S;
            return;
        }
        if self.target_chromium && track.probe.map_or(true, |(probed, _)| probed != point) {
            self.uia.prime(point);
            track.probe = Some((point, now));
            return;
        }
        if let Some((_, primed)) = track.probe {
            if now - primed < PROBE_SETTLE_S {
                return;
            }
        }
        track.probe = None;
        track.sample = track.sample.wrapping_add(1);
        track.next_check = now + COVER_EVERY_S;
        if track.allowed.is_none() {
            let (mut allowed, mut outer) = (HashSet::new(), HashSet::new());
            for seg in &track.segs {
                let (inner, above) = self.uia.lineage(&seg.element, LINEAGE_LIMIT);
                allowed.extend(inner);
                outer.extend(above);
            }
            track.allowed = Some(allowed);
            track.outer = outer;
        }
        let chain = self.uia.hit_chain(point, HIT_DEPTH, 0.0);
        let covered = match chain.first() {
            None => false,
            Some(first) => {
                let own = chain.iter().any(|key| track.segs.iter().any(|seg| seg.rid.as_ref() == Some(key)));
                let allowed = track.allowed.as_ref().is_some_and(|set| set.contains(first));
                !(own || allowed || track.outer.contains(first))
            }
        };
        let band = (rect.t, rect.b);
        let overlapping: Vec<(i32, i32)> =
            self.bands.iter().copied().filter(|b| b.0 <= band.1 && band.0 <= b.1).collect();
        if covered && overlapping.is_empty() {
            self.bands.push(band);
        } else if !covered && !overlapping.is_empty() {
            self.bands.retain(|b| !overlapping.contains(b));
        }
        log_timing(&self.shared, if covered { "CHECK_COVERED" } else { "CHECK_SHOWN" }, inside.len() as f64);
    }

    /// Текущие прямоугольники элементов; `None` (и пересканирование), когда элемента больше нет.
    fn read_rects(&mut self, track: &Track) -> Option<Vec<Rect>> {
        let mut rects = Vec::with_capacity(track.segs.len());
        for seg in &track.segs {
            match self.uia.current_rect(&seg.element) {
                Some(r) => rects.push(r),
                None => {
                    if let Some(entry) = self.flats.get_mut(&track.scope_key) {
                        entry.dirty = true;
                    }
                    return None;
                }
            }
        }
        Some(rects)
    }

    /// Видимая часть документа закладки (документ ∩ окно), иначе окно.
    pub(crate) fn viewport(&self, track: &Track) -> Option<Rect> {
        match self.scopes.iter().find(|s| s.key == track.scope_key) {
            Some(scope) if scope.rect.is_some() => intersect(scope.rect, self.window),
            _ => self.window,
        }
    }

    /// Поставить метки (`Overlay::apply`) и, при debug, сообщить UI новое состояние.
    pub(crate) fn render(&mut self, empty: bool, now: f64) {
        let mut marks: Vec<(String, Vec<Rect>)> = Vec::new();
        let mut pills: Vec<(Dir, String)> = Vec::new();
        let mut viewport: Option<Rect> = None;
        if !empty {
            let mut order: Vec<&Track> = self.tracks.values().collect();
            // самые новые первыми (у равных — по id, чтобы порядок не прыгал между тиками)
            order.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
            for track in order {
                if !track.visible.is_empty() {
                    marks.push((track.id.clone(), track.visible.clone()));
                    if viewport.is_none() {
                        viewport = self.viewport(track);
                    }
                } else if let Some(dir) = track.direction {
                    if track.stable >= 2 && !pills.iter().any(|(known, _)| *known == dir) {
                        pills.push((dir, track.id.clone()));
                        if viewport.is_none() {
                            viewport = self.viewport(track);
                        }
                    }
                }
            }
            for item in &self.items {
                if let Some(lines) = self.ocr_visible.get(&item.b.id) {
                    marks.push((item.b.id.clone(), lines.clone()));
                    if viewport.is_none() {
                        viewport = self.window;
                    }
                }
            }
        }
        for (id, _) in &marks {
            if self.pending_flash.remove(id).is_some() {
                self.overlay.flash(id, now + FLASH_S);
            }
        }
        self.pending_flash.retain(|_, until| *until > now);
        self.overlay.apply(&marks, &pills, viewport, now);
        // как sorted(pills.items()) в Python: "down" < "up"
        pills.sort_by_key(|(dir, _)| match dir {
            Dir::Down => 0,
            Dir::Up => 1,
        });
        let state = RenderState { marks, pills, viewport };
        if self.last_render.as_ref() != Some(&state) {
            if self.shared.debug.load(Ordering::Relaxed) {
                self.events.send(UiEvent::Render(state.clone()));
            }
            self.last_render = Some(state);
        }
    }
}
