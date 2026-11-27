//! Automatic Xbox <-> Keyboard & Mouse Input Glyphs system.
//!
//! Tracks the currently active physical input device (`KeyboardMouse` vs
//! `Gamepad`, updated lock-free from `victory-hle::modules::gameinput` without
//! modifying any control bindings) and renders a zero-VRAM, color-coded
//! Xbox-to-KBM keycap strip directly onto the swap-chain back buffer whenever
//! Keyboard & Mouse is active — automatically hiding the moment an XInput
//! gamepad is touched.
//!
//! Controlled by:
//! - `VICTORY_INPUT_GLYPHS=auto` (default): automatically shows KBM keycap
//!   glyphs when Keyboard/Mouse is active and hides them when Gamepad is used.
//! - `VICTORY_INPUT_GLYPHS=kbm`: always show KBM keycap glyphs.
//! - `VICTORY_INPUT_GLYPHS=xbox` or `0` / `off`: disabled (native Xbox glyphs).
//! - `VICTORY_GLYPH_MAP="RT=LMB,LT=RMB,R=MOUSE,A=SPACE"`: override displayed
//!   keycap labels (e.g. when using a mouse remapper) without touching controls.
//! - Hotkey **`F6`** cycles mode live (`AUTO -> KBM -> XBOX -> OFF`) while the
//!   game window has focus.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[cfg(windows)]
use std::sync::atomic::AtomicBool;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::*;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F6};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

#[cfg(windows)]
use crate::host::Host;

/// The physical input device most recently used by the player.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveDevice {
    KeyboardMouse = 0,
    Gamepad = 1,
}

/// User-selected glyph display mode (`VICTORY_INPUT_GLYPHS` / `F6`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphMode {
    Auto = 0,
    ForceKbm = 1,
    ForceXbox = 2,
    Off = 3,
}

static ACTIVE_DEVICE: AtomicU8 = AtomicU8::new(ActiveDevice::KeyboardMouse as u8);

fn mode_atomic() -> &'static AtomicU8 {
    static MODE: OnceLock<AtomicU8> = OnceLock::new();
    MODE.get_or_init(|| {
        let raw = std::env::var("VICTORY_INPUT_GLYPHS").ok();
        let initial = match raw.as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("0") | Some("off") | Some("false") => GlyphMode::Off,
            Some("xbox") | Some("pad") | Some("gamepad") => GlyphMode::ForceXbox,
            Some("kbm") | Some("kb") | Some("keyboard") | Some("1") => GlyphMode::ForceKbm,
            _ => GlyphMode::Auto,
        };
        AtomicU8::new(initial as u8)
    })
}

/// Current live display mode.
pub fn mode() -> GlyphMode {
    match mode_atomic().load(Ordering::Relaxed) {
        1 => GlyphMode::ForceKbm,
        2 => GlyphMode::ForceXbox,
        3 => GlyphMode::Off,
        _ => GlyphMode::Auto,
    }
}

/// Set the live display mode (primarily for tests / hotkey cycling).
pub fn set_mode(new_mode: GlyphMode) {
    mode_atomic().store(new_mode as u8, Ordering::Relaxed);
}

/// Called by `victory-hle::modules::gameinput` when keyboard or mouse activity
/// is detected on the host.
pub fn note_kbm_activity() {
    let prev = ACTIVE_DEVICE.swap(ActiveDevice::KeyboardMouse as u8, Ordering::Relaxed);
    if prev != ActiveDevice::KeyboardMouse as u8 {
        tracing::info!(device = "kbm", "active input device switched -> Keyboard/Mouse glyphs");
    }
}

/// Called by `victory-hle::modules::gameinput` when real XInput gamepad input
/// (button press, trigger > 0.12, or stick > 0.22) is detected.
pub fn note_gamepad_activity() {
    let prev = ACTIVE_DEVICE.swap(ActiveDevice::Gamepad as u8, Ordering::Relaxed);
    if prev != ActiveDevice::Gamepad as u8 {
        tracing::info!(device = "gamepad", "active input device switched -> Xbox Gamepad glyphs");
    }
}

/// Returns the most recently active physical input device.
pub fn active_device() -> ActiveDevice {
    if ACTIVE_DEVICE.load(Ordering::Relaxed) == ActiveDevice::Gamepad as u8 {
        ActiveDevice::Gamepad
    } else {
        ActiveDevice::KeyboardMouse
    }
}

