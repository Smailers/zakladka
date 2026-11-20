"""Эталоны для Rust-версии «Закладки» из Python-версии (часть 1 плана переписывания).

Запускать интерпретатором старой версии (он же находит её папку: .venv -> text-bookmark):

  <text-bookmark>\\.venv\\Scripts\\python.exe -B tools\\gen_golden.py
      пишет tests\\golden\\*.json рядом с этим скриптом
  <text-bookmark>\\.venv\\Scripts\\python.exe -B tools\\gen_golden.py --check <файл>
      читает <файл> функцией load_bookmarks Python-версии и печатает результат JSON (ASCII)

Импорт text_bookmark.pyw и panel.py без побочных действий (как в test_text_bookmark.py);
байт-код не пишется (sys.dont_write_bytecode), старая папка не меняется.
Настоящий %LOCALAPPDATA%\\text-bookmark\\bookmarks.json этот скрипт не читает и не пишет.
"""

import sys

sys.dont_write_bytecode = True

import argparse  # noqa: E402
from datetime import datetime  # noqa: E402
import importlib.machinery  # noqa: E402
import importlib.util  # noqa: E402
import json  # noqa: E402
from pathlib import Path  # noqa: E402
import random  # noqa: E402
import unicodedata  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "tests" / "golden"
NOW = datetime(2026, 11, 20)


def load_old(folder):
    path = folder / "text_bookmark.pyw"
    loader = importlib.machinery.SourceFileLoader("text_bookmark", str(path))
    spec = importlib.util.spec_from_file_location("text_bookmark", path, loader=loader)
    tb = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(tb)
    sys.path.insert(0, str(folder))
    import panel  # noqa: E402  (tkinter импортируется, окна не создаются)
    return tb, panel


def write(name, payload):
    GOLDEN.mkdir(parents=True, exist_ok=True)
    with (GOLDEN / name).open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=1)
        stream.write("\n")


def outcome(function, *args):
    """Результат или имя ошибки (для случаев с ValueError)."""
    try:
        return {"ok": function(*args)}
    except ValueError as error:
        return {"error": str(error)}


# ---------------------------------------------------------------- flatten

SPECIAL = [
    "Ёлка", "ёжик", "ЁЁЁ", "Ёлка  и\u200bЛЕС\ufffc!", "ﬁ①", "ﬁ", "ﬂ", "ﬀ", "ﬃ", "ﬄ", "ﬅ", "ﬆ", "Ĳ", "ǆ", "ǅ",
    "Ǆ", "ǈ", "👍🏻", "👍🏿", "👨\u200d👩\u200d👧\u200d👦", "🏳\ufe0f\u200d🌈", "❤\ufe0f", "⚠\ufe0f x", "🇷🇺",
    "1\ufe0f\u20e3", "👩🏽\u200d💻", "\u00a0", "a\u00a0b", "\u202f", "\u2009", "\u3000", "\u2028", "\u0085", "\x1c",
    "\x1f", "\t", "\r\n", "\u180e", "\u200b", "\ufeff", "\u00ad", "\ufffc", "\u2060", "ß", "ẞ", "İ", "ı",
    "i\u0307", "Σ", "ς", "ΣΑΣ", "ŉ", "ǰ", "ΐ", "ᾼ", "ᾳ", "ﬓ", "Ꭰ", "ꭰ", "漢字", "日本語", "한국어", "ｶﾀｶﾅ",
    "ﾞ", "㍿", "㌀", "①②③", "⑴", "²³", "½", "Ⅻ", "㎏", "℡", "™", "\u212b", "\u212a", "\u2126", "ℌ", "ﻻ", "ﷺ",
    "क्षत्रिय", "e\u0301", "ñ", "A\u030a", "ＡＢＣ１２３", "！", "𝐀𝐁", "𝔄", "¨", "…", "‐", "—", "№", "℃",
    "Straße", "İstanbul", "ΌΣΟΣ", "ﬀﬁﬂ", "Ⅷ", "ｱｲｳ", "\u0345", "\u1e9e", "ǲ", "\U0001d7d9", "\u00b5",
    "\u017f", "\u1e9b", "ﬖ", "ﬔ", "\u0390", "\u03b0", "\u1f80", "\u1fb3", "\u1fbc", "\uab70", "\u13f8",
]

WORDS = ("закладка текст чат окно вкладка готово начало конец разница между оплаты счёта ноября "
         "декабря года срок первый второй итог ёлка лес подсветка метка цитата").split()


