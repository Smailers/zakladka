//! UI Automation (часть 3; P 1131–1443). Ошибки UIA не паникуют: `None` / пусто / `false`.
//!
//! Хитрости Chromium/Electron (раздел 6 плана, часть 3), которые здесь нельзя потерять:
//! - `FindText` и `GetVisibleRanges` не используются: ложный пустой диапазон и ~2 с;
//! - `segments` — один `BuildUpdatedCache` всего поддерева (Name + BoundingRectangle,
//!   `TreeScope_Subtree`, `RawViewCondition`), обход стеком в порядке документа;
//! - `ElementFromPoint` в Chromium отвечает результатом прошлого асинхронного hit test:
//!   `prime(p)` на одном тике, чтение (`hit_chain(p, 3, 0.0)`) — не раньше чем через 50 мс;
//! - `SAFEARRAY` (`GetRuntimeId`, `GetBoundingRectangles`) читается только через `OwnedArray`,
//!   его `Drop` всегда вызывает `SafeArrayDestroy`;
//! - смещения в тексте — в кодовых точках (`char`), как в Python; UIA отдаёт UTF-16 (BSTR),
//!   поэтому текст диапазона сначала переводится в `char`.

use std::collections::HashSet;
use std::ffi::c_void;
use std::time::Duration;

use windows::core::{Interface, BSTR, VARIANT};
use windows::Win32::Foundation::{POINT, TRUE};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, SAFEARRAY};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElemsize, SafeArrayGetLBound,
    SafeArrayGetUBound, SafeArrayGetVartype, SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VARENUM, VT_I4, VT_R8};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest,
    IUIAutomationElement, IUIAutomationElementArray, IUIAutomationInvokePattern,
    IUIAutomationScrollItemPattern, IUIAutomationSelectionItemPattern, IUIAutomationTextPattern,
    IUIAutomationTextRange, IUIAutomationTreeWalker, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character, TextUnit_Line, TreeScope_Descendants, TreeScope_Subtree,
    UIA_BoundingRectanglePropertyId, UIA_CONTROLTYPE_ID, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_InvokePatternId, UIA_NamePropertyId, UIA_ScrollItemPatternId,
    UIA_SelectionItemPatternId, UIA_TabItemControlTypeId, UIA_TextPatternId, UIA_ValuePatternId,
};

use crate::core::flatten::grapheme_extra;
use crate::core::geometry::to_rects;
use crate::core::matching::find;
use crate::types::{to_hwnd, Hwnd, Point, Rect};

/// Таймауты CUIAutomation8 (соединение и транзакция): зависшая чужая программа держит вызов не дольше.
const TIMEOUT_MS: u32 = 2000;
/// Предел подъёма к Document (`ancestors` в Python, `limit=40`).
const ANCESTORS: usize = 40;
/// Предел подъёма от источника выделения.
const SELECTION_ANCESTORS: usize = 30;
/// `GetText` выделения, UTF-16 единиц (как в Python).
const SELECTION_TEXT: i32 = 4000;

/// Элемент UIA.
///
/// ИНВАРИАНТ: создаётся и используется только в потоках MTA (tracker, locator, selftest —
/// все после `CoInitializeEx(COINIT_MULTITHREADED)`); UI-поток UIA не трогает никогда.
/// Объекты UIA, полученные в MTA, свободнопоточные — поэтому их можно передавать между
/// потоками MTA. Поле `pub(crate)`, чтобы снаружи нельзя было обернуть элемент из STA.
#[derive(Clone)]
pub struct Element(pub(crate) IUIAutomationElement);

// SAFETY: см. инвариант выше — только между потоками MTA.
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

/// TextPattern элемента. Тот же инвариант, что у `Element`: только потоки MTA.
#[derive(Clone)]
pub struct TextPattern(pub(crate) IUIAutomationTextPattern);

// SAFETY: см. инвариант `Element` — только между потоками MTA.
unsafe impl Send for TextPattern {}
unsafe impl Sync for TextPattern {}

