//! Профили dev / prod (раздел 5.2 плана): папка данных, мьютекс, клавиши по умолчанию, акцент.
//! Пишет и меняет только Claude.

#[derive(Debug)]
pub struct Profile {
    pub data_folder: &'static str,
    pub mutex: &'static str,
    pub hotkey_mark: &'static str,
    pub hotkey_extra: &'static str,
    pub bar_color: &'static str,
    pub tray_suffix: &'static str,
}

/// Рабочий профиль. Мьютекс совпадает с мьютексом Python-версии: старая и новая
/// в рабочем режиме одновременно не запустятся.
pub const PROD: Profile = Profile {
    data_folder: "text-bookmark",
    mutex: "Local\\TextBookmark.v1",
    hotkey_mark: "Ctrl+Alt+Z",
    hotkey_extra: "Ctrl+Alt+Shift+Z",
    bar_color: "#FF7A00",
    tray_suffix: "",
};

/// Профиль разработки (`--dev`): своя папка, свой мьютекс, свои клавиши, зелёный акцент.
pub const DEV: Profile = Profile {
    data_folder: "text-bookmark-native",
    mutex: "Local\\TextBookmark.native.dev",
    hotkey_mark: "Ctrl+Alt+X",
    hotkey_extra: "Ctrl+Alt+Shift+X",
    bar_color: "#34C759",
    tray_suffix: " (новая)",
};

/// `"--dev"` среди аргументов -> `DEV`, иначе `PROD`.
pub fn profile(args: &[String]) -> &'static Profile {
    if args.iter().any(|a| a == "--dev") {
        &DEV
    } else {
        &PROD
    }
}
