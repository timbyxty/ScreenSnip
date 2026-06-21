//! Полноэкранный оверлей: показывает замороженный скриншот, позволяет выделить
//! прямоугольник мышью и копирует выделенную область в буфер обмена.

use std::borrow::Cow;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

use anyhow::{anyhow, Result};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, Window, WindowLevel};

use image::ImageEncoder;

use crate::capture::Composite;
use crate::font;

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// Цвет рамки выделения (формат 0RGB), светло-голубой.
const BORDER: u32 = 0x4F_C3_F7;

pub struct Overlay {
    pub window: Rc<Window>,
    _context: softbuffer::Context<Rc<Window>>,
    surface: Surface,
    comp: Composite,
    /// Точка начала перетаскивания (в пикселях композита).
    start: Option<(u32, u32)>,
    /// Текущая позиция курсора (в пикселях композита).
    cur: (u32, u32),
    dragging: bool,
    /// Если задан — сохранить скриншот в файл вместо буфера обмена.
    save_path: Option<PathBuf>,
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop, comp: Composite) -> Result<Self> {
        Self::with_output(event_loop, comp, None)
    }

    pub fn with_output(event_loop: &ActiveEventLoop, comp: Composite, save_path: Option<PathBuf>) -> Result<Self> {
        // Окно создаём СКРЫТЫМ: сначала отключим анимацию переходов и нарисуем
        // первый кадр, и только потом покажем — чтобы оверлей появлялся мгновенно,
        // без «выезда из центра» и без анимации закрытия.
        // `mut` нужен только под Windows (для with_skip_taskbar ниже).
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes()
            .with_title("ScreenSnip")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(comp.origin_x, comp.origin_y))
            .with_inner_size(PhysicalSize::new(comp.width, comp.height))
            .with_visible(false);

        // Не показывать оверлей в панели задач (только Windows).
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_skip_taskbar(true);
        }

        let window = Rc::new(event_loop.create_window(attrs)?);
        window.set_cursor(CursorIcon::Crosshair);

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow!("softbuffer surface: {e}"))?;

        let mut overlay = Self {
            window,
            _context: context,
            surface,
            comp,
            start: None,
            cur: (0, 0),
            dragging: false,
            save_path,
        };

        disable_window_transitions(&overlay.window);
        // Рисуем затемнённый кадр ещё в скрытое окно, затем показываем.
        overlay.render()?;
        overlay.window.set_visible(true);
        overlay.window.focus_window();

        Ok(overlay)
    }

    /// Обновляет позицию курсора (приходит в физических пикселях относительно
    /// левого верхнего угла окна, что совпадает с координатами композита).
    pub fn on_cursor_moved(&mut self, x: f64, y: f64) {
        let cx = (x.max(0.0) as u32).min(self.comp.width.saturating_sub(1));
        let cy = (y.max(0.0) as u32).min(self.comp.height.saturating_sub(1));
        self.cur = (cx, cy);
        if self.dragging {
            self.window.request_redraw();
        }
    }

    pub fn on_left_press(&mut self) {
        self.start = Some(self.cur);
        self.dragging = true;
        self.window.request_redraw();
    }

    /// Завершение выделения по отпусканию ЛКМ. Если область непустая — сохраняет
    /// в файл (если задан save_path) или копирует в буфер обмена.
    pub fn on_left_release(&mut self) -> Result<bool> {
        self.dragging = false;
        let Some((x0, y0, x1, y1)) = self.selection() else {
            return Ok(false);
        };
        let (rgba, w, h) = self.comp.crop_rgba(x0, y0, x1, y1);

        if let Some(ref path) = self.save_path {
            let file = std::fs::File::create(path)?;
            let encoder = image::codecs::png::PngEncoder::new(file);
            encoder
                .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| anyhow!("png encode: {e}"))?;
        } else {
            copy_image_to_clipboard(&rgba, w, h)?;
        }
        Ok(true)
    }

    /// Нормализованный прямоугольник выделения, если он непустой.
    fn selection(&self) -> Option<(u32, u32, u32, u32)> {
        let (sx, sy) = self.start?;
        let (cx, cy) = self.cur;
        let x0 = sx.min(cx);
        let x1 = sx.max(cx);
        let y0 = sy.min(cy);
        let y1 = sy.max(cy);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }

    pub fn render(&mut self) -> Result<()> {
        let w = self.comp.width;
        let h = self.comp.height;
        // Вычисляем выделение ДО взятия мутабельного буфера (иначе конфликт
        // заимствований: selection() берёт &self, а buffer_mut() — &mut self.surface).
        let sel = self.selection();

        self.surface
            .resize(
                NonZeroU32::new(w).ok_or_else(|| anyhow!("нулевая ширина"))?,
                NonZeroU32::new(h).ok_or_else(|| anyhow!("нулевая высота"))?,
            )
            .map_err(|e| anyhow!("resize: {e}"))?;

        let mut buf = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow!("buffer_mut: {e}"))?;

        // Фон — затемнённый скриншот.
        buf.copy_from_slice(&self.comp.dimmed);

        // Выделенная область — в полной яркости + рамка.
        if let Some((x0, y0, x1, y1)) = sel {
            let stride = w as usize;
            for y in y0..y1 {
                let row = y as usize * stride;
                let s = row + x0 as usize;
                let e = row + x1 as usize;
                buf[s..e].copy_from_slice(&self.comp.bright[s..e]);
            }
            for x in x0..x1 {
                put(&mut buf, stride, x, y0, BORDER);
                put(&mut buf, stride, x, y1 - 1, BORDER);
            }
            for y in y0..y1 {
                put(&mut buf, stride, x0, y, BORDER);
                put(&mut buf, stride, x1 - 1, y, BORDER);
            }

            // Подпись: размер выделения и координата верхнего левого угла
            // (в координатах рабочего стола, с учётом смещения виртуального экрана).
            let scale = 2usize;
            let line1 = format!("{} x {}", x1 - x0, y1 - y0);
            let line2 = format!(
                "X {}  Y {}",
                self.comp.origin_x + x0 as i32,
                self.comp.origin_y + y0 as i32
            );

            let pad = 5i32;
            let gap = 3i32;
            let line_h = font::text_height(scale) as i32;
            let tw = font::text_width(&line1, scale).max(font::text_width(&line2, scale)) as i32;
            let box_w = tw + pad * 2;
            let box_h = line_h * 2 + gap + pad * 2;

            // Над выделением; если сверху не помещается — внутрь сверху.
            let mut bx = x0 as i32;
            let mut by = y0 as i32 - box_h - 2;
            if by < 0 {
                by = y0 as i32 + 2;
            }
            if bx + box_w > w as i32 {
                bx = w as i32 - box_w;
            }
            if bx < 0 {
                bx = 0;
            }

            fill_rect(&mut buf, stride, bx, by, box_w, box_h, 0x16_18_1B);
            rect_border(&mut buf, stride, bx, by, box_w, box_h, BORDER);
            font::draw_text(&mut buf, stride, bx + pad, by + pad, &line1, scale, 0xFF_FF_FF);
            font::draw_text(
                &mut buf,
                stride,
                bx + pad,
                by + pad + line_h + gap,
                &line2,
                scale,
                0xD0_D4_D8,
            );
        }

        buf.present().map_err(|e| anyhow!("present: {e}"))?;
        Ok(())
    }
}

