//! Нативный Wayland-оверлей через `wlr-layer-shell`.

use std::borrow::Cow;
use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::io::{FromRawFd, OwnedFd, AsRawFd};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use image::ImageEncoder;
use memmap2::MmapMut;
use nix::sys::memfd::{memfd_create, MFdFlags};
use nix::unistd::ftruncate;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_compositor;
use wayland_client::protocol::wl_buffer;
use wayland_client::protocol::wl_keyboard;
use wayland_client::protocol::wl_output;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_shm_pool;
use wayland_client::protocol::wl_surface;
use wayland_backend::protocol::WEnum;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    self, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, ZwlrLayerSurfaceV1, KeyboardInteractivity,
};

use crate::capture::{Composite, OutputRect};

// ============================================================================

struct SurfaceData {
    surface: wl_surface::WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
    rect: OutputRect,
    width: u32, height: u32,
    pool: Option<wl_shm_pool::WlShmPool>,
    mmap: Option<MmapMut>,
    buf_size: usize,
    frame: usize,
    configured: bool,
}

#[allow(dead_code)]
struct OverlayState {
    qh: QueueHandle<OverlayState>,
    shm: wl_shm::WlShm,
    surfaces: Vec<SurfaceData>,
    seat: Option<WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_surface: Option<usize>,
    cursor_x: f64, cursor_y: f64,
    start_x: f64, start_y: f64,
    dragging: bool,
    dirty: bool, done: bool, cancelled: bool,
    /// Финальное выделение — сохраняется при отпускании до сброса dragging
    final_sel: Option<(u32, u32, u32, u32)>,
    composite: Composite,
    save_path: Option<PathBuf>,
}

// ============================================================================
// Dispatch impls
// ============================================================================

