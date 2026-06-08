//! Маленькое окно для назначения новой горячей клавиши: ловит следующую
//! нажатую комбинацию. Весь текст выводится в заголовке окна (так не нужен
//! рендер шрифтов), клиентская область просто заливается тёмным цветом.

use std::num::NonZeroU32;
use std::rc::Rc;

use anyhow::{anyhow, Result};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::{Window, WindowLevel};

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// Цвет фона окна (0RGB).
const BG: u32 = 0x20_22_25;

pub struct HotkeyCapture {
    pub window: Rc<Window>,
    _context: softbuffer::Context<Rc<Window>>,
    surface: Surface,
    /// Текущие зажатые модификаторы.
    pub mods: ModifiersState,
    current: String,
}

impl HotkeyCapture {
    pub fn new(event_loop: &ActiveEventLoop, current_hotkey: &str) -> Result<Self> {
        let logical = LogicalSize::new(480.0, 110.0);

        // По центру основного монитора.
        let position = event_loop.primary_monitor().map(|m| {
            let scale = m.scale_factor();
            let wpx = (logical.width * scale) as i32;
            let hpx = (logical.height * scale) as i32;
            let ms = m.size();
            let mp = m.position();
            PhysicalPosition::new(
                mp.x + (ms.width as i32 - wpx) / 2,
                mp.y + (ms.height as i32 - hpx) / 2,
            )
        });

        let mut attrs = Window::default_attributes()
            .with_title("Назначение горячей клавиши")
            .with_inner_size(logical)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(true);
        if let Some(pos) = position {
            attrs = attrs.with_position(pos);
        }

        let window = Rc::new(event_loop.create_window(attrs)?);
        window.focus_window();

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow!("softbuffer surface: {e}"))?;

        let cap = Self {
            window,
            _context: context,
            surface,
            mods: ModifiersState::empty(),
            current: current_hotkey.to_string(),
        };
        cap.update_title();
        Ok(cap)
    }

    /// Обновляет заголовок: показывает текущий хоткей и набираемые модификаторы.
    pub fn update_title(&self) {
        let mut prefix = String::new();
        if self.mods.control_key() {
            prefix.push_str("Ctrl+");
        }
        if self.mods.alt_key() {
            prefix.push_str("Alt+");
        }
        if self.mods.shift_key() {
            prefix.push_str("Shift+");
        }
        if self.mods.super_key() {
            prefix.push_str("Win+");
        }

        let title = if prefix.is_empty() {
            format!(
                "Сейчас: {}   —   нажмите новую комбинацию (Esc — отмена)",
                self.current
            )
        } else {
            format!("{prefix}…   —   нажмите клавишу (Esc — отмена)")
        };
        self.window.set_title(&title);
    }

    /// Показать сообщение об ошибке в заголовке (окно остаётся открытым).
    pub fn set_error(&self, msg: &str) {
        self.window.set_title(msg);
    }

    pub fn render(&mut self) -> Result<()> {
        let size = self.window.inner_size();
        let w = size.width.max(1);
        let h = size.height.max(1);
        self.surface
            .resize(
                NonZeroU32::new(w).unwrap(),
                NonZeroU32::new(h).unwrap(),
            )
            .map_err(|e| anyhow!("resize: {e}"))?;
        let mut buf = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow!("buffer_mut: {e}"))?;
        buf.fill(BG);
        buf.present().map_err(|e| anyhow!("present: {e}"))?;
        Ok(())
    }
}

/// Строит строку хоткея (например "Ctrl+Shift+S") из модификаторов и кода
/// клавиши. Возвращает None, если нажата только клавиша-модификатор.
pub fn chord_string(mods: ModifiersState, code: KeyCode) -> Option<String> {
    let key = pretty_key(code)?;
    let mut parts: Vec<String> = Vec::new();
    if mods.control_key() {
        parts.push("Ctrl".into());
    }
    if mods.alt_key() {
        parts.push("Alt".into());
    }
    if mods.shift_key() {
        parts.push("Shift".into());
    }
    if mods.super_key() {
        parts.push("Win".into());
    }
    parts.push(key);
    Some(parts.join("+"))
}

/// Имя клавиши в дружелюбном виде. None — для чистых модификаторов.
fn pretty_key(code: KeyCode) -> Option<String> {
    use KeyCode::*;
    if matches!(
        code,
        ControlLeft | ControlRight | ShiftLeft | ShiftRight | AltLeft | AltRight | SuperLeft
            | SuperRight
    ) {
        return None;
    }
    // Debug-имя варианта совпадает с именами кодов keyboard-types:
    // KeyS, Digit1, F8, PrintScreen, Insert, ...
    let name = format!("{code:?}");
    let pretty = if let Some(letter) = name.strip_prefix("Key") {
        letter.to_string()
    } else if let Some(digit) = name.strip_prefix("Digit") {
        digit.to_string()
    } else {
        name
    };
    Some(pretty)
}