/// Отключает анимации переходов окна (открытие/закрытие) через DWM, чтобы оверлей
/// появлялся и исчезал мгновенно. На не-Windows платформах — заглушка.
#[cfg(windows)]
fn disable_window_transitions(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
    };

    if let Ok(handle) = window.window_handle() {
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd = h.hwnd.get() as HWND;
            let disable: i32 = 1;
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                    &disable as *const i32 as *const core::ffi::c_void,
                    core::mem::size_of::<i32>() as u32,
                );
            }
        }
    }
}

#[cfg(not(windows))]
fn disable_window_transitions(_window: &Window) {}

/// Заливает прямоугольник сплошным цветом.
fn fill_rect(buf: &mut [u32], stride: usize, x: i32, y: i32, bw: i32, bh: i32, color: u32) {
    for yy in y..y + bh {
        if yy < 0 {
            continue;
        }
        let row = yy as usize * stride;
        for xx in x..x + bw {
            if xx < 0 || xx as usize >= stride {
                continue;
            }
            let i = row + xx as usize;
            if i < buf.len() {
                buf[i] = color;
            }
        }
    }
}

/// Рисует рамку прямоугольника.
fn rect_border(buf: &mut [u32], stride: usize, x: i32, y: i32, bw: i32, bh: i32, color: u32) {
    for xx in x..x + bw {
        put_i(buf, stride, xx, y, color);
        put_i(buf, stride, xx, y + bh - 1, color);
    }
    for yy in y..y + bh {
        put_i(buf, stride, x, yy, color);
        put_i(buf, stride, x + bw - 1, yy, color);
    }
}

#[inline]
fn put_i(buf: &mut [u32], stride: usize, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x as usize >= stride {
        return;
    }
    let i = y as usize * stride + x as usize;
    if i < buf.len() {
        buf[i] = color;
    }
}

#[inline]
fn put(buf: &mut [u32], stride: usize, x: u32, y: u32, color: u32) {
    let i = y as usize * stride + x as usize;
    if i < buf.len() {
        buf[i] = color;
    }
}

/// Копирует RGBA в буфер обмена: arboard (X11/Windows), wl-copy (Wayland fallback).
pub(crate) fn copy_image_to_clipboard(rgba: &[u8], w: u32, h: u32) -> Result<(), anyhow::Error> {
    // Пробуем arboard
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if cb.set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: Cow::Borrowed(rgba),
        }).is_ok() {
            return Ok(());
        }
    }

    // Wayland fallback: wl-copy через PNG pipe
    let mut enc = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut enc);
        encoder.write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| anyhow::anyhow!("png encode for wl-copy: {e}"))?;
    }
    let mut cmd = Command::new("wl-copy")
        .arg("--type").arg("image/png")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("wl-copy (needs wl-clipboard): {e}"))?;
    if let Some(mut stdin) = cmd.stdin.take() {
        stdin.write_all(&enc).ok();
    }
    cmd.wait().ok();
    Ok(())
}
