//! Ожидание с пробуждением и монотонное время.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateEventW, CreateWaitableTimerExW, SetEvent, SetWaitableTimer, WaitForMultipleObjects,
    WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
};

/// Короче — высокоточный таймер: обычный таймаут ожидания округляется до шага системного
/// таймера (15,6 мс), и шаг 16 мс превращается в 31 мс.
const PRECISE_BELOW: f64 = 0.05;
/// Остаток меньше этого считается истёкшим (таймер может сработать на доли мс раньше).
const SLACK: f64 = 0.000_2;
/// Самое длинное ожидание одним вызовом, мс (INFINITE = u32::MAX не используется).
const MAX_WAIT_MS: f64 = 4_000_000_000.0;

/// Событие + высокоточный таймер. `Arc<Wake>`, Send + Sync (проверяется в lib.rs).
///
/// Дескрипторы хранятся числами (`isize`), поэтому тип сам `Send + Sync`: дескрипторы объектов
/// ядра действительны во всех потоках процесса, `SetEvent` и `WaitFor*` потокобезопасны.
/// Таймер переводят только под `timer` (Mutex): два ожидающих потока не перебивают друг другу срок.
pub struct Wake {
    /// Событие с автосбросом: `signal` до `wait` не теряется, `wait` его поглощает.
    /// 0 — создать не удалось (тогда `wait` просто спит).
    event: isize,
    /// Высокоточный таймер (Windows 10 1803+); 0 — нет, ожидание обычным таймаутом.
    timer: Mutex<isize>,
}

fn handle(value: isize) -> HANDLE {
    HANDLE(value as *mut c_void)
}

impl Wake {
    pub fn new() -> Wake {
        // SAFETY: создание объектов ядра без имени и атрибутов; ошибки дают 0.
        let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .map(|h| h.0 as isize)
        .unwrap_or(0);
        Wake { event, timer: Mutex::new(timer) }
    }

    pub fn signal(&self) {
        if self.event != 0 {
            // SAFETY: собственный действительный дескриптор события.
            unsafe {
                let _ = SetEvent(handle(self.event));
            }
        }
    }

    /// true = разбудили; < 50 мс — CREATE_WAITABLE_TIMER_HIGH_RESOLUTION.
    pub fn wait(&self, seconds: f64) -> bool {
        let seconds = if seconds > 0.0 { seconds.min(MAX_WAIT_MS / 1000.0) } else { 0.0 }; // NaN -> 0
        if self.event == 0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
            return false;
        }
        if seconds > 0.0 && seconds < PRECISE_BELOW {
            if let Ok(timer) = self.timer.try_lock() {
                if *timer != 0 {
                    if let Some(woken) = self.wait_precise(*timer, seconds) {
                        return woken;
                    }
                }
            }
        }
        let ms = (seconds * 1000.0).ceil() as u32;
        // SAFETY: собственный действительный дескриптор события.
        unsafe { WaitForSingleObject(handle(self.event), ms) == WAIT_OBJECT_0 }
    }

    /// Ожидание события до срока по высокоточному таймеру; `None` — таймер не работает
    /// (тогда `wait` ждёт обычным таймаутом).
    fn wait_precise(&self, timer: isize, seconds: f64) -> Option<bool> {
        let deadline = monotonic() + seconds;
        let handles = [handle(self.event), handle(timer)];
        loop {
            let left = deadline - monotonic();
            if left <= SLACK {
                // Срок вышел: событие могло прийти в последний миг.
                // SAFETY: собственный действительный дескриптор события.
                return Some(unsafe { WaitForSingleObject(handles[0], 0) } == WAIT_OBJECT_0);
            }
            // Относительный срок — отрицательное число в единицах 100 нс.
            let due = -((left * 10_000_000.0).ceil() as i64).max(1);
            // SAFETY: собственный дескриптор таймера, без APC-процедуры.
            unsafe { SetWaitableTimer(handles[1], &due, 0, None, None, false) }.ok()?;
            // Страховочный таймаут на случай, если таймер не сработает.
            let guard_ms = (left * 1000.0).ceil() as u32 + 50;
            // SAFETY: оба дескриптора собственные и действительны всё время жизни `self`.
            let result = unsafe { WaitForMultipleObjects(&handles, false, guard_ms) };
            if result == WAIT_OBJECT_0 {
                return Some(true);
            }
            if result.0 != WAIT_OBJECT_0.0 + 1 && result != WAIT_TIMEOUT {
                return None; // WAIT_FAILED
            }
            // Таймер (или страховка) сработал: следующий круг сверит срок по часам —
            // устаревший сигнал таймера только перезапустит ожидание на остаток.
        }
    }
}

impl Default for Wake {
    fn default() -> Wake {
        Wake::new()
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        let timer = match self.timer.get_mut() {
            Ok(t) => *t,
            Err(poisoned) => *poisoned.into_inner(),
        };
        for value in [self.event, timer] {
            if value != 0 {
                // SAFETY: собственные дескрипторы, закрываются ровно один раз.
                unsafe {
                    let _ = CloseHandle(handle(value));
                }
            }
        }
    }
}

/// Частота QueryPerformanceCounter (постоянна с загрузки системы).
fn frequency() -> f64 {
    static FREQUENCY: OnceLock<f64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut value = 0i64;
        // SAFETY: запись в собственную переменную.
        let ok = unsafe { QueryPerformanceFrequency(&mut value) }.is_ok();
        if ok && value > 0 {
            value as f64
        } else {
            1.0
        }
    })
}

/// Секунды, QueryPerformanceCounter.
pub fn monotonic() -> f64 {
    let mut counter = 0i64;
    // SAFETY: запись в собственную переменную; на Windows XP+ вызов не завершается ошибкой.
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
    }
    counter as f64 / frequency()
}
