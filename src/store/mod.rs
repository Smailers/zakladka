//! Хранение (часть 1): config.json, bookmarks.json, журналы, местное время, id.
//! Остальные части зовут функции плоско: `store::load_config`, `store::log_error`, …

pub mod bookmarks;
pub mod clock;
pub mod config;
pub mod log;

pub use bookmarks::{load_bookmarks, prune, save_bookmarks, valid_bookmark};
pub use clock::{iso_seconds, new_id, now_local, parse_iso, LocalTime};
pub use config::{default_config, ensure_config, load_config, parse_config, save_config_keys};
pub use log::{install_panic_hook, log_error, log_render, log_timing};
