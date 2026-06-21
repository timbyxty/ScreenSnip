//! Захват всех мониторов и склейка их в единый буфер размером с виртуальный экран.
//!
//! Поддерживается два бэкенда:
//! - X11 / Windows: `xcap`
//! - Wayland (Linux): `libwayshot` (wlr-screencopy)

use anyhow::{anyhow, Result};

/// Прямоугольник одного физического выхода (монитора).
/// Координаты — в пикселях виртуального рабочего стола.
#[derive(Debug, Clone, Copy)]
pub struct OutputRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Замороженный снимок всего виртуального рабочего стола.
pub struct Composite {
    /// Яркие пиксели (оригинал) в формате 0RGB, построчно сверху вниз.
    pub bright: Vec<u32>,
    /// Затемнённая копия (для области вне выделения).
    pub dimmed: Vec<u32>,
    /// Размеры виртуального экрана в физических пикселях.
    pub width: u32,
    pub height: u32,
    /// Левый верхний угол виртуального экрана в координатах рабочего стола
    /// (может быть отрицательным при наличии мониторов слева/сверху от основного).
    pub origin_x: i32,
    pub origin_y: i32,
    /// Прямоугольники отдельных мониторов (для оверлея на Wayland).
    pub outputs: Vec<OutputRect>,
}

impl Composite {
    /// Вырезает прямоугольную область и возвращает её как плотный RGBA-буфер
    /// (по 4 байта на пиксель), пригодный для буфера обмена.
    /// Координаты — в пикселях композита (0..width, 0..height).
    pub fn crop_rgba(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> (Vec<u8>, u32, u32) {
        let w = x1.saturating_sub(x0);
        let h = y1.saturating_sub(y0);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        let stride = self.width as usize;
        for y in y0..y1 {
            let row = y as usize * stride;
            for x in x0..x1 {
                let p = self.bright[row + x as usize];
                out.push(((p >> 16) & 0xFF) as u8);
                out.push(((p >> 8) & 0xFF) as u8);
                out.push((p & 0xFF) as u8);
                out.push(0xFF);
            }
        }
        (out, w, h)
    }
}

/// Затемняет пиксель 0RGB до ~40% яркости.
#[inline]
pub(crate) fn dim(p: u32) -> u32 {
    let r = (((p >> 16) & 0xFF) * 40 / 100) << 16;
    let g = (((p >> 8) & 0xFF) * 40 / 100) << 8;
    let b = (p & 0xFF) * 40 / 100;
    r | g | b
}

/// Определяет, запущены ли мы под Wayland (по переменным окружения).
#[cfg(target_os = "linux")]
pub fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE").map_or(false, |v| v == "wayland")
}

/// Захватывает весь виртуальный рабочий стол. Автоматически выбирает бэкенд.
pub fn capture_all() -> Result<Composite> {
    #[cfg(target_os = "linux")]
    if is_wayland() {
        return capture_wayland();
    }
    capture_x11()
}

// ============================================================================
// Бэкенд X11 / Windows (через xcap)
// ============================================================================

fn capture_x11() -> Result<Composite> {
    use xcap::Monitor;

    let monitors = Monitor::all().map_err(|e| anyhow!("не удалось перечислить мониторы: {e}"))?;
    if monitors.is_empty() {
        return Err(anyhow!("не найдено ни одного монитора"));
    }

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for m in &monitors {
        let x = m.x()?;
        let y = m.y()?;
        let w = m.width()? as i32;
        let h = m.height()? as i32;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }
    let vw = (max_x - min_x).max(1) as u32;
    let vh = (max_y - min_y).max(1) as u32;
    let stride = vw as usize;

    let mut bright = vec![0u32; stride * vh as usize];

    for m in &monitors {
        let mx = m.x()?;
        let my = m.y()?;
        let img = m
            .capture_image()
            .map_err(|e| anyhow!("не удалось захватить монитор: {e}"))?;
        let iw = img.width();
        let ih = img.height();
        let raw: Vec<u8> = img.into_raw();

        let ox = (mx - min_x) as usize;
        let oy = (my - min_y) as usize;

        for row in 0..ih as usize {
            let dst_y = oy + row;
            if dst_y >= vh as usize {
                break;
            }
            let src_base = row * iw as usize * 4;
            let dst_base = dst_y * stride + ox;
            for col in 0..iw as usize {
                if ox + col >= stride {
                    break;
                }
                let si = src_base + col * 4;
                let r = raw[si] as u32;
                let g = raw[si + 1] as u32;
                let b = raw[si + 2] as u32;
                bright[dst_base + col] = (r << 16) | (g << 8) | b;
            }
        }
    }

    let outputs: Vec<OutputRect> = monitors
        .iter()
        .filter_map(|m| {
            let x = m.x().ok()?;
            let y = m.y().ok()?;
            let w = m.width().ok()?;
            let h = m.height().ok()?;
            Some(OutputRect { x, y, w, h })
        })
        .collect();

    let dimmed = bright.iter().map(|&p| dim(p)).collect();

    Ok(Composite {
        bright,
        dimmed,
        width: vw,
        height: vh,
        origin_x: min_x,
        origin_y: min_y,
        outputs,
    })
}

// ============================================================================
// Бэкенд Wayland (через libwayshot / wlr-screencopy)
// ============================================================================

#[cfg(target_os = "linux")]
fn capture_wayland() -> Result<Composite> {
    use libwayshot::WayshotConnection;

    let wayshot = WayshotConnection::new().map_err(|e| anyhow!("Wayland захват: {e}"))?;

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for output in wayshot.get_all_outputs() {
        let x = output.logical_region.inner.position.x;
        let y = output.logical_region.inner.position.y;
        let w = output.physical_size.width;
        let h = output.physical_size.height;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w as i32);
        max_y = max_y.max(y + h as i32);
    }

    let img = wayshot
        .screenshot_all(true)
        .map_err(|e| anyhow!("screenshot_all: {e}"))?;

    let rgba = img.to_rgba8();
    let iw = rgba.width();
    let ih = rgba.height();
    let raw = rgba.into_raw();

    let bright: Vec<u32> = raw
        .chunks_exact(4)
        .map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
        .collect();

    let outputs: Vec<OutputRect> = wayshot
        .get_all_outputs()
        .iter()
        .map(|o| OutputRect {
            x: o.logical_region.inner.position.x,
            y: o.logical_region.inner.position.y,
            w: o.physical_size.width,
            h: o.physical_size.height,
        })
        .collect();

    let vw = iw.max(1);
    let vh = ih.max(1);
    let dimmed = bright.iter().map(|&p| dim(p)).collect();

    Ok(Composite {
        bright,
        dimmed,
        width: vw,
        height: vh,
        origin_x: min_x,
        origin_y: min_y,
        outputs,
    })
}
