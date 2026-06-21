#![windows_subsystem = "windows"]

mod autostart;
mod capture;
mod cli;
mod config;
mod font;
mod hotkey_ui;
mod overlay;
mod single_instance;

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use image::ImageEncoder;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowId;

use cli::Cli;
use config::Config;
use hotkey_ui::HotkeyCapture;
use overlay::Overlay;

const ID_CHANGE: &str = "change_hotkey";
const ID_OPEN: &str = "open_config";
const ID_QUIT: &str = "quit";

#[derive(Debug)]
enum UserEvent {
    Hotkey,
    Menu(MenuId),
    TrayClick,
}

struct App {
    overlay: Option<Overlay>,
    capture: Option<HotkeyCapture>,
    hotkeys: GlobalHotKeyManager,
    current_hotkey: HotKey,
    tray: Option<TrayIcon>,
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
            cfg,
        }
    }

    fn open_overlay(&mut self, event_loop: &ActiveEventLoop) {
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
                self.capture = None;
            }
            Err(e) => {
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
        #[cfg(windows)]
        if self.tray.is_none() {
            match make_tray() {
                Ok(tray) => self.tray = Some(tray),
                Err(e) => eprintln!("Не удалось создать иконку в трее: {e}"),
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey => self.open_overlay(event_loop),
            UserEvent::TrayClick => self.open_capture(event_loop),
            UserEvent::Menu(id) => {
                if id == MenuId::new(ID_QUIT) {
                    event_loop.exit();
                } else if id == MenuId::new(ID_CHANGE) {
                    self.open_capture(event_loop);
                } else if id == MenuId::new(ID_OPEN) {
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

fn make_tray() -> Result<TrayIcon> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(ID_CHANGE, "Изменить хоткей…", true, None))?;
    menu.append(&MenuItem::with_id(ID_OPEN, "Открыть конфиг", true, None))?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(ID_QUIT, "Выход", true, None))?;

    let (rgba, w, h) = make_icon();
    let icon = tray_icon::Icon::from_rgba(rgba, w, h)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("ScreenSnip")
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()?;

    Ok(tray)
}

fn make_icon() -> (Vec<u8>, u32, u32) {
    const S: u32 = 32;
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            let border = x < 2 || y < 2 || x >= S - 2 || y >= S - 2;
            let (r, g, b) = if border { (255, 255, 255) } else { (0x2D, 0x9C, 0xDB) };
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    (rgba, S, S)
}

fn open_config_file() {
    let Ok(path) = config::config_path() else {
        return;
    };
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
}

// ============================================================================
// CLI overlay app — упрощённый event loop для режима --region
// ============================================================================

struct CliApp {
    overlay: Option<Overlay>,
    comp: Option<capture::Composite>,
    save_path: Option<PathBuf>,
}

impl ApplicationHandler for CliApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.overlay.is_some() {
            return;
        }
        let Some(comp) = self.comp.take() else {
            event_loop.exit();
            return;
        };
        match Overlay::with_output(event_loop, comp, self.save_path.clone()) {
            Ok(overlay) => {
                overlay.window.request_redraw();
                self.overlay = Some(overlay);
            }
            Err(e) => {
                eprintln!("Ошибка создания оверлея: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_overlay = self
            .overlay
            .as_ref()
            .map_or(false, |o| o.window.id() == window_id);
        if !is_overlay {
            return;
        }

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
                    let done = match result {
                        Some(Ok(true)) => true,
                        Some(Ok(false)) => false,
                        Some(Err(e)) => {
                            eprintln!("Ошибка: {e}");
                            true
                        }
                        None => false,
                    };
                    if done {
                        event_loop.exit();
                    }
                }
                (MouseButton::Right, ElementState::Pressed) => event_loop.exit(),
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
            } => event_loop.exit(),
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

// ============================================================================
// CLI mode: --fullscreen / --region
// ============================================================================

fn run_cli(cli: Cli) -> Result<()> {
    if cli.delay > 0 {
        std::thread::sleep(std::time::Duration::from_secs(cli.delay as u64));
    }

    let comp = capture::capture_all()?;

    if cli.fullscreen {
        let (rgba, w, h) = comp.crop_rgba(0, 0, comp.width, comp.height);
        if let Some(ref path) = cli.output {
            save_png(path, &rgba, w, h)?;
        }
        if cli.clipboard || cli.output.is_none() {
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: Cow::Owned(rgba),
            })?;
        }
        return Ok(());
    }

    // X11 / Windows: winit-based overlay
    let event_loop = EventLoop::<()>::with_user_event().build()?;
    let mut app = CliApp {
        overlay: None,
        comp: Some(comp),
        save_path: cli.output.map(PathBuf::from),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn save_png(path: &str, rgba: &[u8], w: u32, h: u32) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let encoder = image::codecs::png::PngEncoder::new(file);
    encoder
        .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| anyhow!("png encode: {e}"))?;
    Ok(())
}

// ============================================================================
// Daemon mode (tray + hotkey) — как было
// ============================================================================

fn run_daemon() -> Result<()> {
    if !single_instance::acquire() {
        return Ok(());
    }

    let cfg = config::load_or_create().unwrap_or_else(|e| {
        eprintln!("Не удалось прочитать конфиг ({e}), использую значения по умолчанию");
        Config::default()
    });

    if let Err(e) = autostart::apply(cfg.autostart) {
        eprintln!("Не удалось настроить автозапуск: {e}");
    }

    let hotkey = config::parse_hotkey(&cfg.hotkey).unwrap_or_else(|e| {
        eprintln!("Ошибка в хоткее ({e}), использую Ctrl+Shift+S");
        config::parse_hotkey("Ctrl+Shift+S").unwrap()
    });
    let hotkeys = GlobalHotKeyManager::new()?;
    hotkeys.register(hotkey)?;

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

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
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let rx = MenuEvent::receiver();
            while let Ok(ev) = rx.recv() {
                let _ = proxy.send_event(UserEvent::Menu(ev.id));
            }
        });
    }
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

    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        if let Err(e) = gtk::init() {
            eprintln!("Не удалось инициализировать gtk для трея: {e}");
            return;
        }
        let _tray = match make_tray() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Не удалось создать иконку в трее: {e}");
                return;
            }
        };
        gtk::main();
    });

    let mut app = App::new(hotkeys, hotkey, cfg);
    event_loop.run_app(&mut app)?;
    Ok(())
}

// ============================================================================
// Entry point
// ============================================================================

fn main() -> Result<()> {
    let cli = <Cli as clap::Parser>::parse();

    if cli.is_cli_mode() {
        run_cli(cli)
    } else {
        run_daemon()
    }
}
