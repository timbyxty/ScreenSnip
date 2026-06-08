//! Управление автозапуском через HKCU\Software\Microsoft\Windows\CurrentVersion\Run.

use anyhow::{anyhow, Result};
use auto_launch::AutoLaunchBuilder;

const APP_NAME: &str = "ScreenSnip";

/// Приводит состояние автозапуска в соответствие с настройкой.
/// Идемпотентно: безопасно вызывать при каждом старте.
pub fn apply(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe = exe.to_str().ok_or_else(|| anyhow!("путь к exe не в UTF-8"))?;

    let auto = AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(exe)
        .build()
        .map_err(|e| anyhow!("autostart build: {e}"))?;

    let currently = auto.is_enabled().unwrap_or(false);
    if enabled && !currently {
        auto.enable().map_err(|e| anyhow!("autostart enable: {e}"))?;
    } else if !enabled && currently {
        auto.disable().map_err(|e| anyhow!("autostart disable: {e}"))?;
    }
    Ok(())
}
