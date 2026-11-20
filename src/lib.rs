//! «Закладка» — модули программы. `main.rs` только вызывает их.
//! Библиотека (а не одни `mod` в `main.rs`) нужна, чтобы тесты `tests\*.rs` видели модули.
//!
//! Внимание: модуль называется `core`, поэтому внутри программы писать `crate::core::…`,
//! а не `core::…` (иначе — неоднозначность со стандартным `core`); не делать `use crate::*;`.

pub mod app_id; // Claude
pub mod types; // Claude

pub mod core; // часть 1
pub mod store; // часть 1
pub mod win; // часть 2
pub mod ocr; // часть 3
pub mod uia; // часть 3
pub mod tracker; // state.rs — Claude; части 4 и 5
pub mod selftest; // часть 5
pub mod ui; // части 6, 7, 8

// Договор о потоках (раздел 4 плана): проверяется компилятором. Если часть меняет внутреннее
// устройство типа и он перестаёт быть Send/Sync — сборка падает здесь, а не в чужом файле.
const _: () = {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    #[allow(dead_code)]
    fn contracts() {
        send_sync::<crate::win::Api>(); // Arc<Api> у всех потоков
        send_sync::<crate::win::Wake>(); // Arc<Wake> у tracker и locator
        send_sync::<crate::uia::Uia>(); // Arc<Uia> у tracker и locator
        send_sync::<crate::uia::Element>();
        send_sync::<crate::uia::TextPattern>();
        send_sync::<crate::types::Shared>();
        send_sync::<crate::types::CommandSender>();
        send_sync::<crate::types::EventSink>();
        send::<crate::types::Command>();
        send::<crate::types::UiEvent>();
        send::<crate::types::OverlayWindows>(); // UI -> tracker::spawn
        send::<crate::types::PieceBindings>();
        send::<crate::uia::Segment>();
        send::<crate::uia::ScopeKey>();
        send::<crate::tracker::locator::Request>();
        send::<crate::tracker::locator::ScanResult>();
        send::<crate::tracker::locator::LocatorHandle>();
    }
};
