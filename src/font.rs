//! Минимальный растровый шрифт 5×7 для подписи размера выделения и координат.
//! Каждый глиф — 7 строк; в строке используются младшие 5 бит (бит 4 — левый столбец).

const GW: usize = 5;
const GH: usize = 7;

fn glyph(c: char) -> Option<[u8; GH]> {
    Some(match c {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 6, 8, 16, 31],
        '3' => [31, 1, 2, 6, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        'x' => [0, 0, 17, 10, 4, 10, 17],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        '-' => [0, 0, 0, 14, 0, 0, 0],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => return None,
    })
}

/// Ширина строки в пикселях при заданном масштабе.
pub fn text_width(s: &str, scale: usize) -> usize {
    s.chars().count() * (GW + 1) * scale
}

/// Высота одной строки в пикселях.
pub fn text_height(scale: usize) -> usize {
    GH * scale
}

/// Рисует строку в буфере 0RGB.
pub fn draw_text(buf: &mut [u32], stride: usize, x: i32, y: i32, s: &str, scale: usize, color: u32) {
    let mut cx = x;
    for ch in s.chars() {
        if let Some(g) = glyph(ch) {
            for (row, bits) in g.iter().enumerate() {
                for col in 0..GW {
                    if bits & (1 << (GW - 1 - col)) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let px = cx + (col * scale + dx) as i32;
                                let py = y + (row * scale + dy) as i32;
                                if px >= 0 && py >= 0 {
                                    let i = py as usize * stride + px as usize;
                                    if (px as usize) < stride && i < buf.len() {
                                        buf[i] = color;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        cx += ((GW + 1) * scale) as i32;
    }
}
