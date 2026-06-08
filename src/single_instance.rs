//! Single-instance: только одна копия программы одновременно.
//!
//! Windows — именованный мьютекс (его же имя видит установщик через AppMutex).
//! Unix — эксклюзивная блокировка lock-файла (flock).

/// Возвращает `false`, если другая копия уже запущена.
#[cfg(windows)]
pub fn acquire() -> bool {
    use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    // Должно совпадать с AppMutex в installer/setup.iss.
    const MUTEX_NAME: &str = "ScreenSnip_SingleInstance";

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
        // Дескриптор намеренно не закрываем: держит имя всё время процесса.
        windows_sys::Win32::Foundation::GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Возвращает `false`, если другая копия уже запущена.
#[cfg(unix)]
pub fn acquire() -> bool {
    use fs2::FileExt;

    let path = std::env::temp_dir().join("screensnip.lock");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => match file.try_lock_exclusive() {
            // Блокировку держим до конца процесса — «забываем» File, чтобы fd не закрылся.
            Ok(()) => {
                std::mem::forget(file);
                true
            }
            Err(_) => false,
        },
        // Не смогли открыть файл блокировки — не мешаем запуску.
        Err(_) => true,
    }
}