def flatten_cases(tb, rng):
    cases = list(SPECIAL)
    pieces = SPECIAL + WORDS + ["Hello", "WORLD", " ", "  ", "\n", "123", "x"]
    while len(cases) < 260:
        cases.append("".join(rng.choice(pieces) for _ in range(rng.randint(1, 8))))
    return [[text, *tb.flatten(text)] for text in cases]


def flatten_table(tb):
    flat, folded, combining, spaces = {}, {}, [], []
    for code in range(0x110000):
        if 0xD800 <= code <= 0xDFFF:
            continue
        char = chr(code)
        result = tb.flatten(char)[0]
        if result != char:
            flat[f"{code:x}"] = result
        if char.casefold() != char:
            folded[f"{code:x}"] = char.casefold()
        if unicodedata.combining(char):
            combining.append(code)
        if char.isspace():
            spaces.append(code)
    return {"unicode": unicodedata.unidata_version, "flatten": flat, "casefold": folded,
            "combining": to_ranges(combining), "isspace": spaces}


def to_ranges(codes):
    ranges = []
    for code in codes:
        if ranges and ranges[-1][1] == code - 1:
            ranges[-1][1] = code
        else:
            ranges.append([code, code])
    return ranges


# ---------------------------------------------------------------- matching

def flat(tb, text):
    return tb.flatten(text)[0]


def mutate(rng, text, rate):
    swaps = {"о": "0", "О": "0", "з": "3", "б": "6", "l": "1", "e": "е", "a": "а", "с": "c", "и": "п"}
    out = []
    for char in text:
        roll = rng.random()
        if roll < rate:
            out.append(swaps.get(char, rng.choice("абвгд0123")))
        elif roll < rate * 1.3:
            continue  # пропуск
        elif roll < rate * 1.6:
            out.append(char + rng.choice("ьъ"))  # вставка
        else:
            out.append(char)
    return "".join(out)


def word_text(tb, rng, count, words=WORDS):
    return flat(tb, " ".join(rng.choice(words) for _ in range(count)))


def similarity_cases(tb, rng):
    cases = [["", ""], ["", "a"], ["a", ""], ["a", "a"], ["ab", "ba"], ["abc", "abc"],
             ["привет", "превед"], ["abcd", "bcda"], ["aaaa", "aa"], ["abab", "baba"]]
    for alphabet in ("ab", "abc", "abcde", "abcdefghij", "абвгдеёжзийклмнопрстуфхцчшщъыьэюя"):
        for _ in range(45):
            a = "".join(rng.choice(alphabet) for _ in range(rng.randint(0, 60)))
            b = "".join(rng.choice(alphabet) for _ in range(rng.randint(0, 60)))
            cases.append([a, b])
    for _ in range(30):
        a = word_text(tb, rng, rng.randint(10, 40))
        cases.append([a, mutate(rng, a, rng.choice((0.02, 0.1, 0.3)))])
    for _ in range(12):
        a = word_text(tb, rng, rng.randint(60, 100))
        cases.append([a[:700], mutate(rng, a, 0.05)[:700]])
    for _ in range(6):
        a = "".join(rng.choice("ab") for _ in range(rng.randint(300, 500)))
        b = "".join(rng.choice("ab") for _ in range(rng.randint(300, 500)))
        cases.append([a, b])
    return [[a, b, tb.similarity(a, b)] for a, b in cases]


