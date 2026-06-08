// Без консольного окна в релизе.
#![windows_subsystem = "windows"]

mod autostart;
mod capture;
mod config;
mod font;
mod hotkey_ui;
mod overlay;
mod single_instance;

use anyhow::Result;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowId;

use config::Config;
use hotkey_ui::HotkeyCapture;
use overlay::Overlay;

/// События, доставляемые в event loop из фоновых потоков.
#[derive(Debug)]
enum UserEvent {
    /// Нажата глобальная горячая клавиша.
    Hotkey,
    /// Выбран пункт меню в трее.
    Menu(MenuId),
    /// Левый клик по иконке в трее.
    TrayClick,
}

struct App {
    overlay: Option<Overlay>,
    capture: Option<HotkeyCapture>,
    // Менеджер хоткеев держим живым всё время (иначе хоткей снимется).
    hotkeys: GlobalHotKeyManager,
    /// Текущий зарегистрированный хоткей (нужен, чтобы снять при смене).
    current_hotkey: HotKey,
    tray: Option<TrayIcon>,
    change_id: Option<MenuId>,
    open_id: Option<MenuId>,
    quit_id: Option<MenuId>,
    cfg: Config,
}

impl App {
    fn new(hotkeys: GlobalHotKeyManager, current_hotkey: HotKey, cfg: Config) -> Self {
        Self {
            overlay: None,
            capture: None,
            hotkeys,
            current_hotkey,
            tray: None,
            change_id: None,
            open_id: None,
            quit_id: None,
            cfg,
        }
    }