/// Whether KBM glyphs should be shown on the current frame.
pub fn show_kbm_glyphs() -> bool {
    match mode() {
        GlyphMode::Auto => active_device() == ActiveDevice::KeyboardMouse,
        GlyphMode::ForceKbm => true,
        GlyphMode::ForceXbox | GlyphMode::Off => false,
    }
}

/// One Xbox-button -> PC-keycap glyph entry matching `gameinput::FALLBACK_BUTTONS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphBinding {
    pub xbox: &'static str,
    pub badge_rgb: [u8; 3],
    pub kbm: String,
}

/// Default Xbox -> KBM glyph layout matching `victory-hle/src/modules/gameinput.rs`
/// (`FALLBACK_BUTTONS`, `WASD` left stick, `IJKL` right stick, `Z`/`C` triggers).
pub fn default_bindings() -> Vec<GlyphBinding> {
    const GREEN: [u8; 3] = [66, 206, 98];
    const RED: [u8; 3] = [232, 74, 74];
    const BLUE: [u8; 3] = [68, 152, 246];
    const AMBER: [u8; 3] = [242, 194, 54];
    const STEEL: [u8; 3] = [148, 160, 182];
    const SLATE: [u8; 3] = [122, 134, 156];

    vec![
        GlyphBinding { xbox: "A", badge_rgb: GREEN, kbm: "ENTER".into() },
        GlyphBinding { xbox: "B", badge_rgb: RED, kbm: "BKSP".into() },
        GlyphBinding { xbox: "X", badge_rgb: BLUE, kbm: "X".into() },
        GlyphBinding { xbox: "Y", badge_rgb: AMBER, kbm: "Y".into() },
        GlyphBinding { xbox: "LB", badge_rgb: STEEL, kbm: "Q".into() },
        GlyphBinding { xbox: "RB", badge_rgb: STEEL, kbm: "E".into() },
        GlyphBinding { xbox: "LT", badge_rgb: STEEL, kbm: "Z".into() },
        GlyphBinding { xbox: "RT", badge_rgb: STEEL, kbm: "C".into() },
        GlyphBinding { xbox: "L", badge_rgb: SLATE, kbm: "WASD".into() },
        GlyphBinding { xbox: "R", badge_rgb: SLATE, kbm: "IJKL".into() },
        GlyphBinding { xbox: "LS", badge_rgb: SLATE, kbm: "F".into() },
        GlyphBinding { xbox: "RS", badge_rgb: SLATE, kbm: "H".into() },
        GlyphBinding { xbox: "MENU", badge_rgb: SLATE, kbm: "ESC".into() },
        GlyphBinding { xbox: "VIEW", badge_rgb: SLATE, kbm: "TAB".into() },
    ]
}

/// Applies `VICTORY_GLYPH_MAP` overrides (e.g. `RT=LMB,LT=RMB,R=MOUSE,A=SPACE`)
/// onto `default_bindings()`.
pub fn parse_bindings(overrides: Option<&str>) -> Vec<GlyphBinding> {
    let mut list = default_bindings();
    let Some(raw) = overrides else {
        return list;
    };
    for pair in raw.split([',', ';']) {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_uppercase();
        let v = v.trim().to_ascii_uppercase();
        if v.is_empty() || v.len() > 8 {
            continue;
        }
        if let Some(entry) = list.iter_mut().find(|e| e.xbox == k) {
            entry.kbm = v;
        }
    }
    list
}

fn bindings() -> &'static [GlyphBinding] {
    static LIST: OnceLock<Vec<GlyphBinding>> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw = std::env::var("VICTORY_GLYPH_MAP").ok();
        parse_bindings(raw.as_deref())
    })
}

#[cfg(windows)]
const UPLOADS: usize = 6;
#[cfg(windows)]
const GLYPH_W: u32 = 5;
#[cfg(windows)]
const GLYPH_H: u32 = 7;
#[cfg(windows)]
const CELL_W: u32 = 6;
#[cfg(windows)]
const MARGIN_X: u32 = 12;
#[cfg(windows)]
const MARGIN_Y: u32 = 12;
#[cfg(windows)]
const STRIP_W_DOTS: u32 = 304;
#[cfg(windows)]
const STRIP_H_DOTS: u32 = 27;

