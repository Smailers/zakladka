#![windows_subsystem = "windows"]
//! Вход программы «Закладка»: аргументы, мьютекс профиля, DPI, папка данных, настройки, запуск UI.
//! Порядок — раздел 4 плана, сценарий «Запуск». Пишет и меняет только Claude.
//!
//!   Zakladka.exe                    рабочий профиль
//!   Zakladka.exe --dev              профиль разработки (своя папка, клавиши, мьютекс)
//!   Zakladka.exe [--dev] --selftest <режим> … [--out <файл>]   самопроверка (selftest::run)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::Threading::CreateMutexW;

use zakladka::app_id::{self, Profile};
use zakladka::types::Shared;
use zakladka::{selftest, store, ui, win};

fn main() {
    // args_os: аргумент с неверным UTF-16 не роняет программу.
    let args: Vec<String> =
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    // Самопроверка — как в Python: без мьютекса и без окон; DPI и COM она включает сама.
    // Ей передаётся всё, что после "--selftest".
    if let Some(at) = args.iter().position(|a| a == "--selftest") {
        return selftest::run(&args[at + 1..]);
    }

    let profile = app_id::profile(args);
    // SAFETY: обычный вызов Win32; имя живёт до конца вызова.
    let mutex = match unsafe { CreateMutexW(None, false, &HSTRING::from(profile.mutex)) } {
        Ok(handle) => handle,
        Err(_) => return 1,
    };
    // SAFETY: читается сразу после CreateMutexW, между ними других вызовов Win32 нет.
    let duplicate = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let code = if duplicate { 0 } else { start(profile) };
    // SAFETY: дескриптор получен выше и закрывается один раз.
    unsafe {
        let _ = CloseHandle(mutex);
    }
    code
}

fn start(profile: &'static Profile) -> i32 {
    // До любого окна: PER_MONITOR_AWARE_V2 (запасной путь — shcore), манифеста нет.
    win::Api::enable_dpi();

    let data_dir = data_dir(profile);
    let _ = std::fs::create_dir_all(&data_dir);
    let config_path = data_dir.join("config.json");
    store::ensure_config(&config_path, profile);
    let config = store::load_config(&config_path, profile);

    let shared = Arc::new(Shared {
        debug: AtomicBool::new(config.debug),
        config: RwLock::new(config),
        profile,
        data_dir,
    });

    // Один раз на процесс: MTA живёт всё время работы; потоки tracker и locator сами
    // вызывают CoInitializeEx(COINIT_MULTITHREADED). Отметку не возвращаем — до выхода.
    // SAFETY: обычный вызов COM.
    if unsafe { CoIncrementMTAUsage() }.is_err() {
        store::log_error(&shared, "MTA_USAGE");
    }

    store::install_panic_hook(shared.clone());

    let api = Arc::new(win::Api::new());
    ui::run(shared, api, config_path)
}

/// `%LOCALAPPDATA%\<profile.data_folder>`; без LOCALAPPDATA — `%USERPROFILE%\AppData\Local`.
fn data_dir(profile: &Profile) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join(profile.data_folder)
}