pub type RuntimeId = Box<[i32]>;

/// Root: без hit test, без ▲/▼.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ScopeKey {
    Doc(RuntimeId),
    Root(RuntimeId),
}

/// Лист документа с непустым именем.
#[derive(Clone)]
pub struct Segment {
    pub element: Element,
    pub name: String,
    pub rect: Rect,
}

#[derive(Clone)]
pub struct Selection {
    pub text: String,
    pub rects: Vec<Rect>,
    pub scope: Option<Element>,
    pub any_pattern: bool,
}

/// `Arc<Uia>`, Send + Sync (проверяется в lib.rs).
pub struct Uia {
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,          // RawViewWalker
    cache_type: IUIAutomationCacheRequest,    // ControlType — для подъёма к Document
    cache_tree: IUIAutomationCacheRequest,    // Name + BoundingRectangle, всё поддерево, Raw view
}

// SAFETY: тот же инвариант, что у `Element`: `Uia` создаётся и используется только в потоках MTA
// (tracker, locator, selftest — после `CoInitializeEx(COINIT_MULTITHREADED)`), UI-поток его не
// трогает. Объект CUIAutomation и полученные из него в MTA объекты свободнопоточные; кэш-запросы
// после `new` только читаются (передаются параметром), поэтому общий `Arc<Uia>` безопасен.
unsafe impl Send for Uia {}
unsafe impl Sync for Uia {}

/// `SAFEARRAY`, который вернул UIA (им владеет вызывающий). Единственная обёртка для таких
/// массивов: `Drop` всегда вызывает `SafeArrayDestroy`, на любой ветке.
struct OwnedArray(*mut SAFEARRAY);

impl OwnedArray {
    /// Копия одномерного массива элементов `T` с типом `vt`; другой массив или ошибка — `None`.
    fn to_vec<T: Copy>(&self, vt: VARENUM) -> Option<Vec<T>> {
        let psa = self.0;
        if psa.is_null() {
            return None;
        }
        // SAFETY: `psa` — живой массив, которым владеет `self`; размер элемента и число измерений
        // проверены до чтения; данные читаются только между Access/Unaccess.
        unsafe {
            if SafeArrayGetDim(psa) != 1 || SafeArrayGetElemsize(psa) as usize != std::mem::size_of::<T>() {
                return None;
            }
            // Тип известен не у всех массивов (нет FADF_HAVEVARTYPE) — тогда хватает проверки размера.
            if let Ok(actual) = SafeArrayGetVartype(psa) {
                if actual != vt {
                    return None;
                }
            }
            let lower = SafeArrayGetLBound(psa, 1).ok()?;
            let upper = SafeArrayGetUBound(psa, 1).ok()?;
            let count = usize::try_from(i64::from(upper) - i64::from(lower) + 1).unwrap_or(0);
            let mut data: *mut c_void = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut data).ok()?;
            let out = if count == 0 || data.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data as *const T, count).to_vec()
            };
            let _ = SafeArrayUnaccessData(psa);
            Some(out)
        }
    }
}

impl Drop for OwnedArray {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: массив принадлежит нам и больше нигде не используется.
            unsafe {
                let _ = SafeArrayDestroy(self.0);
            }
        }
    }
}

/// Массив `VT_I4` (runtime id).
fn ints(array: windows::core::Result<*mut SAFEARRAY>) -> Option<Vec<i32>> {
    OwnedArray(array.ok()?).to_vec::<i32>(VT_I4)
}

/// Массив `VT_R8` (прямоугольники диапазона: x, y, w, h подряд).
fn doubles(array: windows::core::Result<*mut SAFEARRAY>) -> Option<Vec<f64>> {
    OwnedArray(array.ok()?).to_vec::<f64>(VT_R8)
}

/// BSTR (UTF-16) -> String; непарные суррогаты -> U+FFFD.
fn bstr(b: &BSTR) -> String {
    String::from_utf16_lossy(b.as_wide())
}