#[cfg(windows)]
fn hotkey_pressed() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain key state query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F6)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut pid = 0u32;
    // SAFETY: plain foreground window query.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut pid) };
    pid == std::process::id()
}

#[cfg(windows)]
#[rustfmt::skip]
const fn font_glyph(c: u8) -> [u8; 7] {
    match c {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        b'3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00010, 0b00100, 0b00100, 0b00100],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'A' => [0b00100, 0b01010, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        b'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01111, 0b10000, 0b10000, 0b10011, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        b'[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        b']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        _ => [0; 7],
    }
}

#[cfg(windows)]
fn strip_scale(back_h: u32) -> u32 {
    if back_h >= 1800 {
        3
    } else if back_h >= 640 {
        2
    } else {
        1
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PixelFmt {
    Rgba8,
    Bgra8,
    Rgb10A2,
    Rgba16F,
}

#[cfg(windows)]
fn pixel_fmt(format: DXGI_FORMAT) -> Option<(PixelFmt, u32)> {
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => Some((PixelFmt::Rgba8, 4)),
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => Some((PixelFmt::Bgra8, 4)),
        DXGI_FORMAT_R10G10B10A2_UNORM => Some((PixelFmt::Rgb10A2, 4)),
        DXGI_FORMAT_R16G16B16A16_FLOAT => Some((PixelFmt::Rgba16F, 8)),
        _ => None,
    }
}

#[cfg(windows)]
fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = bits & 0x007F_FFFF;
    if exp <= 0 {
        sign
    } else if exp >= 31 {
        sign | 0x7C00
    } else {
        sign | ((exp as u16) << 10) | ((mant >> 13) as u16)
    }
}

#[cfg(windows)]
fn write_pixel(dst: &mut [u8], fmt: PixelFmt, [r, g, b]: [u8; 3]) {
    match fmt {
        PixelFmt::Rgba8 => dst[..4].copy_from_slice(&[r, g, b, 255]),
        PixelFmt::Bgra8 => dst[..4].copy_from_slice(&[b, g, r, 255]),
        PixelFmt::Rgb10A2 => {
            let r10 = (u32::from(r) * 1023 + 127) / 255;
            let g10 = (u32::from(g) * 1023 + 127) / 255;
            let b10 = (u32::from(b) * 1023 + 127) / 255;
            let packed = r10 | (g10 << 10) | (b10 << 20) | (3u32 << 30);
            dst[..4].copy_from_slice(&packed.to_le_bytes());
        }
        PixelFmt::Rgba16F => {
            let to_lin = |c: u8| {
                let s = f32::from(c) / 255.0;
                s * s
            };
            let vals = [
                f32_to_f16(to_lin(r)),
                f32_to_f16(to_lin(g)),
                f32_to_f16(to_lin(b)),
                f32_to_f16(1.0),
            ];
            for (i, h) in vals.into_iter().enumerate() {
                dst[i * 2..i * 2 + 2].copy_from_slice(&h.to_le_bytes());
            }
        }
    }
}

#[cfg(windows)]
struct Ring {
    buffers: Vec<(ID3D12Resource, *mut u8)>,
    next: usize,
    scale: u32,
    format: DXGI_FORMAT,
    pitch: u32,
}

#[cfg(windows)]
// SAFETY: upload mappings owned by `InputGlyphsHud` on the presenter thread.
unsafe impl Send for Ring {}

/// Zero-VRAM KBM keycap strip drawn onto the swap-chain back buffer whenever
/// `show_kbm_glyphs()` is true (automatically hides when Gamepad is active).
#[cfg(windows)]
pub(crate) struct InputGlyphsHud {
    ring: Option<Ring>,
    last_shown: bool,
}

#[cfg(windows)]
impl InputGlyphsHud {
    pub(crate) fn new() -> Self {
        Self {
            ring: None,
            last_shown: false,
        }
    }

    pub(crate) fn tick(&mut self) {
        if hotkey_pressed() {
            let next = match mode() {
                GlyphMode::Auto => GlyphMode::ForceKbm,
                GlyphMode::ForceKbm => GlyphMode::ForceXbox,
                GlyphMode::ForceXbox => GlyphMode::Off,
                GlyphMode::Off => GlyphMode::Auto,
            };
            set_mode(next);
            tracing::warn!(mode = ?next, "input glyphs mode cycled (F6)");
        }
    }