macro_rules! ignore_dispatch {
    ($t:ty, $ev:ty) => {
        impl Dispatch<$t, ()> for OverlayState {
            fn event(
                _: &mut OverlayState, _: &$t, _: $ev,
                _: &(), _: &Connection, _: &QueueHandle<OverlayState>,
            ) {}
        }
    };
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for OverlayState {
    fn event(
        _: &mut OverlayState, _: &wl_registry::WlRegistry, _: wl_registry::Event,
        _: &GlobalListContents, _: &Connection, _: &QueueHandle<OverlayState>,
    ) {}
}

ignore_dispatch!(wl_output::WlOutput, wl_output::Event);
ignore_dispatch!(wl_compositor::WlCompositor, wl_compositor::Event);
ignore_dispatch!(wl_shm::WlShm, wl_shm::Event);
ignore_dispatch!(wl_shm_pool::WlShmPool, wl_shm_pool::Event);
ignore_dispatch!(wl_buffer::WlBuffer, wl_buffer::Event);
ignore_dispatch!(wl_surface::WlSurface, wl_surface::Event);
ignore_dispatch!(ZwlrLayerShellV1, zwlr_layer_shell_v1::Event);

impl Dispatch<ZwlrLayerSurfaceV1, ()> for OverlayState {
    fn event(
        s: &mut OverlayState, proxy: &ZwlrLayerSurfaceV1,
        ev: zwlr_layer_surface_v1::Event, _: &(), _: &Connection,
        _qh: &QueueHandle<OverlayState>,
    ) {
        match ev {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                let surf = s.surfaces.iter_mut().find(|x| x.layer_surface == *proxy).unwrap();
                surf.configured = true;
                proxy.ack_configure(serial);
                let (w, h) = if width > 0 && height > 0 { (width, height) } else { (surf.rect.w, surf.rect.h) };
                surf.width = w; surf.height = h;
                if surf.pool.is_none() {
                    let stride = (w * 4) as i32;
                    let one = (stride * h as i32) as usize;
                    let pool_size = 2 * one;
                    if let Ok((pool, mmap)) = create_double_pool(&s.shm, w, h, stride, pool_size, _qh) {
                        surf.pool = Some(pool);
                        surf.mmap = Some(mmap);
                        surf.buf_size = one;
                        surf.frame = 0;
                    }
                }
                s.dirty = true;
            }
            zwlr_layer_surface_v1::Event::Closed => { s.done = true; s.cancelled = true; }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for OverlayState {
    fn event(
        s: &mut OverlayState, seat: &WlSeat, ev: wl_seat::Event, _: &(),
        _: &Connection, qh: &QueueHandle<OverlayState>,
    ) {
        match ev {
            wl_seat::Event::Capabilities { capabilities } => {
                if let WEnum::Value(caps) = capabilities {
                    if caps.contains(wl_seat::Capability::Pointer) {
                        s.pointer = Some(seat.get_pointer(qh, ()));
                    }
                    if caps.contains(wl_seat::Capability::Keyboard) {
                        s.keyboard = Some(seat.get_keyboard(qh, ()));
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for OverlayState {
    fn event(
        s: &mut OverlayState, _: &wl_pointer::WlPointer, ev: wl_pointer::Event, _: &(),
        _: &Connection, _: &QueueHandle<OverlayState>,
    ) {
        match ev {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => {
                let idx = s.surfaces.iter().position(|x| x.surface == surface);
                s.cursor_surface = idx;
                if let Some(i) = idx {
                    let r = &s.surfaces[i].rect;
                    s.cursor_x = r.x as f64 + surface_x;
                    s.cursor_y = r.y as f64 + surface_y;
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some(i) = s.cursor_surface {
                    let r = &s.surfaces[i].rect;
                    s.cursor_x = r.x as f64 + surface_x;
                    s.cursor_y = r.y as f64 + surface_y;
                    if s.dragging { s.dirty = true; }
                }
            }
            wl_pointer::Event::Button { state, .. } => {
                if let WEnum::Value(btn) = state {
                    match btn {
                        wl_pointer::ButtonState::Pressed => {
                            s.dragging = true; s.start_x = s.cursor_x; s.start_y = s.cursor_y;
                            s.dirty = true;
                        }
                        wl_pointer::ButtonState::Released if s.dragging => {
                            // Сохранить финальное выделение ДО сброса dragging
                            let sx = s.start_x.floor() as u32;
                            let sy = s.start_y.floor() as u32;
                            let cx = s.cursor_x.floor() as u32;
                            let cy = s.cursor_y.floor() as u32;
                            let x0 = sx.min(cx); let y0 = sy.min(cy);
                            let x1 = sx.max(cx); let y1 = sy.max(cy);
                            if x1 > x0 && y1 > y0 {
                                s.final_sel = Some((x0, y0, x1, y1));
                            }
                            s.done = true; s.cancelled = false; s.dragging = false;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for OverlayState {
    fn event(
        s: &mut OverlayState, _: &wl_keyboard::WlKeyboard, ev: wl_keyboard::Event, _: &(),
        _: &Connection, _: &QueueHandle<OverlayState>,
    ) {
        if let wl_keyboard::Event::Key { state, .. } = ev {
            if let WEnum::Value(ks) = state {
                if let wl_keyboard::KeyState::Pressed = ks { s.done = true; s.cancelled = true; }
            }
        }
    }
}

// ============================================================================
// SHM helpers
// ============================================================================

fn create_shm_fd(size: usize) -> Result<OwnedFd> {
    let fd = memfd_create("screensnip", MFdFlags::empty())
        .map_err(|e| anyhow!("memfd_create: {e}"))?;
    ftruncate(&fd, size as i64).map_err(|e| anyhow!("ftruncate: {e}"))?;
    Ok(fd)
}

fn create_double_pool(
    shm: &wl_shm::WlShm, _w: u32, _h: u32, _stride: i32, pool_size: usize,
    qh: &QueueHandle<OverlayState>,
) -> Result<(wl_shm_pool::WlShmPool, MmapMut)> {
    let fd = create_shm_fd(pool_size)?;
    let dup = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 { return Err(anyhow!("fd dup")); }
    let mmap_file = unsafe { File::from_raw_fd(dup) };
    let mmap = unsafe { MmapMut::map_mut(&mmap_file) }.map_err(|e| anyhow!("mmap: {e}"))?;
    let pool = shm.create_pool(fd.as_fd(), pool_size as i32, qh, ());
    Ok((pool, mmap))
}

fn make_buffer(
    pool: &wl_shm_pool::WlShmPool, offset: i32, w: u32, h: u32, stride: i32,
    qh: &QueueHandle<OverlayState>,
) -> wl_buffer::WlBuffer {
    pool.create_buffer(offset, w as i32, h as i32, stride, wl_shm::Format::Argb8888, qh, ())
}

// ============================================================================
// Rendering
// ============================================================================

const BORDER: u32 = 0x4F_C3_F7;

fn current_sel(state: &OverlayState) -> Option<(u32, u32, u32, u32)> {
    if !state.dragging { return None; }
    let sx = state.start_x.floor() as u32; let sy = state.start_y.floor() as u32;
    let cx = state.cursor_x.floor() as u32; let cy = state.cursor_y.floor() as u32;
    let x0 = sx.min(cx); let x1 = sx.max(cx);
    let y0 = sy.min(cy); let y1 = sy.max(cy);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

fn put(mmap: &mut [u8], stride: usize, x: usize, y: usize, color: u32) {
    let di = y * stride + x * 4;
    if di + 3 < mmap.len() {
        mmap[di] = (color & 0xFF) as u8;
        mmap[di + 1] = ((color >> 8) & 0xFF) as u8;
        mmap[di + 2] = ((color >> 16) & 0xFF) as u8;
        mmap[di + 3] = 255;
    }
}

/// Рендерит dimmed/selection в новую половину mmap и коммитит на surface.
fn render_and_commit(state: &mut OverlayState, idx: usize) {
    let rect = state.surfaces[idx].rect;
    let w = rect.w as usize; let h = rect.h as usize;
    let comp = &state.composite;
    let ox = (rect.x - comp.origin_x).max(0) as usize;
    let oy = (rect.y - comp.origin_y).max(0) as usize;
    let sel_c = current_sel(state);
    let stride = w;

    let qh = state.qh.clone();
    let surf = &mut state.surfaces[idx];
    let pool = match surf.pool.as_ref() { Some(p) => p, None => return };
    let mmap = match surf.mmap.as_mut() { Some(m) => m, None => return };

    let half = surf.buf_size;
    let offset = (surf.frame % 2) * half;
    let buf_slice = &mut mmap[offset..offset + half];

    // Рендерим пиксели
    for y in 0..h {
        let src = (oy + y) * comp.width as usize + ox;
        let dst = y * stride * 4;
        for x in 0..w {
            let gx = ox + x; let gy = oy + y;
            let bright = sel_c.is_some_and(|(sx0, sy0, sx1, sy1)|
                gx >= sx0 as usize && gx < sx1 as usize && gy >= sy0 as usize && gy < sy1 as usize);
            let p = if bright { comp.bright[src + x] } else { comp.dimmed[src + x] };
            let di = dst + x * 4;
            buf_slice[di] = (p & 0xFF) as u8;
            buf_slice[di + 1] = ((p >> 8) & 0xFF) as u8;
            buf_slice[di + 2] = ((p >> 16) & 0xFF) as u8;
            buf_slice[di + 3] = 255;
        }
    }

    // Border
    if let Some((sx0, sy0, sx1, sy1)) = sel_c {
        let rx0 = (sx0 as usize).max(ox); let ry0 = (sy0 as usize).max(oy);
        let rx1 = (sx1 as usize).min(ox + w); let ry1 = (sy1 as usize).min(oy + h);
        if rx0 < rx1 && ry0 < ry1 {
            let (lx0, ly0, lx1, ly1) = (rx0 - ox, ry0 - oy, rx1 - ox, ry1 - oy);
            for x in lx0..lx1 { put(buf_slice, stride, x, ly0, BORDER); put(buf_slice, stride, x, ly1 - 1, BORDER); }
            for y in ly0..ly1 { put(buf_slice, stride, lx0, y, BORDER); put(buf_slice, stride, lx1 - 1, y, BORDER); }
        }
    }

    // Создать новый буфер и закоммитить
    let buf = make_buffer(pool, offset as i32, surf.width, surf.height, (surf.width * 4) as i32, &qh);
    surf.surface.attach(Some(&buf), 0, 0);
    surf.surface.damage(0, 0, i32::MAX, i32::MAX);
    surf.surface.commit();
    surf.frame += 1;
}

// ============================================================================
// Public API
// ============================================================================

#[allow(dead_code)]
pub struct OverlayWayland {
    conn: Connection,
    event_queue: EventQueue<OverlayState>,
    state: OverlayState,
}

impl OverlayWayland {
    pub fn new(composite: Composite, save_path: Option<PathBuf>) -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut event_queue) =
            registry_queue_init::<OverlayState>(&conn).map_err(|e| anyhow!("registry: {e}"))?;
        let qh = event_queue.handle();

        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=5, ()).unwrap();
        let seat: WlSeat = globals.bind(&qh, 7..=9, ()).unwrap();

        let mut surfaces = Vec::new();
        for rect in &composite.outputs {
            let surface = compositor.create_surface(&qh, ());
            let ls = layer_shell.get_layer_surface(
                &surface, None,
                zwlr_layer_shell_v1::Layer::Overlay,
                "screensnip".to_string(), &qh, (),
            );
            ls.set_exclusive_zone(-1);
            ls.set_anchor(Anchor::all());
            ls.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
            surface.commit();
            surfaces.push(SurfaceData {
                surface, layer_surface: ls, rect: *rect,
                width: rect.w, height: rect.h,
                pool: None, mmap: None, buf_size: 0, frame: 0,
                configured: false,
            });
        }

        let mut state = OverlayState {
            qh,
            shm, surfaces, seat: Some(seat),
            pointer: None, keyboard: None,
            cursor_surface: None, cursor_x: 0.0, cursor_y: 0.0,
            start_x: 0.0, start_y: 0.0, dragging: false,
            dirty: false, done: false, cancelled: false,
            final_sel: None,
            composite, save_path,
        };
        event_queue.roundtrip(&mut state).map_err(|e| anyhow!("init rt: {e}"))?;

        Ok(Self { conn, event_queue, state })
    }

    pub fn run(&mut self) -> Result<()> {
        // Ждём, пока все поверхности сконфигурированы и имеют mmap
        loop {
            if self.state.surfaces.iter().all(|s| s.configured && s.mmap.is_some()) { break; }
            self.event_queue.blocking_dispatch(&mut self.state)
                .map_err(|e| anyhow!("init: {e}"))?;
        }

        // Первоначальный рендер
        for i in 0..self.state.surfaces.len() { render_and_commit(&mut self.state, i); }
        self.state.dirty = false;

        // Главный цикл
        loop {
            if self.state.done { break; }
            self.event_queue.blocking_dispatch(&mut self.state)
                .map_err(|e| anyhow!("dispatch: {e}"))?;
            if self.state.dirty {
                for i in 0..self.state.surfaces.len() { render_and_commit(&mut self.state, i); }
                self.state.dirty = false;
            }
        }

        // Обработка результата
        if !self.state.cancelled {
            if let Some(sel) = self.state.final_sel.take() {
                let (rgba, w, h) = self.state.composite.crop_rgba(sel.0, sel.1, sel.2, sel.3);
                if let Some(ref p) = self.state.save_path {
                    let f = std::fs::File::create(p)?;
                    image::codecs::png::PngEncoder::new(f)
                        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
                        .map_err(|e| anyhow!("png: {e}"))?;
                } else {
                    let mut c = arboard::Clipboard::new()?;
                    c.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: Cow::Owned(rgba) })?;
                }
            }
        }

        // Cleanup
        for surf in &self.state.surfaces {
            surf.surface.attach(None as Option<&wl_buffer::WlBuffer>, 0, 0);
            surf.surface.commit();
        }
        let _ = self.event_queue.roundtrip(&mut self.state);
        Ok(())
    }
}