def matching_cases(tb, rng):
    best, fuzzy_best, candidates, quotes, occurrences, contexts = [], [], [], [], [], []
    short_words = WORDS[:8]
    while len(best) < 140:
        text = word_text(tb, rng, rng.randint(3, 60), rng.choice((WORDS, short_words)))
        if rng.random() < 0.3:  # повторяющаяся фраза: равные оценки
            phrase = word_text(tb, rng, 3)
            text = (phrase + word_text(tb, rng, 2)) * rng.randint(2, 4) + text
        start = rng.randint(0, max(0, len(text) - 1))
        quote = text[start:start + rng.randint(3, 50)] if rng.random() < 0.85 else word_text(tb, rng, 2)
        if not quote:
            continue
        where = text.find(quote)
        if rng.random() < 0.6 and where >= 0:
            prefix = text[max(0, where - rng.randint(0, 64)):where]
            suffix = text[where + len(quote):where + len(quote) + rng.randint(0, 64)]
        else:
            prefix, suffix = word_text(tb, rng, rng.randint(0, 3)), word_text(tb, rng, rng.randint(0, 3))
        if rng.random() < 0.3:
            prefix = mutate(rng, prefix, 0.2)
        min_score = rng.choice((0.3, 0.6, 0.6, 0.8))
        best.append([text, quote, prefix, suffix, min_score,
                     tb.best_occurrence(text, quote, prefix, suffix, min_score)])

        noisy = mutate(rng, text, rng.choice((0.0, 0.03, 0.08)))
        fuzzy_best.append([noisy, quote, prefix, suffix, min_score,
                           tb.fuzzy_best(noisy, quote, prefix, suffix, min_score)])

        min_ratio = rng.choice((0.5, 0.6, 0.75, 0.9))
        gram, limit = rng.choice((2, 3, 5, 5, 7)), rng.choice((1, 3, 30, 30))
        candidates.append([noisy, quote, min_ratio, gram, limit,
                           [list(x) for x in tb.fuzzy_candidates(noisy, quote, min_ratio, gram, limit)]])

        piece = mutate(rng, quote, 0.1) if rng.random() < 0.5 else "xx" + quote + "qq"
        min_len = rng.choice((0, 1, 5, 12, 12, 20))
        quotes.append([text, piece, min_len, tb.find_quote(text, piece, min_len)])

        limit = rng.choice((0, 1, 2, 50, 1000))
        occurrences.append([text, quote, limit, tb.occurrences(text, quote, limit)])

        at = rng.randint(0, len(text))
        end = min(len(text), at + rng.randint(0, 30))
        contexts.append([text, at, end, prefix, suffix, tb.context_score(text, at, end, prefix, suffix)])
    return {"best_occurrence": best, "fuzzy_best": fuzzy_best, "fuzzy_candidates": candidates,
            "find_quote": quotes, "occurrences": occurrences, "context_score": contexts}


def py_set_cases(rng):
    cases = []
    for _ in range(150):
        top = rng.choice((10, 50, 300, 5000, 100000))
        values = [rng.randint(0, top) for _ in range(rng.randint(1, 60))]
        found = set()
        for value in values:
            found.add(value)
        cases.append([values, list(found)])
    return cases


# ---------------------------------------------------------------- geometry

def merge_cases(tb, rng):
    cases = [[[[64, 50, 200, 70], [10, 10, 50, 30], [52, 11, 90, 31], [10, 50, 60, 70]], 14]]
    for _ in range(150):
        rects = []
        for line in range(rng.randint(1, 4)):
            top = rng.randint(-600, 600)
            x = rng.randint(-800, 800)
            for _ in range(rng.randint(1, 8)):
                height = rng.randint(3, 30)
                shift = rng.randint(-6, 6)
                width = rng.randint(2, 90)
                rects.append([x, top + shift, x + width, top + shift + height])
                x += width + rng.choice((0, 2, 5, 10, 14, 15, 30, 80))
        rng.shuffle(rects)
        cases.append([rects, rng.choice((0, 5, 14, 14, 30))])
    return [[rects, gap, [list(r) for r in tb.merge_lines([tuple(r) for r in rects], gap)]]
            for rects, gap in cases]


def to_rects_cases(tb, rng):
    cases = [[10.4, 20, 100, 30, 5, 5, 1, 30], [], [0.5, 1.5, 2.5, 3.5], [2.5, 2.5, 3.5, 3.5, 1], [-0.5, -1.5, 3, 3]]
    for _ in range(60):
        cases.append([rng.choice((rng.uniform(-500, 500), round(rng.uniform(-50, 50)) + 0.5))
                      for _ in range(rng.choice((4, 8, 9, 12)))])
    return [[values, [list(r) for r in tb.to_rects(values)]] for values in cases]