    fn open_overlay(&mut self, event_loop: &ActiveEventLoop) {
        // Не открываем во время настройки хоткея и повторно.
        if self.overlay.is_some() || self.capture.is_some() {
            return;
        }
        let comp = match capture::capture_all() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Ошибка захвата экрана: {e}");
                return;
            }
        };
        match Overlay::new(event_loop, comp) {
            Ok(overlay) => {
                overlay.window.request_redraw();
                self.overlay = Some(overlay);
            }
            Err(e) => eprintln!("Ошибка создания оверлея: {e}"),
        }
    }

    fn open_capture(&mut self, event_loop: &ActiveEventLoop) {
        if self.capture.is_some() || self.overlay.is_some() {
            return;
        }
        match HotkeyCapture::new(event_loop, &self.cfg.hotkey) {
            Ok(c) => {
                c.window.request_redraw();
                self.capture = Some(c);
            }
            Err(e) => eprintln!("Не удалось открыть окно настройки: {e}"),
        }
    }

    /// Применяет новую комбинацию: перерегистрирует хоткей вживую и сохраняет конфиг.
    fn apply_new_hotkey(&mut self, s: String) {
        let new = match config::parse_hotkey(&s) {
            Ok(h) => h,
            Err(e) => {
                if let Some(c) = self.capture.as_ref() {
                    c.set_error(&format!("Не понял клавишу: {e}"));
                }
                return;
            }
        };
        // Снимаем старый и ставим новый.
        let _ = self.hotkeys.unregister(self.current_hotkey);
        match self.hotkeys.register(new) {
            Ok(()) => {
                self.current_hotkey = new;
                self.cfg.hotkey = s.clone();
                if let Err(e) = config::save(&self.cfg) {
                    eprintln!("Не удалось сохранить конфиг: {e}");
                }
                if let Some(t) = &self.tray {
                    let _ = t.set_tooltip(Some(format!("ScreenSnip — {s}")));
                }
                self.capture = None; // успех — закрываем окно
            }
            Err(e) => {
                // Откат на старый хоткей, окно остаётся открытым.
                let _ = self.hotkeys.register(self.current_hotkey);
                if let Some(c) = self.capture.as_ref() {
                    c.set_error(&format!("«{s}» недоступна ({e}) — попробуйте другую"));
                }
            }
        }
    }

    fn handle_overlay_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Some(o) = self.overlay.as_mut() {
                    if let Err(e) = o.render() {
                        eprintln!("Ошибка отрисовки: {e}");
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(o) = self.overlay.as_mut() {
                    o.on_cursor_moved(position.x, position.y);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    if let Some(o) = self.overlay.as_mut() {
                        o.on_left_press();
                    }
                }
                (MouseButton::Left, ElementState::Released) => {
                    let result = self.overlay.as_mut().map(|o| o.on_left_release());
                    match result {
                        Some(Ok(true)) => self.overlay = None,
                        Some(Ok(false)) => {}
                        Some(Err(e)) => {
                            eprintln!("Ошибка копирования: {e}");
                            self.overlay = None;
                        }
                        None => {}
                    }
                }
                (MouseButton::Right, ElementState::Pressed) => self.overlay = None,
                _ => {}
            },
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.overlay = None,
            WindowEvent::CloseRequested => self.overlay = None,
            _ => {}
        }
    }

    fn handle_capture_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Some(c) = self.capture.as_mut() {
                    if let Err(e) = c.render() {
                        eprintln!("Ошибка отрисовки: {e}");
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                if let Some(c) = self.capture.as_mut() {
                    c.mods = m.state();
                    c.update_title();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key,
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                // Esc — отмена настройки.
                if matches!(logical_key, Key::Named(NamedKey::Escape)) {
                    self.capture = None;
                    return;
                }
                if let PhysicalKey::Code(code) = physical_key {
                    let mods = self
                        .capture
                        .as_ref()
                        .map(|c| c.mods)
                        .unwrap_or_else(ModifiersState::empty);
                    if let Some(s) = hotkey_ui::chord_string(mods, code) {
                        self.apply_new_hotkey(s);
                    }
                }
            }
            WindowEvent::CloseRequested => self.capture = None,
            _ => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // Иконку в трее создаём один раз, уже внутри запущенного event loop.
        if self.tray.is_none() {
            match build_tray(&self.cfg) {
                Ok((tray, change_id, open_id, quit_id)) => {
                    self.tray = Some(tray);
                    self.change_id = Some(change_id);
                    self.open_id = Some(open_id);
                    self.quit_id = Some(quit_id);
                }
                Err(e) => eprintln!("Не удалось создать иконку в трее: {e}"),
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey => self.open_overlay(event_loop),
            UserEvent::TrayClick => self.open_capture(event_loop),
            UserEvent::Menu(id) => {
                if self.quit_id.as_ref() == Some(&id) {
                    event_loop.exit();
                } else if self.change_id.as_ref() == Some(&id) {
                    self.open_capture(event_loop);
                } else if self.open_id.as_ref() == Some(&id) {
                    open_config_file();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .capture
            .as_ref()
            .map_or(false, |c| c.window.id() == window_id)
        {
            self.handle_capture_event(event);
        } else if self
            .overlay
            .as_ref()
            .map_or(false, |o| o.window.id() == window_id)
        {
            self.handle_overlay_event(event);
        }
    }
}

/// Создаёт иконку в трее с меню и возвращает идентификаторы пунктов.
fn build_tray(cfg: &Config) -> Result<(TrayIcon, MenuId, MenuId, MenuId)> {
    let menu = Menu::new();
    let change_item = MenuItem::new("Изменить хоткей…", true, None);
    let open_item = MenuItem::new("Открыть конфиг", true, None);
    let quit_item = MenuItem::new("Выход", true, None);
    menu.append(&change_item)?;
    menu.append(&open_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let (rgba, w, h) = make_icon();
    let icon = tray_icon::Icon::from_rgba(rgba, w, h)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip(format!("ScreenSnip — {}", cfg.hotkey))
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()?;

    Ok((
        tray,
        change_item.id().clone(),
        open_item.id().clone(),
        quit_item.id().clone(),
    ))
}

/// Простая иконка 32×32: синий квадрат с белой рамкой.
fn make_icon() -> (Vec<u8>, u32, u32) {
    const S: u32 = 32;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            let border = x < 2 || y < 2 || x >= S - 2 || y >= S - 2;
            let (r, g, b) = if border {
                (255, 255, 255)
            } else {
                (0x2D, 0x9C, 0xDB)
            };
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    (rgba, S, S)
}

/// Открывает файл конфигурации в редакторе по умолчанию.
fn open_config_file() {
    if let Ok(path) = config::config_path() {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
}

fn main() -> Result<()> {
    // Если уже запущена другая копия — тихо выходим (иначе два трея и конфликт хоткея).
    if !single_instance::acquire() {
        return Ok(());
    }

    let cfg = config::load_or_create().unwrap_or_else(|e| {
        eprintln!("Не удалось прочитать конфиг ({e}), использую значения по умолчанию");
        Config::default()
    });

    // Автозапуск (управляется только приложением, не инсталлятором).
    if let Err(e) = autostart::apply(cfg.autostart) {
        eprintln!("Не удалось настроить автозапуск: {e}");
    }

    // Регистрация глобального хоткея.
    let hotkey = config::parse_hotkey(&cfg.hotkey).unwrap_or_else(|e| {
        eprintln!("Ошибка в хоткее ({e}), использую Ctrl+Shift+S");
        config::parse_hotkey("Ctrl+Shift+S").unwrap()
    });
    let hotkeys = GlobalHotKeyManager::new()?;
    hotkeys.register(hotkey)?;

    // Event loop с пользовательскими событиями.
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    // Мост: горячая клавиша -> пробуждение event loop.
    // receiver() отдаёт &'static mpsc::Receiver (не Clone, не Sync), поэтому
    // получаем ссылку уже ВНУТРИ потока и блокирующе ждём событий.
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let rx = GlobalHotKeyEvent::receiver();
            while let Ok(ev) = rx.recv() {
                if ev.state == HotKeyState::Pressed {
                    let _ = proxy.send_event(UserEvent::Hotkey);
                }
            }
        });
    }
    // Мост: пункты меню трея -> event loop.
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let rx = MenuEvent::receiver();
            while let Ok(ev) = rx.recv() {
                let _ = proxy.send_event(UserEvent::Menu(ev.id));
            }
        });
    }
    // Мост: клики по иконке трея -> event loop (левый клик открывает настройку).
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let rx = TrayIconEvent::receiver();
            while let Ok(ev) = rx.recv() {
                if let TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = ev
                {
                    let _ = proxy.send_event(UserEvent::TrayClick);
                }
            }
        });
    }

    let mut app = App::new(hotkeys, hotkey, cfg);
    event_loop.run_app(&mut app)?;
    Ok(())
}