/// Python `not s.strip()`: `str.isspace` — это White_Space Юникода и ещё U+001C–U+001F.
fn blank(s: &str) -> bool {
    s.chars().all(|c| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Python `a // b` для `b > 0` (вниз, а не к нулю: координаты бывают отрицательными).
fn floor_div(a: i64, b: i64) -> i32 {
    a.div_euclid(b).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn clamp_i32(v: isize) -> i32 {
    v.clamp(i32::MIN as isize, i32::MAX as isize) as i32
}

/// P 1323–1332: позиции `[start, end)` имени -> позиции для `MoveEndpointByUnit(Character)`.
/// Если текст диапазона не равен имени (маркер списка «2. »), сдвиг — по первым 20 символам имени;
/// затем вычитаются кодовые точки, которые UIA считает внутри одного символа (`grapheme_extra`).
/// `None` — имени в тексте диапазона нет.
fn piece_offsets(text: &[char], name: &[char], start: usize, end: usize) -> Option<(isize, isize)> {
    let (mut start, mut end) = (start, end);
    if text != name {
        let head = &name[..name.len().min(20)];
        let shift = if head.is_empty() { 0 } else { find(text, head, 0)? };
        start += shift;
        end += shift;
    }
    let start = start.saturating_sub(grapheme_extra(&text[..start.min(text.len())]));
    let end = end.saturating_sub(grapheme_extra(&text[..end.min(text.len())]));
    Some((start as isize, end as isize))
}

impl Uia {
    /// CUIAutomation8 + таймауты 2000 мс, иначе CUIAutomation.
    /// Поток должен быть в MTA (`CoInitializeEx(COINIT_MULTITHREADED)`), иначе `Err`.
    pub fn new() -> Result<Uia, ()> {
        let automation = Self::create().ok_or(())?;
        // SAFETY: вызовы COM на живом объекте; ошибки возвращаются как `Err`.
        unsafe {
            let walker = automation.RawViewWalker().map_err(|_| ())?;
            let cache_type = automation.CreateCacheRequest().map_err(|_| ())?;
            cache_type.AddProperty(UIA_ControlTypePropertyId).map_err(|_| ())?;
            // Всё поддерево одним межпроцессным вызовом: Name + BoundingRectangle каждого узла.
            let cache_tree = automation.CreateCacheRequest().map_err(|_| ())?;
            cache_tree.AddProperty(UIA_NamePropertyId).map_err(|_| ())?;
            cache_tree.AddProperty(UIA_BoundingRectanglePropertyId).map_err(|_| ())?;
            cache_tree.SetTreeScope(TreeScope_Subtree).map_err(|_| ())?;
            let raw = automation.RawViewCondition().map_err(|_| ())?;
            cache_tree.SetTreeFilter(&raw).map_err(|_| ())?;
            Ok(Uia { automation, walker, cache_type, cache_tree })
        }
    }

    /// CUIAutomation8 с таймаутами; не вышло (старая Windows, ошибка таймаутов) — CUIAutomation.
    fn create() -> Option<IUIAutomation> {
        // SAFETY: создание COM-объекта в текущем (MTA) потоке.
        unsafe {
            let fresh: windows::core::Result<IUIAutomation2> =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
            if let Ok(a) = fresh {
                if a.SetConnectionTimeout(TIMEOUT_MS).is_ok() && a.SetTransactionTimeout(TIMEOUT_MS).is_ok() {
                    if let Ok(base) = a.cast::<IUIAutomation>() {
                        return Some(base);
                    }
                }
            }
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        }
    }

    // ------------------------------------------------------------ помощники (сырые COM-объекты)

    fn rid_raw(&self, e: &IUIAutomationElement) -> Option<RuntimeId> {
        // SAFETY: вызов COM; массив сразу переходит во владение `OwnedArray`.
        let ids = ints(unsafe { e.GetRuntimeId() })?;
        if ids.is_empty() {
            None
        } else {
            Some(ids.into_boxed_slice())
        }
    }

    fn process_of(&self, e: &IUIAutomationElement) -> Option<u32> {
        // SAFETY: вызов COM.
        unsafe { e.CurrentProcessId() }.ok().map(|pid| pid as u32)
    }

    fn text_pattern_raw(&self, e: &IUIAutomationElement) -> Option<IUIAutomationTextPattern> {
        // SAFETY: вызов COM; нет шаблона — S_OK с NULL, это `Err` -> `None`.
        unsafe { e.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok()
    }

    /// (элемент, тип) от `e` вверх, не больше `limit` штук (Python `ancestors`). `visit` возвращает
    /// `false`, чтобы остановиться. Родители берутся с кэшем ControlType — один вызов на шаг.
    fn ancestors(
        &self,
        e: &IUIAutomationElement,
        limit: usize,
        mut visit: impl FnMut(&IUIAutomationElement, UIA_CONTROLTYPE_ID) -> bool,
    ) {
        // SAFETY: вызовы COM; любая ошибка заканчивает подъём.
        unsafe {
            let Ok(mut kind) = e.CurrentControlType() else { return };
            let mut current = e.clone();
            for step in 0..limit {
                if !visit(&current, kind) || step + 1 == limit {
                    return;
                }
                let Ok(parent) = self.walker.GetParentElementBuildCache(&current, &self.cache_type) else { return };
                let Ok(parent_kind) = parent.CachedControlType() else { return };
                current = parent;
                kind = parent_kind;
            }
        }
    }

    fn document_raw(&self, e: &IUIAutomationElement) -> Option<IUIAutomationElement> {
        let mut found = None;
        self.ancestors(e, ANCESTORS, |current, kind| {
            if kind == UIA_DocumentControlTypeId {
                found = Some(current.clone());
                return false;
            }
            true
        });
        found
    }

    fn range_rects(&self, range: &IUIAutomationTextRange) -> Vec<Rect> {
        // SAFETY: вызов COM; массив сразу переходит во владение `OwnedArray`.
        doubles(unsafe { range.GetBoundingRectangles() }).map(|v| to_rects(&v)).unwrap_or_default()
    }

    fn elements(array: &IUIAutomationElementArray) -> Vec<Element> {
        // SAFETY: вызовы COM; пропавшие элементы пропускаются.
        unsafe {
            let count = array.Length().unwrap_or(0);
            (0..count).filter_map(|i| array.GetElement(i).ok().map(Element)).collect()
        }
    }

    fn window_root(&self, hwnd: Hwnd) -> Option<IUIAutomationElement> {
        // SAFETY: вызов COM; неверный дескриптор — `Err`.
        unsafe { self.automation.ElementFromHandle(to_hwnd(hwnd)) }.ok()
    }

    fn element_at(&self, p: Point) -> Option<IUIAutomationElement> {
        // SAFETY: вызов COM; пустая точка — `Err`.
        unsafe { self.automation.ElementFromPoint(POINT::from(p)) }.ok()
    }

    // ------------------------------------------------------------ интерфейс раздела 5.5

    /// Выделение UIA из элемента в фокусе или под курсором (только процесса `pid`): подъём до
    /// 30 предков, первый TextPattern с непустым `GetSelection`; на Document подъём кончается.
    pub fn selection(&self, pid: u32, cursor: Point) -> Selection {
        let mut sources = Vec::with_capacity(2);
        // SAFETY: вызов COM.
        if let Ok(focused) = unsafe { self.automation.GetFocusedElement() } {
            if self.process_of(&focused) == Some(pid) {
                sources.push(focused);
            }
        }
        if let Some(pointed) = self.element_at(cursor) {
            if self.process_of(&pointed) == Some(pid) {
                sources.push(pointed);
            }
        }
        let mut any_pattern = false;
        for source in &sources {
            let mut found: Option<Selection> = None;
            self.ancestors(source, SELECTION_ANCESTORS, |element, kind| {
                let Some(pattern) = self.text_pattern_raw(element) else { return true };
                any_pattern = true;
                // SAFETY: вызовы COM; ошибки — «нет выделения».
                unsafe {
                    let ranges = pattern.GetSelection().ok();
                    let count = ranges.as_ref().and_then(|r| r.Length().ok()).unwrap_or(0);
                    for i in 0..count {
                        let Some(range) = ranges.as_ref().and_then(|r| r.GetElement(i).ok()) else { continue };
                        let Ok(text) = range.GetText(SELECTION_TEXT) else { continue };
                        let text = bstr(&text);
                        if blank(&text) {
                            continue;
                        }
                        let scope = if kind == UIA_DocumentControlTypeId {
                            element.clone()
                        } else {
                            self.document_raw(element).unwrap_or_else(|| element.clone())
                        };
                        found = Some(Selection {
                            text,
                            rects: self.range_rects(&range),
                            scope: Some(Element(scope)),
                            any_pattern: true,
                        });
                        return false;
                    }
                }
                kind != UIA_DocumentControlTypeId
            });
            if let Some(selection) = found {
                return selection;
            }
        }
        Selection { text: String::new(), rects: Vec::new(), scope: None, any_pattern }
    }

    /// Документы, показанные в окне: hit test в трёх точках средней линии (50 %, 30 %, 70 % ширины);
    /// ничего — корень окна как `ScopeKey::Root` (без hit test, без ▲/▼).
    pub fn shown_scopes(&self, hwnd: Hwnd, pid: u32, rect: Option<Rect>) -> Vec<(ScopeKey, Element)> {
        let mut scopes = Vec::new();
        let mut seen: HashSet<RuntimeId> = HashSet::new();
        if let Some(r) = rect {
            let (l, t, rr, b) = (i64::from(r.l), i64::from(r.t), i64::from(r.r), i64::from(r.b));
            let middle = floor_div(t + b, 2);
            // Python: (left + right) // 2, left + (right - left) * 3 // 10, left + (right - left) * 7 // 10
            // (l * 10 кратно 10, поэтому деление суммы — то же, что l + деление второго слагаемого).
            let xs = [floor_div(l + rr, 2), floor_div(l * 10 + (rr - l) * 3, 10), floor_div(l * 10 + (rr - l) * 7, 10)];
            for x in xs {
                let Some(hit) = self.element_at(Point { x, y: middle }) else { continue };
                if self.process_of(&hit) != Some(pid) {
                    continue;
                }
                let Some(document) = self.document_raw(&hit) else { continue };
                let Some(key) = self.rid_raw(&document) else { continue };
                if seen.insert(key.clone()) {
                    scopes.push((ScopeKey::Doc(key), Element(document)));
                }
            }
        }
        if scopes.is_empty() {
            if let Some(root) = self.window_root(hwnd) {
                if let Some(key) = self.rid_raw(&root) {
                    scopes.push((ScopeKey::Root(key), Element(root)));
                }
            }
        }
        scopes
    }

    /// Именованные листья под `scope` в порядке документа: в Chromium это куски видимого текста
    /// (Text, пункты списка без детей, ссылки), в родном UI — подписи и пункты списков.
    /// Один `BuildUpdatedCache` всего поддерева, обход стеком; `limit` — предел узлов (30000).
    /// `Err` — поддерево не получено.
    pub fn segments(&self, scope: &Element, limit: usize) -> Result<Vec<Segment>, ()> {
        // SAFETY: вызовы COM; узлы читаются из кэша (без межпроцессных вызовов).
        unsafe {
            let root = scope.0.BuildUpdatedCache(&self.cache_tree).map_err(|_| ())?;
            let mut result = Vec::new();
            let mut stack = vec![root];
            let mut seen = 0usize;
            while seen < limit {
                let Some(element) = stack.pop() else { break };
                seen += 1;
                let children = element.GetCachedChildren().ok();
                let count = children.as_ref().and_then(|c| c.Length().ok()).unwrap_or(0);
                if count > 0 {
                    if let Some(children) = &children {
                        for i in (0..count).rev() {
                            if let Ok(child) = children.GetElement(i) {
                                stack.push(child);
                            }
                        }
                    }
                    continue;
                }
                let Ok(name) = element.CachedName() else { continue };
                let name = bstr(&name);
                if blank(&name) {
                    continue;
                }
                let Ok(rect) = element.CachedBoundingRectangle() else { continue };
                result.push(Segment { element: Element(element), name, rect: Rect::from(rect) });
            }
            Ok(result)
        }
    }

    pub fn text_pattern(&self, e: &Element) -> Option<TextPattern> {
        self.text_pattern_raw(&e.0).map(TextPattern)
    }

    pub fn rid(&self, e: &Element) -> Option<RuntimeId> {
        self.rid_raw(&e.0)
    }

    pub fn parent_rid(&self, e: &Element) -> Option<RuntimeId> {
        // SAFETY: вызов COM; у корня родителя нет — `Err` -> `None`.
        let parent = unsafe { self.walker.GetParentElement(&e.0) }.ok()?;
        self.rid_raw(&parent)
    }

    /// Ближайший Document среди `e` и его предков (до 40).
    pub fn document_of(&self, e: &Element) -> Option<Element> {
        self.document_raw(&e.0).map(Element)
    }

    pub fn is_document(&self, e: &Element) -> bool {
        // SAFETY: вызов COM.
        matches!(unsafe { e.0.CurrentControlType() }, Ok(kind) if kind == UIA_DocumentControlTypeId)
    }

    pub fn current_rect(&self, e: &Element) -> Option<Rect> {
        // SAFETY: вызов COM.
        unsafe { e.0.CurrentBoundingRectangle() }.ok().map(Rect::from)
    }

    pub fn current_name(&self, e: &Element) -> Option<String> {
        // SAFETY: вызов COM.
        unsafe { e.0.CurrentName() }.ok().map(|name| bstr(&name))
    }

    /// Прямоугольники строк символов `[start, end)` (в `char`) элемента Text с именем `name`:
    /// `RangeFromChild` -> `GetText(len + 64)` -> сдвиг по маркеру списка -> `grapheme_extra` ->
    /// `Clone` -> `MoveEndpointByUnit(Start)` -> `MoveEndpointByRange(End, Start)` ->
    /// `MoveEndpointByUnit(End, max(1, end - start))` -> `GetBoundingRectangles`.
    pub fn piece_rects(&self, p: Option<&TextPattern>, e: &Element, name: &str, start: usize, end: usize) -> Vec<Rect> {
        let Some(pattern) = p else { return Vec::new() };
        self.piece_rects_raw(&pattern.0, &e.0, name, start, end).unwrap_or_default()
    }

    fn piece_rects_raw(
        &self,
        pattern: &IUIAutomationTextPattern,
        element: &IUIAutomationElement,
        name: &str,
        start: usize,
        end: usize,
    ) -> Option<Vec<Rect>> {
        // SAFETY: вызовы COM; любая ошибка — пустой результат (как `except` в Python).
        unsafe {
            let mut found = pattern.RangeFromChild(element).ok()?;
            // Python: len(name) + 64 кодовых точек; здесь — в UTF-16, как считает UIA (не меньше).
            let max = clamp_i32(name.encode_utf16().count().saturating_add(64) as isize);
            let text: Vec<char> = bstr(&found.GetText(max).ok()?).chars().collect();
            let name: Vec<char> = name.chars().collect();
            let (start, end) = piece_offsets(&text, &name, start, end)?;
            if start > 0 || end < text.len() as isize {
                found = found.Clone().ok()?;
                if start != 0 {
                    found.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, clamp_i32(start)).ok()?;
                }
                found.MoveEndpointByRange(TextPatternRangeEndpoint_End, &found, TextPatternRangeEndpoint_Start).ok()?;
                found
                    .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, clamp_i32((end - start).max(1)))
                    .ok()?;
            }
            Some(self.range_rects(&found))
        }
    }

    /// Runtime id элемента в точке и его предков, ближайший первым (`depth` штук).
    /// Chromium отвечает на `ElementFromPoint` результатом прошлого асинхронного hit test и
    /// обновляет его в фоне, поэтому точку спрашивают дважды: при `settle_s > 0` — подготовка,
    /// пауза и чтение здесь же; при `settle_s == 0` подготовку делает `prime` на прошлом тике.
    pub fn hit_chain(&self, p: Point, depth: usize, settle_s: f64) -> Vec<RuntimeId> {
        if settle_s > 0.0 {
            self.prime(p);
            if let Ok(pause) = Duration::try_from_secs_f64(settle_s) {
                std::thread::sleep(pause);
            }
        }
        let Some(mut current) = self.element_at(p) else { return Vec::new() };
        let mut chain = Vec::with_capacity(depth);
        for step in 0..depth {
            if let Some(key) = self.rid_raw(&current) {
                chain.push(key);
            }
            if step + 1 == depth {
                break;
            }
            // SAFETY: вызов COM; у корня родителя нет — конец цепочки.
            match unsafe { self.walker.GetParentElement(&current) } {
                Ok(parent) => current = parent,
                Err(_) => break,
            }
        }
        chain
    }

    /// Первая половина hit test Chromium: заставляет его посчитать ответ для этой точки.
    /// Читать (`hit_chain(p, 3, 0.0)`) — не раньше чем через 50 мс.
    pub fn prime(&self, p: Point) {
        let _ = self.element_at(p);
    }

    /// (url, name) документа: адрес страницы (ValuePattern) и заголовок.
    pub fn document_info(&self, e: &Element) -> (String, String) {
        // SAFETY: вызовы COM; ошибка — пустая строка.
        unsafe {
            let name = e.0.CurrentName().ok().map(|n| bstr(&n)).unwrap_or_default();
            let url = e
                .0
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()
                .and_then(|value| value.CurrentValue().ok())
                .map(|v| bstr(&v))
                .unwrap_or_default();
            (url, name)
        }
    }

    /// Вкладки окна браузера: `FindAll(TreeScope_Descendants, ControlType = TabItem 50019)`;
    /// 50–75 мс; работает и для свёрнутого окна.
    pub fn tabs(&self, hwnd: Hwnd) -> Vec<Element> {
        let Some(root) = self.window_root(hwnd) else { return Vec::new() };
        // SAFETY: вызовы COM; ошибка — пустой список.
        unsafe {
            let Ok(condition) = self
                .automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0))
            else {
                return Vec::new();
            };
            match root.FindAll(TreeScope_Descendants, &condition) {
                Ok(found) => Self::elements(&found),
                Err(_) => Vec::new(),
            }
        }
    }

    pub fn select(&self, e: &Element) -> bool {
        // SAFETY: вызовы COM.
        unsafe {
            e.0.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
                .and_then(|item| item.Select())
                .is_ok()
        }
    }

    /// Ровно одна Hyperlink с точным именем.
    /// Условие «Hyperlink 50005 И Name = имя»; найдено не ровно одна — `false` (список чатов приложения).
    pub fn invoke_link(&self, hwnd: Hwnd, name: &str) -> bool {
        let Some(root) = self.window_root(hwnd) else { return false };
        // SAFETY: вызовы COM; любая ошибка — `false`.
        let invoked = || -> windows::core::Result<bool> {
            unsafe {
                let link = self
                    .automation
                    .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_HyperlinkControlTypeId.0))?;
                let named = self.automation.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(name))?;
                let both = self.automation.CreateAndCondition(&link, &named)?;
                let found = root.FindAll(TreeScope_Descendants, &both)?;
                if found.Length()? != 1 {
                    return Ok(false);
                }
                let invoke =
                    found.GetElement(0)?.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?;
                invoke.Invoke()?;
                Ok(true)
            }
        };
        invoked().unwrap_or(false)
    }

    /// (id от элемента вверх до его Document, не включая; id Document и выше).
    pub fn lineage(&self, e: &Element, limit: usize) -> (HashSet<RuntimeId>, HashSet<RuntimeId>) {
        let (mut inner, mut outer) = (HashSet::new(), HashSet::new());
        let mut below = true;
        self.ancestors(&e.0, limit, |current, kind| {
            if kind == UIA_DocumentControlTypeId {
                below = false;
            }
            if let Some(key) = self.rid_raw(current) {
                if below {
                    inner.insert(key);
                } else {
                    outer.insert(key);
                }
            }
            true
        });
        (inner, outer)
    }

    /// `ScrollItemPattern.ScrollIntoView`, иначе `RangeFromChild(element).ScrollIntoView(true)`.
    pub fn scroll_to(&self, e: &Element, p: Option<&TextPattern>) -> bool {
        // SAFETY: вызовы COM.
        unsafe {
            if let Ok(item) = e.0.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId) {
                if item.ScrollIntoView().is_ok() {
                    return true;
                }
            }
            if let Some(pattern) = p {
                if let Ok(range) = pattern.0.RangeFromChild(&e.0) {
                    if range.ScrollIntoView(TRUE).is_ok() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Для selftest auto.
    /// Текст строки под точкой: `RangeFromPoint` -> `ExpandToEnclosingUnit(Line)` -> `GetText(max)`.
    pub fn range_text_at(&self, p: &TextPattern, at: Point, max: i32) -> Option<String> {
        // SAFETY: вызовы COM.
        unsafe {
            let range = p.0.RangeFromPoint(POINT::from(at)).ok()?;
            range.ExpandToEnclosingUnit(TextUnit_Line).ok()?;
            range.GetText(max).ok().map(|text| bstr(&text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Ole::SafeArrayCreateVector;
    use windows::Win32::System::Variant::VT_R4;

    /// Свой массив `VT_I4` — как runtime id от UIA.
    fn vector_i4(values: &[i32]) -> *mut SAFEARRAY {
        // SAFETY: массив создаётся здесь; данные пишутся между Access/Unaccess.
        unsafe {
            let psa = SafeArrayCreateVector(VT_I4, 0, values.len() as u32);
            assert!(!psa.is_null());
            let mut data: *mut c_void = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut data).expect("SafeArrayAccessData");
            if !values.is_empty() {
                std::ptr::copy_nonoverlapping(values.as_ptr(), data as *mut i32, values.len());
            }
            SafeArrayUnaccessData(psa).expect("SafeArrayUnaccessData");
            psa
        }
    }

    #[test]
    fn owned_array_reads_and_checks_type() {
        let ids = OwnedArray(vector_i4(&[42, 7, -1]));
        assert_eq!(ids.to_vec::<i32>(VT_I4), Some(vec![42, 7, -1]));
        assert_eq!(ids.to_vec::<f64>(VT_R8), None, "другой размер элемента");
        assert_eq!(ids.to_vec::<f32>(VT_R4), None, "тот же размер, другой тип");
        assert_eq!(OwnedArray(vector_i4(&[])).to_vec::<i32>(VT_I4), Some(Vec::new()));
        assert_eq!(OwnedArray(std::ptr::null_mut()).to_vec::<i32>(VT_I4), None);
        assert_eq!(ints(Ok(vector_i4(&[1, 2]))), Some(vec![1, 2]));
        assert_eq!(ints(Err(windows::core::Error::empty())), None);
    }

    #[test]
    fn python_like_helpers() {
        assert!(blank("") && blank(" \t\u{a0}\u{3000}\n") && blank("\u{1c}\u{1f}"));
        assert!(!blank(" а ") && !blank("\u{200b}"));
        assert_eq!(floor_div(-3, 2), -2);
        assert_eq!(floor_div(-1920 * 10 + 1920 * 3, 10), -1920 + 576);
        assert_eq!(floor_div(7, 2), 3);
        assert_eq!(clamp_i32(isize::MAX), i32::MAX);
    }
}