    pub(crate) fn active(&self) -> bool {
        show_kbm_glyphs()
    }

    fn ensure_ring(
        &mut self,
        host: &Host,
        scale: u32,
        format: DXGI_FORMAT,
        fmt: PixelFmt,
        bpp: u32,
    ) -> Option<&mut Ring> {
        let needs_new = self
            .ring
            .as_ref()
            .is_none_or(|r| r.scale != scale || r.format != format);
        if needs_new {
            let bw = STRIP_W_DOTS * scale;
            let bh = STRIP_H_DOTS * scale;
            let pitch = (bw * bpp).next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT);
            let total = u64::from(pitch) * u64::from(bh);
            let mut buffers = Vec::with_capacity(UPLOADS);
            for _ in 0..UPLOADS {
                let (buf, ptr) = host.upload_buffer(total)?;
                Self::rasterize(ptr, pitch, scale, fmt, bpp);
                buffers.push((buf, ptr));
            }
            self.ring = Some(Ring {
                buffers,
                next: 0,
                scale,
                format,
                pitch,
            });
        }
        self.ring.as_mut()
    }

    fn rasterize(dst: *mut u8, pitch: u32, scale: u32, fmt: PixelFmt, bpp: u32) {
        let bw = STRIP_W_DOTS * scale;
        let bh = STRIP_H_DOTS * scale;
        let bg = [11u8, 10, 16];
        let border = [42u8, 38, 56];
        let accent = [242u8, 41, 139]; // Anaconda magenta
        let key_bg = [28u8, 25, 38];
        let key_border = [76u8, 69, 98];
        let key_fg = [245u8, 240, 246];
        let badge_fg = [12u8, 10, 16];

        // Fill background + thin frame + left magenta accent bar.
        let mut dot_rgb = vec![bg; (STRIP_W_DOTS * STRIP_H_DOTS) as usize];
        let set_dot = |buf: &mut [[u8; 3]], x: u32, y: u32, rgb: [u8; 3]| {
            if x < STRIP_W_DOTS && y < STRIP_H_DOTS {
                buf[(y * STRIP_W_DOTS + x) as usize] = rgb;
            }
        };
        for x in 0..STRIP_W_DOTS {
            set_dot(&mut dot_rgb, x, 0, border);
            set_dot(&mut dot_rgb, x, STRIP_H_DOTS - 1, border);
        }
        for y in 0..STRIP_H_DOTS {
            set_dot(&mut dot_rgb, STRIP_W_DOTS - 1, y, border);
            for ax in 0..3 {
                set_dot(&mut dot_rgb, ax, y, accent);
            }
        }

        let draw_text = |buf: &mut [[u8; 3]], x0: u32, y0: u32, text: &str, rgb: [u8; 3]| {
            for (ci, b) in text.bytes().enumerate() {
                let g = font_glyph(b.to_ascii_uppercase());
                for gy in 0..GLYPH_H {
                    let row = g[gy as usize];
                    for gx in 0..GLYPH_W {
                        if (row >> (GLYPH_W - 1 - gx)) & 1 != 0 {
                            set_dot(buf, x0 + ci as u32 * CELL_W + gx, y0 + gy, rgb);
                        }
                    }
                }
            }
        };

        let draw_chip = |buf: &mut [[u8; 3]], x0: u32, y0: u32, xbox: &str, badge: [u8; 3], kbm: &str| -> u32 {
            let bw_dots = (xbox.len() as u32) * CELL_W + 3;
            let kw_dots = (kbm.len() as u32) * CELL_W + 5;
            let h_dots = 10u32;
            // Colored Xbox icon pill
            for y in y0..y0 + h_dots {
                for x in x0..x0 + bw_dots {
                    set_dot(buf, x, y, badge);
                }
            }
            draw_text(buf, x0 + 2, y0 + 2, xbox, badge_fg);

            // Keycap box directly beside it
            let kx0 = x0 + bw_dots + 1;
            for y in y0..y0 + h_dots {
                for x in kx0..kx0 + kw_dots {
                    let edge = x == kx0 || x + 1 == kx0 + kw_dots || y == y0 || y + 1 == y0 + h_dots;
                    set_dot(buf, x, y, if edge { key_border } else { key_bg });
                }
            }
            draw_text(buf, kx0 + 3, y0 + 2, kbm, key_fg);
            kx0 + kw_dots + 5
        };

        // Row 1: primary gameplay buttons (A, B, X, Y, LB, RB, LT, RT)
        // Row 2: sticks, clicks & system buttons (L, R, LS, RS, MENU, VIEW)
        let list = bindings();
        let mut x1 = 6u32;
        for b in list.iter().take(8) {
            let need = (b.xbox.len() + b.kbm.len()) as u32 * CELL_W + 10;
            if x1 + need < STRIP_W_DOTS - 2 {
                x1 = draw_chip(&mut dot_rgb, x1, 3, b.xbox, b.badge_rgb, &b.kbm);
            }
        }
        let mut x2 = 6u32;
        for b in list.iter().skip(8) {
            let need = (b.xbox.len() + b.kbm.len()) as u32 * CELL_W + 10;
            if x2 + need < STRIP_W_DOTS - 2 {
                x2 = draw_chip(&mut dot_rgb, x2, 15, b.xbox, b.badge_rgb, &b.kbm);
            }
        }

        // Upscale dots into the mapped upload buffer.
        for py in 0..bh {
            let dy = py / scale;
            // SAFETY: mapped upload buffer of `pitch * bh` bytes.
            let row = unsafe {
                std::slice::from_raw_parts_mut(dst.add((py * pitch) as usize), (bw * bpp) as usize)
            };
            for px in 0..bw {
                let dx = px / scale;
                let rgb = dot_rgb[(dy * STRIP_W_DOTS + dx) as usize];
                let off = (px * bpp) as usize;
                write_pixel(&mut row[off..off + bpp as usize], fmt, rgb);
            }
        }
    }

    /// Copies the KBM keycap glyph strip onto `back` (which must be in
    /// `D3D12_RESOURCE_STATE_COPY_DEST`) when Keyboard & Mouse is active.
    pub(crate) fn draw(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
        keep: impl FnOnce(&ID3D12Resource),
    ) {
        let shown = show_kbm_glyphs();
        self.last_shown = shown;
        if !shown {
            return;
        }
        let Some((fmt, bpp)) = pixel_fmt(format) else {
            return;
        };
        let scale = strip_scale(height);
        let bw = STRIP_W_DOTS * scale;
        let bh = STRIP_H_DOTS * scale;
        if width < bw + MARGIN_X * 2 || height < bh + MARGIN_Y * 2 {
            return;
        }
        let Some(ring) = self.ensure_ring(host, scale, format, fmt, bpp) else {
            return;
        };
        let slot = ring.next % ring.buffers.len();
        ring.next = ring.next.wrapping_add(1);
        let src_buf = ring.buffers[slot].0.clone();
        keep(&src_buf);

        let dst_x = MARGIN_X;
        let dst_y = height - bh - MARGIN_Y;
        // Defensive guard against swapchain dimension/format mismatch.
        // SAFETY: live D3D12 back-buffer resource.
        let bd = unsafe { back.GetDesc() };
        if bd.Format != format
            || u64::from(dst_x + bw) > bd.Width
            || dst_y + bh > bd.Height
        {
            return;
        }
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&src_buf) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format,
                        Width: bw,
                        Height: bh,
                        Depth: 1,
                        RowPitch: ring.pitch,
                    },
                },
            },
        };
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(back) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let part = D3D12_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: bw,
            bottom: bh,
            back: 1,
        };
        // SAFETY: open command list, `back` in COPY_DEST, `src_buf` in GENERIC_READ.
        unsafe {
            list.CopyTextureRegion(&to, dst_x, dst_y, 0, &from, Some(&part));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_device_switches_automatically_and_respects_mode() {
        set_mode(GlyphMode::Auto);
        note_kbm_activity();
        assert_eq!(active_device(), ActiveDevice::KeyboardMouse);
        assert!(show_kbm_glyphs());

        note_gamepad_activity();
        assert_eq!(active_device(), ActiveDevice::Gamepad);
        assert!(!show_kbm_glyphs());

        set_mode(GlyphMode::ForceKbm);
        assert!(show_kbm_glyphs());

        set_mode(GlyphMode::ForceXbox);
        note_kbm_activity();
        assert!(!show_kbm_glyphs());

        set_mode(GlyphMode::Auto);
        assert!(show_kbm_glyphs());
    }

    #[test]
    fn glyph_map_overrides_apply_without_losing_defaults() {
        let map = parse_bindings(Some("RT=LMB, LT=RMB, r=MOUSE, A=SPACE"));
        let find = |xbox: &str| map.iter().find(|b| b.xbox == xbox).unwrap().kbm.as_str();
        assert_eq!(find("RT"), "LMB");
        assert_eq!(find("LT"), "RMB");
        assert_eq!(find("R"), "MOUSE");
        assert_eq!(find("A"), "SPACE");
        assert_eq!(find("LB"), "Q");
        assert_eq!(find("L"), "WASD");
    }

    #[cfg(windows)]
    #[test]
    fn input_glyphs_strip_copies_cleanly_onto_gpu_backbuffer() {
        let Some(host) = crate::host::host() else {
            return;
        };
        set_mode(GlyphMode::Auto);
        note_kbm_activity();
        let (w, h) = (1280u32, 720u32);
        let fmt = DXGI_FORMAT_R8G8B8A8_UNORM;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(w),
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let mut tex: Option<ID3D12Resource> = None;
        unsafe {
            host.device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut tex,
                )
                .unwrap();
        }
        let tex = tex.unwrap();
        let mut glyphs = InputGlyphsHud::new();
        let list = host.new_list().unwrap();
        glyphs.draw(host, &list.list, &tex, (w, h), fmt, |_| {});
        list.close();
        host.execute(&[&list]);

        // Verify magenta accent pixel at bottom-left of the KBM glyph strip.
        let scale = strip_scale(h);
        let bh = STRIP_H_DOTS * scale;
        let dst_x = MARGIN_X;
        let dst_y = h - bh - MARGIN_Y;
        let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        unsafe {
            host.device
                .GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut fp), None, None, Some(&mut total));
        }
        let rb = host.read_back_buffer(total).unwrap();
        let l2 = host.new_list().unwrap();
        let src_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&tex) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let dst_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&rb) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
        };
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: unsafe { std::mem::transmute_copy(&tex) },
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: D3D12_RESOURCE_STATE_COPY_DEST,
                    StateAfter: D3D12_RESOURCE_STATE_COPY_SOURCE,
                }),
            },
        };
        unsafe {
            l2.list.ResourceBarrier(&[barrier]);
            l2.list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None);
        }
        l2.close();
        host.execute(&[&l2]);

        let mut ptr = std::ptr::null_mut();
        unsafe { rb.Map(0, None, Some(&mut ptr)).unwrap() };
        let offset = fp.Offset as usize + (dst_y as usize) * (fp.Footprint.RowPitch as usize) + (dst_x as usize) * 4;
        let px = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>().add(offset), 4) };
        assert_eq!(px, &[242, 41, 139, 255], "expected magenta glyph-strip accent at bottom-left");
        unsafe { rb.Unmap(0, None) };
    }

    #[cfg(windows)]
    #[test]
    fn input_glyphs_all_swap_formats_and_resolutions_close_cleanly() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let _ = host.log_messages();
        set_mode(GlyphMode::ForceKbm);
        let mut glyphs = InputGlyphsHud::new();
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        for fmt in [
            DXGI_FORMAT_R8G8B8A8_UNORM,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_FORMAT_R10G10B10A2_UNORM,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
        ] {
            for (w, h) in [(640u32, 360u32), (1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: u64::from(w),
                    Height: h,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: fmt,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                let mut tex: Option<ID3D12Resource> = None;
                unsafe {
                    host.device
                        .CreateCommittedResource(
                            &heap,
                            D3D12_HEAP_FLAG_NONE,
                            &desc,
                            D3D12_RESOURCE_STATE_COPY_DEST,
                            None,
                            &mut tex,
                        )
                        .unwrap();
                }
                let tex = tex.unwrap();
                let list = host.new_list().unwrap();
                glyphs.draw(host, &list.list, &tex, (w, h), fmt, |r| list.keep(r));
                let closed = unsafe { list.list.Close() };
                let msg = host.log_messages();
                assert!(closed.is_ok(), "{w}x{h} fmt={} Close failed: {closed:?} msg={msg:?}", fmt.0);
                assert!(msg.is_none(), "{w}x{h} fmt={} debug error: {msg:?}", fmt.0);
            }
        }
        set_mode(GlyphMode::Auto);
    }
}
