//! Win32 (часть 2): окна, процессы, снимок экрана, буфер обмена, значок, ожидание.
//! Методы `Api` разнесены по файлам: api.rs (окна, процессы, DPI, ввод), capture.rs (`capture`),
//! clipboard.rs (`copy_selection`), icon.rs (`make_icon`).

pub mod api;
pub mod capture;
pub mod clipboard;
pub mod icon;
pub mod wake;

pub use api::Api;
pub use capture::Bitmap;
pub use wake::{monotonic, Wake};