def doc_cases(tb, rng):
    """FlatDoc.spans, cut_to_segments, choose_capture на случайных сегментах."""
    spans, cuts, captures = [], [], []
    for _ in range(80):
        names = [rng.choice(WORDS + [" ", "  Ёж ", "ﬁ", "👍🏻", "a\u200bb"]) for _ in range(rng.randint(1, 25))]
        doc = tb.FlatDoc(names)
        a, b = rng.randint(-5, len(doc.text) + 5), rng.randint(-5, len(doc.text) + 5)
        spans.append([names, a, b, [list(s) for s in doc.spans(a, b)]])
        if doc.text:
            start = rng.randint(0, len(doc.text) - 1)
            length = rng.randint(0, len(doc.text) - start)
            most = rng.choice((1, 2, 3, 12))
            cuts.append([names, start, length, most, tb.cut_to_segments(doc, start, length, most)])
        quote = rng.choice(WORDS[:6])
        rects = [[0, i * 20, 50, i * 20 + 15] if rng.random() < 0.9 else None for i in range(len(names))]
        selection = [[5, rng.randint(0, 20 * len(names)), 20, rng.randint(0, 20 * len(names))]] if rng.random() < 0.5 else []
        cursor = [rng.randint(-50, 100), rng.randint(-50, 20 * len(names))]
        result = tb.choose_capture(doc, flat(tb, quote), [tuple(r) if r else None for r in rects],
                                   [tuple(r) for r in selection], tuple(cursor))
        captures.append([names, flat(tb, quote), rects, selection, cursor, result])
    return {"spans": spans, "cut_to_segments": cuts, "choose_capture": captures}


# ---------------------------------------------------------------- colours

def color_cases(tb, panel, rng):
    def color():
        return "#" + "".join(rng.choice("0123456789ABCDEF") for _ in range(6))

    presets = [c for c, _ in panel.PRESETS]
    pairs = [[a, b] for a in presets for b in presets + ["#000000", "#FFFFFF"]]
    pairs += [[color(), color()] for _ in range(150)]
    contrast = [[a, b, panel.contrast_ratio(a, b), tb.contrast_ratio(a, b)] for a, b in pairs]
    labels = [[c, panel.label_color(c), tb.contrast_label(c)] for c in presets + [color() for _ in range(100)]]
    mixes = [[a, b, s, panel.mix(a, b, s)] for a, b, s in
             [[color(), color(), rng.choice((0, 0.25, 0.5, 1, 2, -1, rng.random()))] for _ in range(100)]]
    normalize = []
    for text in ["ffd400", " #fd0 ", "", "#12345", "#GGGGGG", "auto", "##fd0", "#ABCDEF", "abc", "#abcd",
                 "\t#00ff00\n", "#00FF00 ", "１２３", "#ffd4000", "#-12345", "+12345"]:
        try:
            normalize.append([text, panel.normalize_color(text)])
        except ValueError:
            normalize.append([text, None])
    opacity = [[x, panel.clean_colors({"opacity": x})["opacity"]] for x in
               [0.3, 0.05, 0.2, 0.6, 0.61, 1.0, 1.5, 30, 45, 90, 0.375, 0.285, 0.555, 0.333333, 100, 0.0, -1]
               + [rng.uniform(0, 1) for _ in range(40)] + [rng.uniform(1, 100) for _ in range(20)]]
    return {"contrast": contrast, "label": labels, "mix": mixes, "normalize": normalize, "opacity": opacity}


# ---------------------------------------------------------------- config

def config_cases(tb, rng):
    values = {
        "hotkey_mark": ["Ctrl+Alt+Q", "Ctrl+C", "Win+Shift+B", "Alt+F4", "ctrl + alt + f5", "", 5, None, "Ctrl+Alt+Ж"],
        "hotkey_extra": ["Ctrl+Alt+Shift+Q", "Shift+A", "Ctrl+Win+Space", True, "Ctrl+Alt+F25"],
        "color": ["#00ff00", "#FFD400", "red", "#fff", "#12345G", 7, None, "#ABCDEF"],
        "bar_color": ["#34C759", "orange", "#00FF00", "", ["#00FF00"]],
        "opacity": [0.1, 0.9, 0.35, 2, 0.05, 1, True, "0.5", 0.5, 0.0999, None],
        "max_age_days": [1, 7, 3650, 3651, 0, 7.0, True, "7", -1, 30],
        "max_bookmarks": [10, 9, 5000, 5001, 300, True, 10.0, 1234],
        "hints": [True, False, 0, 1, "true", None],
        "ocr": [True, False, 1],
        "copy_fallback": [False, True, "no"],
        "debug": [True, False, 1, None],
        "label_color": ["auto", "#ABCDEF", "red", "AUTO", "#abcdef", 5, None, "#fff"],
        "unknown": [1, "x", [1, 2]],
    }
    payloads = [[], "text", 5, None, {}, {"hotkey_mark": "Ctrl+Alt+Q", "hotkey_extra": "Ctrl+C", "opacity": 2,
                                          "color": "#00ff00", "bar_color": "red", "max_age_days": 7,
                                          "max_bookmarks": True, "debug": True, "unknown": 1}]
    while len(payloads) < 160:
        keys = rng.sample(sorted(values), rng.randint(1, len(values)))
        payloads.append({key: rng.choice(values[key]) for key in keys})
    return [[payload, tb.parse_config(payload)] for payload in payloads]


