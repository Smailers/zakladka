//! Модель окна цветов без окон (часть 8; N 1132–1398): значения, готовые цвета, «Авто/Чёрный/
//! Белый/Другой…», сброс, текст о контрасте, результат. `ColorModel` — на усмотрение части 8.
//!
//! Окно (`color_dialog.rs`) только рисует модель и передаёт ей щелчки; всё, что проверяют тесты
//! `ColorDialogTests` Python-версии, живёт здесь.

use crate::core::color::{clean_colors, contrast_ratio, label_color, Colors, OPACITY_RANGE, PRESETS};
use crate::core::text::APP_TITLE;
use crate::types::{LabelColor, Rgb};

/// Цвета «По умолчанию» (N 29): жёлтая заливка 30 %, оранжевый акцент, надпись — авто.
pub const DEFAULT_COLORS: Colors = Colors {
    color: Rgb(0xFF, 0xD4, 0x00),
    opacity: 0.3,
    bar_color: Rgb(0xFF, 0x7A, 0x00),
    label_color: LabelColor::Auto,
};

/// Образец текста в «Как будет выглядеть» (N 1136).
pub const PREVIEW_TEXT: &str = "Здесь вы остановились";

/// Пояснения окна (N 1177–1295).
pub const TIP_SLIDER: &str = "20 % — едва заметно, 60 % — ярко";
pub const TIP_OTHER: &str = "Выбрать любой цвет";
pub const TIP_PIPETTE: &str = "Взять цвет с экрана: щёлкните по нужной точке; Esc — отмена";
pub const TIP_RESET: &str = "Жёлтая заливка 30 %, оранжевый акцент, надпись — авто";

/// Цвет с образцами: заливка (`color`) или акцент (`bar_color`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Field {
    Fill,
    Accent,
}

/// Кнопки «Цвет надписи на плашке» (N 1188–1190).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LabelChoice {
    Auto,
    Black,
    White,
    Custom,
}

impl LabelChoice {
    pub const ALL: [LabelChoice; 4] = [LabelChoice::Auto, LabelChoice::Black, LabelChoice::White, LabelChoice::Custom];

    pub fn caption(self) -> &'static str {
        match self {
            LabelChoice::Auto => "Авто",
            LabelChoice::Black => "Чёрный",
            LabelChoice::White => "Белый",
            LabelChoice::Custom => "Другой…",
        }
    }

    /// Пояснение кнопки; "" — без пояснения.
    pub fn tip(self) -> &'static str {
        match self {
            LabelChoice::Auto => "Чёрный или белый — что лучше читается на акценте",
            LabelChoice::Custom => "Выбрать любой цвет надписи",
            LabelChoice::Black | LabelChoice::White => "",
        }
    }
}

/// Что выбирается системным окном «Другой…» (`_choose`, N 1306–1310).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pick {
    Field(Field),
    Label,
}

const BLACK: Rgb = Rgb(0, 0, 0);
const WHITE: Rgb = Rgb(0xFF, 0xFF, 0xFF);

/// Значения окна цветов. Плотность хранится целыми процентами, как `IntVar` ползунка в Python.
#[derive(Clone, PartialEq, Debug)]
pub struct ColorModel {
    values: Colors,
    percent: u32,
    pipette: Option<Field>,
}

impl ColorModel {
    /// Начальные значения — проверенные `clean_colors` (N 1140), плотность — в процентах (N 1169).
    pub fn new(current: Colors) -> ColorModel {
        let values = clean_colors(&current);
        ColorModel { percent: percent_of(values.opacity), values, pipette: None }
    }

    /// Готовый результат (N 1239–1242): плотность = проценты / 100 с округлением до 0,01.
    /// Для целых процентов `p / 100.0` уже ближайшее к десятичной дроби число (как `round(p/100, 2)`).
    pub fn result(&self) -> Colors {
        Colors { opacity: self.percent as f64 / 100.0, ..self.values }
    }

    pub fn get(&self, field: Field) -> Rgb {
        match field {
            Field::Fill => self.values.color,
            Field::Accent => self.values.bar_color,
        }
    }

    /// `_set` (N 1299–1304): цвет уже разобран, поэтому всегда принимается.
    pub fn set(&mut self, field: Field, color: Rgb) {
        match field {
            Field::Fill => self.values.color = color,
            Field::Accent => self.values.bar_color = color,
        }
    }

    pub fn opacity_percent(&self) -> u32 {
        self.percent
    }

    /// Ползунок «Плотность»: значение в пределах `OPACITY_RANGE`. `true` — значение изменилось.
    pub fn set_opacity_percent(&mut self, percent: i64) -> bool {
        let (low, high) = OPACITY_RANGE;
        let percent = percent.clamp(low as i64, high as i64) as u32;
        let changed = percent != self.percent;
        self.percent = percent;
        changed
    }

    /// Надпись справа от ползунка (N 1365).
    pub fn opacity_text(&self) -> String {
        format!("{} %", self.percent)
    }

    pub fn label(&self) -> LabelColor {
        self.values.label_color
    }

    pub fn set_label(&mut self, label: LabelColor) {
        self.values.label_color = label;
    }

    /// Кнопка «Авто/Чёрный/Белый» (N 1326–1331). «Другой…» здесь ничего не меняет и возвращает
    /// `false`: цвет выбирает системное окно (`Pick::Label`).
    pub fn choose_label(&mut self, choice: LabelChoice) -> bool {
        let label = match choice {
            LabelChoice::Auto => LabelColor::Auto,
            LabelChoice::Black => LabelColor::Custom(BLACK),
            LabelChoice::White => LabelColor::Custom(WHITE),
            LabelChoice::Custom => return false,
        };
        self.values.label_color = label;
        true
    }

