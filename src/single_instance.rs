//! Single-instance через именованный мьютекс Windows.
//!
//! Имя должно совпадать с `AppMutex` в installer/setup.iss — тогда установщик
//! сможет обнаружить запущенную копию при обновлении/удалении и предложить её закрыть.

use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Threading::CreateMutexW;

/// Имя мьютекса (Local-пространство имён). Должно совпадать с AppMutex в Inno Setup.
const MUTEX_NAME: &str = "ScreenSnip_SingleInstance";

/// Пытается захватить «слот» единственного экземпляра.
/// Возвращает `false`, если другая копия уже запущена.
///
/// Дескриптор мьютекса намеренно не закрывается: он живёт всё время процесса,
/// удерживая имя; ОС освободит его при завершении процесса.
pub fn acquire() -> bool {
    let name: Vec<u16> = MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            // Не смогли создать мьютекс — не блокируем запуск.
            return true;
        }
        // GetLastError нужно прочитать сразу после CreateMutexW.
        windows_sys::Win32::Foundation::GetLastError() != ERROR_ALREADY_EXISTS
    }
}
