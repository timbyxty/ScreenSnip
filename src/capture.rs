//! Захват всех мониторов и склейка их в единый буфер размером с виртуальный экран.
//!
//! Предполагается одинаковый масштаб (DPI) на всех мониторах — тогда координаты
//! мониторов и захваченные пиксели находятся в одной физической системе координат,
//! и склейка по смещениям даёт пиксельно точный результат.

use anyhow::{anyhow, Result};
use xcap::Monitor;

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
                out.push(((p >> 16) & 0xFF) as u8); // R
                out.push(((p >> 8) & 0xFF) as u8); // G
                out.push((p & 0xFF) as u8); // B
                out.push(0xFF); // A
            }
        }
        (out, w, h)
    }
}

/// Затемняет пиксель 0RGB до ~40% яркости.
#[inline]
fn dim(p: u32) -> u32 {
    let r = (((p >> 16) & 0xFF) * 40 / 100) << 16;
    let g = (((p >> 8) & 0xFF) * 40 / 100) << 8;
    let b = (p & 0xFF) * 40 / 100;
    r | g | b
}

/// Захватывает все мониторы и собирает единый замороженный снимок.
pub fn capture_all() -> Result<Composite> {
    let monitors = Monitor::all().map_err(|e| anyhow!("не удалось перечислить мониторы: {e}"))?;
    if monitors.is_empty() {
        return Err(anyhow!("не найдено ни одного монитора"));
    }

    // Границы виртуального экрана.
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
        // Захват одного монитора: image::RgbaImage (RGBA, построчно).
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

    let dimmed = bright.iter().map(|&p| dim(p)).collect();

    Ok(Composite {
        bright,
        dimmed,
        width: vw,
        height: vh,
        origin_x: min_x,
        origin_y: min_y,
    })
}
