//! Чистая логика без Win32 (часть 1): плоский текст, поиск, геометрия, правила, клавиши, цвета, тексты.
//! Путь всегда `crate::core::…` (имя совпадает со стандартным `core`).

pub mod color;
pub mod flatten;
pub mod geometry;
pub mod hotkey;
pub mod matching;
pub mod ribbon;
pub mod rules;
pub mod text;

pub use color::Colors;
pub use flatten::FlatDoc;
pub use hotkey::HotkeyError;
pub use rules::ScopeKeyHash;