# ---------------------------------------------------------------- misc

ISO = [
    "2026-11-19T10:00:00", "2026-11-19T22:40:07", "2026-11-19", "2026-11-19 10:00", "20261119T1000", "20261119",
    "2026-W47-4", "2026W474", "2026W474T10", "2026-W47", "2026W47", "2026-W53", "2026-W53-1", "2020-W53-7",
    "2026-W00-1", "2026-W47-8", "2026-W47-0", "2026-11-19T10:00:00.1234567", "2026-11-19T10:00:00,5",
    "2026-11-19T10:00:00.000001", "2026-11-19x10", "2026-11-19ж10", "2026-11-19T10:00:00Z",
    "2026-11-19T10:00:00+03:00", "2026-11-19T10:00:00-05", "2026-02-29", "2024-02-29T23:59:59.999999",
    "2026-11-19T", "2026-11-1", "0000-01-01", "0001-01-01", "9999-12-31T23:59:59", "2026-11-19T10:60",
    "2026-11-19T1", "2026-11-19T10:00:00.", "2026-11-19T24:00:00", "2026-11-19T10:00:00:00",
    "2026-11-19T100000", "2026-11-19T1000", "2026-11-19T10:0000", "2026-11-19T10:00:00x", "2026-13-01",
    "2026-00-10", "2026-11-31", "2026-11-30", "not a date", "", "2026", "2026-11", "2026-1119", "202611-19",
    "2026-11-19T10:00:00.123+00:00", "2026-11-19T10Z", "2026-11-19T10:00:00.5Z", "+2026-11-19", "2026-11-19 ",
    " 2026-11-19", "2026-11-19T10:00:00 ", "１２３４-11-19", "2026-11-19T10:00:00.12345678901", "2026-W47-",
    "2026-W4", "2026W4", "2026-W47-4T10:00", "2026W47410", "2026W4741000", "2026-W47-410", "9999-W52-6",
    "0001-W01-1", "2026-11-19T10:00:00,", "2026-11-19T10.5", "2026-11-19T10,5", "2026-11-19T10:00.5",
    "2026-11-19T10:00:00.1", "2026-11-19\t10:00", "2026-11-19\u00a010:00", "2026-11-19T10:00:00\u00a0",
    "2026-11-19T10:00:00.12a", "1999-12-31T23:59:59", "2000-02-29", "1900-02-29", "2026-11-19T00:00",
    "2026-11-19😀10:00", "20261119ж10", "2026W47ж10", "2026-11-19жж10", "2026-11-19ж", "2026-11-1ж10:00",
    "2026-11-19ж10:00:00ж", "2026-11-19T1ж", "ж2026-11-19", "2026-11-19T10:00:00:12", "2026-11-19T1000:00",
    "2026-11ж19", "2026-11-ж9T10", "20261119T1ж", "2026-W47ж4", "2026W474ж10", "2026-11-19T10:00+ж",
]


def iso_cases():
    cases = []
    for text in ISO:
        try:
            value = datetime.fromisoformat(text)
            result = None if value.tzinfo is not None else [value.year, value.month, value.day, value.hour,
                                                              value.minute, value.second, value.microsecond]
        except ValueError:
            result = None
        cases.append([text, result])
    return cases


def bookmark(**fields):
    base = {"id": "a", "exe": "claude.exe", "kind": "pos", "quote": "цитата", "prefix": "", "suffix": "",
            "label": "Цитата", "title": "", "created": "2026-11-19T10:00:00"}
    base.update(fields)
    return base


def prune_cases(tb, rng):
    created = ["2026-11-19T10:00:00", "2026-10-21T00:00:00", "2026-10-20T23:59:59", "2026-10-21T00:00:00.5",
               "2026-11-19T10:00:00+03:00", "not a date", "2026-11-25T00:00:00", "2026-11-19", "2026-W47-4",
               "2026-11-19T10:00:00Z", "2025-01-01T00:00:00", "2026-11-20T00:00:00", "2026-11-19T09:00:00"]
    cases = []
    for _ in range(40):
        items = [bookmark(id=str(i), created=rng.choice(created)) for i in range(rng.randint(0, 12))]
        days, count = rng.choice((1, 3, 30, 3650)), rng.choice((0, 1, 2, 5, 300))
        now = rng.choice(("2026-11-20T00:00:00", "2026-11-20T12:30:00.250000"))
        kept = tb.prune(items, datetime.fromisoformat(now), days, count)
        cases.append([[(i["id"], i["created"]) for i in items], now, days, count, [i["id"] for i in kept]])
    return cases