    /// Какая кнопка надписи выбрана (N 1359–1362): чёрный и белый — свои кнопки, иной — «Другой…».
    pub fn selected_label(&self) -> LabelChoice {
        match self.values.label_color {
            LabelColor::Auto => LabelChoice::Auto,
            LabelColor::Custom(BLACK) => LabelChoice::Black,
            LabelColor::Custom(WHITE) => LabelChoice::White,
            LabelColor::Custom(_) => LabelChoice::Custom,
        }
    }

    /// Цвет надписи на плашке на самом деле (N 1333–1335).
    pub fn label_actual(&self) -> Rgb {
        match self.values.label_color {
            LabelColor::Auto => label_color(self.values.bar_color),
            LabelColor::Custom(c) => c,
        }
    }

    /// Текст о контрасте и «хорошо ли» (N 1337–1344).
    pub fn contrast_text(&self) -> (String, bool) {
        let accent = self.values.bar_color;
        let ratio = contrast_ratio(accent, self.label_actual());
        let text = format!("Контраст {ratio:.1}:1").replace('.', ",");
        if ratio >= 4.5 {
            return (text + " · Хорошо читается", true);
        }
        let better = if label_color(accent) == BLACK { "чёрный" } else { "белый" };
        (format!("{text} · Плохо читается — лучше {better}"), false)
    }

    /// Номер выбранного готового цвета (кольцо на образце), `None` — свой цвет.
    pub fn selected_preset(&self, field: Field) -> Option<usize> {
        let value = self.get(field);
        PRESETS.iter().position(|(c, _)| *c == value)
    }

    /// Свой цвет (не из готовых) — показывается отдельным образцом перед «Другой…» (N 1353–1358).
    pub fn custom_swatch(&self, field: Field) -> Option<Rgb> {
        match self.selected_preset(field) {
            Some(_) => None,
            None => Some(self.get(field)),
        }
    }

    /// Пояснение своего образца (N 1287).
    pub fn custom_tip(&self, field: Field) -> String {
        format!("Свой цвет {}", self.get(field).hex())
    }

    /// «По умолчанию» (N 1252–1255).
    pub fn reset(&mut self) {
        self.values = DEFAULT_COLORS;
        self.percent = percent_of(DEFAULT_COLORS.opacity);
    }

    /// Начальный цвет системного окна (N 1307): «Авто» — чёрный или белый по акценту.
    pub fn choose_start(&self, pick: Pick) -> Rgb {
        match pick {
            Pick::Field(field) => self.get(field),
            Pick::Label => self.label_actual(),
        }
    }

    /// Заголовок системного окна (N 1308): «Закладка — цвет» / «Закладка — цвет надписи».
    pub fn choose_title(&self, pick: Pick) -> String {
        let title = match pick {
            Pick::Field(_) => "цвет",
            Pick::Label => "цвет надписи",
        };
        format!("{APP_TITLE} — {title}")
    }

    /// Цвет из системного окна.
    pub fn apply_pick(&mut self, pick: Pick, color: Rgb) {
        match pick {
            Pick::Field(field) => self.set(field, color),
            Pick::Label => self.values.label_color = LabelColor::Custom(color),
        }
    }

    /// Пипетка запущена (N 1323–1324): окно прячется до её конца.
    pub fn pipette_started(&mut self, field: Field) {
        self.pipette = Some(field);
    }

    /// Пипетка кончилась (`done`, N 1313–1319): окно возвращается; цвет — в то поле, для которого
    /// её запускали. `None` — отмена. Возвращает `true`, если пипетка была запущена.
    pub fn pipette_finished(&mut self, color: Option<Rgb>) -> bool {
        let Some(field) = self.pipette.take() else { return false };
        if let Some(color) = color {
            self.set(field, color);
        }
        true
    }

    /// Окно спрятано на время пипетки.
    pub fn hidden(&self) -> bool {
        self.pipette.is_some()
    }
}

/// `round(opacity * 100)` в пределах `OPACITY_RANGE`.
fn percent_of(opacity: f64) -> u32 {
    let (low, high) = OPACITY_RANGE;
    let value = (opacity * 100.0).round_ties_even();
    if value.is_nan() {
        return low;
    }
    value.clamp(low as f64, high as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent_of(0.3), 30);
        assert_eq!(percent_of(0.45), 45);
        assert_eq!(percent_of(0.05), 20);
        assert_eq!(percent_of(f64::NAN), 20);
        let mut m = ColorModel::new(DEFAULT_COLORS);
        assert!(m.set_opacity_percent(99));
        assert_eq!(m.result().opacity, 0.6);
        assert!(!m.set_opacity_percent(61));
        assert!(m.set_opacity_percent(-5));
        assert_eq!(m.opacity_text(), "20 %");
    }

    #[test]
    fn titles_and_starts() {
        let m = ColorModel::new(DEFAULT_COLORS);
        assert_eq!(m.choose_title(Pick::Field(Field::Fill)), "Закладка — цвет");
        assert_eq!(m.choose_title(Pick::Label), "Закладка — цвет надписи");
        assert_eq!(m.choose_start(Pick::Label), Rgb(0, 0, 0)); // авто на оранжевом — чёрный
        assert_eq!(m.choose_start(Pick::Field(Field::Accent)), Rgb(0xFF, 0x7A, 0x00));
    }
}
