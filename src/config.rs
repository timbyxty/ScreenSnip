//! Загрузка/сохранение конфигурации и разбор строки горячей клавиши.

use anyhow::{anyhow, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Горячая клавиша, например "Ctrl+Shift+S" или "PrintScreen".
    pub hotkey: String,
    /// Запускать программу автоматически при входе в Windows.
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Shift+S".to_string(),
            autostart: true,
        }
    }
}

/// Путь к файлу конфигурации: %APPDATA%\ScreenSnip\config.toml
pub fn config_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "ScreenSnip", "ScreenSnip")
        .ok_or_else(|| anyhow!("не удалось определить каталог конфигурации"))?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// Загружает конфиг; если файла нет — создаёт с настройками по умолчанию.
pub fn load_or_create() -> Result<Config> {
    let path = config_path()?;
    if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    } else {
        let cfg = Config::default();
        save(&cfg)?;
        Ok(cfg)
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(cfg)?)?;
    Ok(())
}

/// Разбирает строку вида "Ctrl+Shift+S" в HotKey.
/// Модификаторы: Ctrl/Control, Shift, Alt, Win/Super/Meta.
/// Клавиша: одиночная буква/цифра, F1..F24, либо имя кода (KeyS, Digit1, PrintScreen...).
pub fn parse_hotkey(s: &str) -> Result<HotKey> {
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;

    for raw in s.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "ctl" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" => mods |= Modifiers::ALT,
            "win" | "super" | "meta" | "cmd" | "windows" => mods |= Modifiers::META,
            _ => {
                if code.is_some() {
                    return Err(anyhow!("в хоткее '{s}' указано более одной клавиши"));
                }
                code = Some(parse_code(token)?);
            }
        }
    }

    let code = code.ok_or_else(|| anyhow!("в хоткее '{s}' не указана основная клавиша"))?;
    Ok(HotKey::new(Some(mods), code))
}

fn parse_code(token: &str) -> Result<Code> {
    // Одиночная буква a-z -> KeyA..KeyZ
    if token.len() == 1 {
        let c = token.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphabetic() {
            return code_from_name(&format!("Key{c}"));
        }
        if c.is_ascii_digit() {
            return code_from_name(&format!("Digit{c}"));
        }
    }
    // F1..F24
    let upper = token.to_ascii_uppercase();
    if let Some(num) = upper.strip_prefix('F') {
        if num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty() {
            return code_from_name(&format!("F{num}"));
        }
    }
    // Имя кода как есть (KeyS, Digit1, PrintScreen, Insert, Home, ...).
    code_from_name(token)
}

/// Code реализует FromStr (через keyboard-types) с именами вариантов.
fn code_from_name(name: &str) -> Result<Code> {
    // Нормализуем популярные синонимы.
    let normalized = match name.to_ascii_lowercase().as_str() {
        "printscreen" | "prtsc" | "print" => "PrintScreen",
        "insert" | "ins" => "Insert",
        "delete" | "del" => "Delete",
        "esc" | "escape" => "Escape",
        "space" => "Space",
        "enter" | "return" => "Enter",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        other => {
            // Иначе используем имя в исходном виде (ожидается формат keyboard-types).
            let _ = other;
            return name
                .parse::<Code>()
                .map_err(|_| anyhow!("неизвестная клавиша: '{name}'"));
        }
    };
    normalized
        .parse::<Code>()
        .map_err(|_| anyhow!("неизвестная клавиша: '{name}'"))
}