def hotkey_cases(tb):
    texts = ["Ctrl+Alt+M", "ctrl + shift + f5", "Win+Space", "M", "Ctrl+", "Ctrl+M+N", "Ctrl+Ж", "Ctrl+F25", "",
             "Ctrl+Alt+Z", "Win+Shift+B", "Ctrl+C", "Ctrl+Z", "Shift+A", "Alt+F4", "Ctrl+Shift+Z", "Control+Alt+1",
             "Ctrl+Alt+F1", "Ctrl+Alt+F9", "Ctrl+Alt+F10", "Ctrl+Alt+F19", "Ctrl+Alt+F20", "Ctrl+Alt+F24",
             "Ctrl+Alt+F0", "Ctrl+Alt+F01", "Ctrl+Alt+Insert", "Ctrl+Alt+Ins", "Alt+Win+Del", "Ctrl+Alt+Home",
             "Ctrl+Alt+End", "Ctrl+Alt+PageUp", "Ctrl+Alt+PageDown", "Ctrl+Alt+Pause", "Ctrl+Alt+Tab",
             "Ctrl+Alt+_", "Ctrl+Alt+\u212a", "Ctrl+Alt+\u0130", "+", "++", "Ctrl++", "Alt", "Ctrl+Alt",
             "Ctrl+Alt+Shift+Win+Z", "  ctrl+alt+z  ", "CTRL+ALT+Z", "Ctrl\t+Alt+Z", "Ctrl+Alt+Z+", "Win+Alt+Z",
             "Ctrl+Alt+ ", "Ctrl+Alt+F", "Ctrl+Alt+f", "Ctrl+Alt+Space", "Shift+Win+X", "Ctrl+Win+X",
             "Ctrl+Shift+X", "Ctrl+Alt+Ctrl+X", "Ctrl+Alt+X+Y+Z", "Ctrl+Alt+ZZ", "Ctrl+Alt+\u00a0Z"]
    return [[t, outcome(tb.parse_hotkey, t), outcome(tb.safe_hotkey, t)] for t in texts]


def text_cases(tb, panel):
    programs = ["claude.exe", "ChatGPT.exe", "antigravity.exe", "brave.exe", "chrome.exe", "msedge.exe",
                "editors.exe", "WINWORD.EXE", "notepad.exe", r"C:\Apps\telegram.exe", "codex", "", "C:/x/y/Zed.EXE",
                "  spaced.exe  ", "ßtrange.exe", "ǆungla.exe", "файл.exe", "a.exe.exe", ".exe", "Code.exe", "x"]
    titles = ["Инвойс.docx", "*Инвойс.docx", "", "  ●Чат•  ", "Straße*", "ΣΑΣ", "İstanbul", "a.txt - Блокнот"]
    urls = [["https://claude.ai/chat/1#x", "Чат"], ["app://-/index.html", "*Мой чат"], ["", ""],
            ["  HTTPS://A/B#c#d  ", "x"], ["file:///C:/a.html", ""], ["ftp://x", "Имя●"], ["http://", "y"]]
    copies = [["Chrome_WidgetWin_1", "claude.exe", ""], ["Qt5152QWindowIcon", "editors.exe", "Бюджет.xlsx - ONLYOFFICE"],
              ["Qt5152QWindowIcon", "editors.exe", "Инвойс.docx - ONLYOFFICE"], ["Notepad", "notepad.exe", "a.CSV"],
              ["Notepad", "notepad.exe", "a.csvx"], ["Notepad", "notepad.exe", "a.xls_"], ["Notepad", "notepad.exe", "a.XLSM!"],
              ["Notepad", "notepad.exe", "a.xlsb"], ["Notepad", "notepad.exe", "a.ods-b"], ["Notepad", "notepad.exe", "a.xlsxy"],
              ["Notepad", "notepad.exe", "a.xls2"], ["Notepad", "notepad.exe", "a.xlſ"], ["Notepad", "notepad.exe", "a. xls"],
              ["Notepad", "notepad.exe", "a.xlsж"], ["Notepad", "notepad.exe", "a.xls.docx"], ["Notepad", "excel.exe", ""],
              ["mintty", "x.exe", ""], ["Mintty", "x.exe", ""], ["Notepad", "notepad.exe", ".csv"]]
    texts = ["  a\n\tb  c ", "", "A & B", "x" * 100, "\u00a0слово\u2028два\x1cтри", "a&&b", "  ", "😀" * 60]
    return {
        "program_name": [[p, panel.program_name(p)] for p in programs],
        "one_line": [[t, panel.one_line(t)] for t in texts],
        "menu_label": [[t, limit, tb.menu_label(t, limit)] for t in texts for limit in (48, 10, 1, 0)],
        "title_key": [[t, tb.title_key(t)] for t in titles],
        "same_title": [[a, b, tb.same_title(a, b)] for a in titles for b in titles],
        "page_key": [[u, n, tb.page_key(u, n)] for u, n in urls],
        "copy_allowed": [[c, e, t, tb.copy_allowed(c, e, t)] for c, e, t in copies],
        "grapheme_extra": [[t, tb.grapheme_extra(t)] for t in ["abc", "⚠\ufe0f x", "👍🏻", "👨\u200d👩", "e\u0301",
                                                               "क्षत्रिय", "\u0345", "a\u20e3", "🏳\ufe0f\u200d🌈"]],
        "has_cyrillic": [[t, tb.has_cyrillic(t)] for t in ["abc ж", "abc", "\u0400", "\u04ff", "\u0500", "Ѐ"]],
        "help_text": [["Ctrl+Alt+M", "Ctrl+Alt+Shift+M", 30,
                       tb.HELP_TEXT.format(mark="Ctrl+Alt+M", extra="Ctrl+Alt+Shift+M", days=30)],
                      ["{extra}", "{mark}", 7, tb.HELP_TEXT.format(mark="{extra}", extra="{mark}", days=7)]],
        "vk_name": [[vk, tb.App.vk_name(tb.App, vk)] for vk in range(0, 0x100)],
    }


def bookmark_files(tb):
    """Python пишет bookmarks.json (только в tests\\golden), Rust его читает и пишет тот же байт в байт."""
    items = [
        dict(id="0123456789abcdef0123456789abcdef", exe="claude.exe", kind="pos",
             quote="цитата\"с\\кавычками/и\u2028разрывом", prefix="пре\u0001фикс\x7f", suffix="хвост\n\t\r\b\f",
             label="Подпись 👍🏻 & <тег>", title="Чат — Claude", created="2026-11-19T22:40:07", mode="uia",
             page="https://claude.ai/chat/1", doc="Чат", url="https://claude.ai/chat/1#x"),
        dict(id="fedcba9876543210fedcba9876543210", exe="editors.exe", kind="extra",
             quote="ß İ ﬁ 漢字 \u00a0 \u200d 👨\u200d👩\u200d👧", prefix="", suffix="", label="", title="Инвойс.docx",
             created="2026-11-01T00:00:00", mode="ocr", page="", doc="", url=""),
        dict(id="00000000000000000000000000000001", exe="chrome.exe", kind="pos", quote="q", prefix="p", suffix="s",
             label="l", title="t", created="2026-11-19T10:00:00", mode="uia", page="инвойс.docx", doc="Doc",
             url="file:///C:/a.html"),
    ]
    tb.save_bookmarks(GOLDEN / "bookmarks_py.json", items)
    loaded = tb.load_bookmarks(GOLDEN / "bookmarks_py.json", NOW, 3650, 1000)
    write("bookmarks_py_expected.json", loaded)

    mixed = {"version": 1, "bookmarks": [
        dict(bookmark(id="ok"), junk="x"), {"id": 1}, bookmark(id="k", kind="other"), bookmark(id="q", quote=""),
        bookmark(id="e", exe=""), dict(bookmark(id="m"), mode="ocr"), dict(bookmark(id="mb"), mode="other"),
        dict(bookmark(id="mn"), mode=None), dict(bookmark(id="opt"), page=5, doc=None, url="https://a/b"),
        dict(bookmark(id="nl"), label=None), "string", 5, None, [1], dict(bookmark(id="old"), created="2025-01-01T00:00:00"),
        dict(bookmark(id="tz"), created="2026-11-19T10:00:00+03:00"), dict(bookmark(id="late"), created="2026-11-21T00:00:00"),
    ]}
    with (GOLDEN / "bookmarks_mixed.json").open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(mixed, stream, ensure_ascii=False)
    write("bookmarks_mixed_expected.json", tb.load_bookmarks(GOLDEN / "bookmarks_mixed.json", NOW, 30, 300))


def store_cases(tb):
    """Тексты файлов, которые пишет Python: config.json по умолчанию, save_config, строка debug-render."""
    import shutil
    from types import SimpleNamespace

    tb.log_error = lambda error: None  # save_config при ошибке пишет журнал в %LOCALAPPDATA% — не нужно
    scratch = GOLDEN / "_scratch"
    scratch.mkdir(parents=True, exist_ok=True)
    try:
        path = scratch / "config.json"
        with path.open("x", encoding="utf-8") as stream:  # как main(): текстовый режим Windows
            json.dump(tb.DEFAULT_CONFIG, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
        default_text = path.read_bytes().decode("utf-8")
        saves = []
        inputs = [
            None,
            b"{broken",
            b"[1, 2]",
            "﻿{\"z_unknown\": [1, {\"a\": null}], \"color\": \"#000000\", \"ж\": \"т\\n\", \"n\": 1e-05}".encode(),
            json.dumps({"hotkey_mark": "Ctrl+Alt+Z", "later": {}, "debug": True, "opacity": 0.3, "big": 12345678901234,
                        "f": 1e16, "g": 123.0, "neg": -0.5, "empty": [], "e": "\u0001\"\\"}).encode(),
        ]
        changes = {"color": "#ABCDEF", "opacity": 0.35, "label_color": "auto", "hotkey_mark": "Ctrl+Alt+Q", "new": [True]}
        for raw in inputs:
            if path.exists():
                path.unlink()
            if raw is not None:
                path.write_bytes(raw)
            tb.App.save_config(SimpleNamespace(config_path=path), changes)
            saves.append([None if raw is None else raw.decode("utf-8"), path.read_bytes().decode("utf-8")])
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    render = []
    for marks, pills, viewport in [
        ([], [], None),
        ([("0123456789abcdef", [(1, 2, 3, 4), (-5, -6, 7, 8)]), ("short", [])], [("up", "abcdef0123456789")], (0, 0, 1920, 1080)),
        ([("a" * 32, [(10, 20, 30, 40)])], [("down", "b" * 32), ("up", "c")], (-1920, -100, 0, 980)),
    ]:
        render.append([[[bid, [list(r) for r in lines]] for bid, lines in marks], [list(p) for p in pills],
                       list(viewport) if viewport else None,
                       json.dumps({"time": "2026-11-20T10:00:00.123",
                                   "marks": [[bid[:8], lines] for bid, lines in marks],
                                   "pills": [[where, bid[:8]] for where, bid in pills],
                                   "viewport": viewport})])
    return {"default_config_text": default_text, "save_config": [saves, changes], "render": render}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--old", help="папка Python-версии (по умолчанию — родитель .venv интерпретатора)")
    parser.add_argument("--check", help="прочитать bookmarks.json функцией load_bookmarks и напечатать JSON")
    args = parser.parse_args()
    folder = Path(args.old) if args.old else Path(sys.prefix).resolve().parent
    tb, panel = load_old(folder)

    if args.check:
        items = tb.load_bookmarks(Path(args.check), NOW, 3650, 100000)
        sys.stdout.write(json.dumps(items, ensure_ascii=True))
        return

    rng = random.Random(20261120)
    write("flatten.json", flatten_cases(tb, rng))
    write("flatten_table.json", flatten_table(tb))
    write("similarity.json", similarity_cases(tb, rng))
    write("matching.json", matching_cases(tb, rng))
    write("pyset.json", py_set_cases(rng))
    write("geometry.json", {"merge_lines": merge_cases(tb, rng), "to_rects": to_rects_cases(tb, rng),
                            **doc_cases(tb, rng)})
    write("color.json", color_cases(tb, panel, rng))
    write("config.json", config_cases(tb, rng))
    write("store.json", store_cases(tb))
    write("misc.json", {"iso": iso_cases(), "prune": prune_cases(tb, rng), "hotkey": hotkey_cases(tb),
                        **text_cases(tb, panel)})
    bookmark_files(tb)
    print("golden:", ", ".join(sorted(p.name for p in GOLDEN.glob("*.json"))))


if __name__ == "__main__":
    main()
