//! Lightweight release-ready on-screen FPS & PC hardware specs HUD overlay
//! drawn directly onto the swap-chain back buffer (no window title bar
//! required; works in fullscreen, borderless, and windowed modes, on both real
//! and AMD FrameGen frames).
//!
//! Uses a tiny upload-buffer ring + single `CopyTextureRegion` (< 0.01 ms GPU,
//! 0 bytes dedicated VRAM, works on all swap-chain formats).
//!
//! Controlled by:
//! - `VICTORY_FPS_HUD=side` (or `mid-right`): right side below the game's top-right cash/wanted HUD.
//! - `VICTORY_FPS_HUD=right` (or `1`, default when unset): top-right corner.
//! - `VICTORY_FPS_HUD=left` / `side-left`: top-left / mid-left edge.
//! - `VICTORY_FPS_HUD=bottom-right` / `bottom-left`: bottom corners.
//! - `VICTORY_FPS_HUD=0` / `off`: disabled at startup.
//! - Hotkey **`F4`** toggles visibility live while the game window has focus.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F4};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::host::Host;

const REFRESH: Duration = Duration::from_millis(250);
const UPLOADS: usize = 6;

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
const CELL_W: u32 = 6;
const CELL_H: u32 = 9;
const COLS: u32 = 20;
const LINES: usize = 5;
const PAD_X: u32 = 6;
const PAD_Y: u32 = 4;
const MARGIN: u32 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Corner {
    Off,
    TopRight,
    MidRight,
    TopLeft,
    MidLeft,
    BottomRight,
    BottomLeft,
}

fn parse_corner(raw: Option<&str>) -> Corner {
    match raw.map(str::trim) {
        Some("0") | Some("off") | Some("false") => Corner::Off,
        Some("side") | Some("mid-right") | Some("side-right") | Some("mr") => Corner::MidRight,
        Some("side-left") | Some("mid-left") | Some("ml") => Corner::MidLeft,
        Some("left") | Some("top-left") | Some("tl") => Corner::TopLeft,
        Some("bottom-right") | Some("br") => Corner::BottomRight,
        Some("bottom-left") | Some("bl") => Corner::BottomLeft,
        // Default ON in top-right (preserves existing default layout).
        _ => Corner::TopRight,
    }
}

fn config_corner() -> Corner {
    static CORNER: OnceLock<Corner> = OnceLock::new();
    *CORNER.get_or_init(|| parse_corner(std::env::var("VICTORY_FPS_HUD").ok().as_deref()))
}

fn shown_flag() -> &'static AtomicBool {
    static SHOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHOWN.get_or_init(|| AtomicBool::new(config_corner() != Corner::Off))
}

fn hotkey_pressed() -> bool {
    static DOWN: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain key state query.
    let down = unsafe { GetAsyncKeyState(i32::from(VK_F4)) } < 0;
    if DOWN.swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut pid = 0u32;
    // SAFETY: plain foreground window query.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut pid) };
    pid == std::process::id()
}

/// Reads the CPU brand string via x86_64 CPUID extended leaves (`0x80000002..=4`).
fn cpu_brand_raw() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        let max_ext = std::arch::x86_64::__cpuid(0x8000_0000).eax;
        if max_ext >= 0x8000_0004 {
            let mut bytes = [0u8; 48];
            for (i, leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004].into_iter().enumerate() {
                let r = std::arch::x86_64::__cpuid(leaf);
                let base = i * 16;
                bytes[base..base + 4].copy_from_slice(&r.eax.to_le_bytes());
                bytes[base + 4..base + 8].copy_from_slice(&r.ebx.to_le_bytes());
                bytes[base + 8..base + 12].copy_from_slice(&r.ecx.to_le_bytes());
                bytes[base + 12..base + 16].copy_from_slice(&r.edx.to_le_bytes());
            }
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            return String::from_utf8_lossy(&bytes[..end]).into_owned();
        }
    }
    String::new()
}

/// Formats a raw CPU brand string + logical thread count into a compact <= 20-char HUD line.
fn format_cpu_label(raw: &str, threads: usize) -> String {
    let mut s = raw
        .replace("(R)", " ")
        .replace("(TM)", " ")
        .replace("(tm)", " ");
    if let Some(at) = s.find('@') {
        s.truncate(at);
    }
    let mut tokens: Vec<String> = Vec::new();
    for tok in s.split_whitespace() {
        let up = tok.to_ascii_uppercase();
        if matches!(
            up.as_str(),
            "AMD"
                | "INTEL"
                | "CORE"
                | "CPU"
                | "PROCESSOR"
                | "WITH"
                | "RADEON"
                | "GRAPHICS"
                | "VEGA"
                | "APU"
                | "MOBILE"
                | "GEN"
        ) || up.ends_with("-CORE")
        {
            continue;
        }
        tokens.push(up);
    }
    let body = if tokens.is_empty() {
        "UNKNOWN".to_owned()
    } else {
        tokens.join(" ")
    };
    let max_cols = COLS as usize;
    let base = format!("CPU {body}");
    if threads > 0 {
        let with_threads = format!("{base} {threads}T");
        if with_threads.len() <= max_cols {
            return with_threads;
        }
    }
    base.chars().take(max_cols).collect()
}

/// Formats a DXGI adapter name + dedicated VRAM bytes into a compact <= 20-char HUD line.
fn format_gpu_label(raw: &str, dedicated_bytes: u64) -> String {
    let s = raw
        .replace("(R)", " ")
        .replace("(TM)", " ")
        .replace("(tm)", " ");
    let mut tokens: Vec<String> = Vec::new();
    for tok in s.split_whitespace() {
        let up = tok.to_ascii_uppercase();
        if matches!(
            up.as_str(),
            "NVIDIA" | "GEFORCE" | "AMD" | "RADEON" | "INTEL" | "GRAPHICS" | "SERIES" | "LAPTOP"
        ) {
            continue;
        }
        tokens.push(up);
    }
    let body = if tokens.is_empty() {
        "GPU".to_owned()
    } else {
        tokens.join(" ")
    };
    let max_cols = COLS as usize;
    let vram_gb = ((dedicated_bytes as f64) / (1024.0 * 1024.0 * 1024.0)).round() as u64;
    let base = format!("GPU {body}");
    if vram_gb > 0 {
        let with_vram = format!("{base} {vram_gb}G");
        if with_vram.len() <= max_cols {
            return with_vram;
        }
    }
    base.chars().take(max_cols).collect()
}

fn cached_cpu_label() -> &'static str {
    static LABEL: OnceLock<String> = OnceLock::new();
    LABEL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(0, usize::from);
        format_cpu_label(&cpu_brand_raw(), threads)
    })
}

/// System physical RAM (used_gb, total_gb) via `GlobalMemoryStatusEx`.
fn system_ram_gb() -> Option<(f64, f64)> {
    let mut m = MEMORYSTATUSEX::default();
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized Win32 structure.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
    if !ok || m.ullTotalPhys == 0 {
        return None;
    }
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let total = m.ullTotalPhys as f64 / GIB;
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys) as f64 / GIB;
    Some((used, total))
}

#[rustfmt::skip]
const fn glyph(c: u8) -> [u8; 7] {
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
        b'.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        b',' => [0, 0, 0, 0, 0b00100, 0b00100, 0b01000],
        b':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b'-' => [0, 0, 0, 0b11111, 0, 0, 0],
        b'+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        b'%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        b'(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        b')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        b'_' => [0, 0, 0, 0, 0, 0, 0b11111],
        b' ' => [0; 7],
        _ => [0; 7],
    }
}

const fn box_dots() -> (u32, u32) {
    (COLS * CELL_W + 2 * PAD_X, (LINES as u32) * CELL_H + 2 * PAD_Y - 2)
}

const fn box_pixels(scale: u32) -> (u32, u32) {
    let (dw, dh) = box_dots();
    (dw * scale, dh * scale)
}

fn hud_scale(height: u32) -> u32 {
    if height >= 1080 {
        3
    } else if height >= 540 {
        2
    } else {
        1
    }
}

fn bpp(format: DXGI_FORMAT) -> Option<usize> {
    Some(match format {
        DXGI_FORMAT_R8G8B8A8_UNORM
        | DXGI_FORMAT_B8G8R8A8_UNORM
        | DXGI_FORMAT_R10G10B10A2_UNORM => 4,
        DXGI_FORMAT_R16G16B16A16_FLOAT => 8,
        _ => return None,
    })
}

fn encode_f16(v: f32) -> [u8; 2] {
    // Simple exact IEEE 754 float32 -> float16 for normalized [0..1] UI colors.
    let bits = v.clamp(0.0, 1.0).to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let frac = bits & 0x7F_FFFF;
    let h: u16 = if exp < -14 {
        0
    } else {
        let he = (exp + 15) as u16;
        let hf = (frac >> 13) as u16;
        (he << 10) | hf
    };
    h.to_le_bytes()
}

fn encode_pixel(format: DXGI_FORMAT, [r, g, b]: [u8; 3], out: &mut [u8]) {
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM => out[..4].copy_from_slice(&[r, g, b, 255]),
        DXGI_FORMAT_B8G8R8A8_UNORM => out[..4].copy_from_slice(&[b, g, r, 255]),
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let r10 = (u32::from(r) * 1023 + 127) / 255;
            let g10 = (u32::from(g) * 1023 + 127) / 255;
            let b10 = (u32::from(b) * 1023 + 127) / 255;
            let packed = r10 | (g10 << 10) | (b10 << 20) | (3u32 << 30);
            out[..4].copy_from_slice(&packed.to_le_bytes());
        }
        DXGI_FORMAT_R16G16B16A16_FLOAT => {
            out[0..2].copy_from_slice(&encode_f16(f32::from(r) / 255.0));
            out[2..4].copy_from_slice(&encode_f16(f32::from(g) / 255.0));
            out[4..6].copy_from_slice(&encode_f16(f32::from(b) / 255.0));
            out[6..8].copy_from_slice(&encode_f16(1.0));
        }
        _ => {}
    }
}

fn paint_hud(
    lines: &[String; LINES],
    accent: [u8; 3],
    vram_hot: bool,
    scale: u32,
    format: DXGI_FORMAT,
    pitch: usize,
) -> Vec<u8> {
    let bytes = bpp(format).unwrap_or(4);
    let (dw, dh) = box_dots();
    let (_, ph) = box_pixels(scale);
    let mut bg = [0u8; 8];
    let mut border = [0u8; 8];
    let mut bar = [0u8; 8];
    let mut line0_col = [0u8; 8];
    let mut vram_col = [0u8; 8];
    let mut ram_col = [0u8; 8];
    let mut spec_col = [0u8; 8];
    encode_pixel(format, [14, 17, 23], &mut bg);
    encode_pixel(format, [48, 56, 70], &mut border);
    encode_pixel(format, accent, &mut bar);
    encode_pixel(format, accent, &mut line0_col);
    encode_pixel(
        format,
        if vram_hot { [255, 198, 86] } else { [230, 236, 244] },
        &mut vram_col,
    );
    encode_pixel(format, [202, 212, 224], &mut ram_col);
    encode_pixel(format, [138, 206, 244], &mut spec_col);

    let mut out = vec![0u8; pitch * ph as usize];
    for dy in 0..dh {
        let top = pitch * (dy * scale) as usize;
        for dx in 0..dw {
            let color: &[u8] = if dx < 2 {
                &bar[..bytes]
            } else if dy == 0 || dy + 1 == dh || dx + 1 == dw {
                &border[..bytes]
            } else {
                let mut lit = None;
                if let (Some(tx), Some(ty)) = (dx.checked_sub(PAD_X), dy.checked_sub(PAD_Y)) {
                    let col = tx / CELL_W;
                    let row = ty / CELL_H;
                    let gx = tx % CELL_W;
                    let gy = ty % CELL_H;
                    if col < COLS && (row as usize) < LINES && gx < GLYPH_W && gy < GLYPH_H {
                        if let Some(&ch) = lines[row as usize].as_bytes().get(col as usize) {
                            let bits = glyph(ch.to_ascii_uppercase())[gy as usize];
                            if ((bits >> (GLYPH_W - 1 - gx)) & 1) != 0 {
                                lit = Some(row);
                            }
                        }
                    }
                }
                match lit {
                    Some(0) => &line0_col[..bytes],
                    Some(1) => &vram_col[..bytes],
                    Some(2) => &ram_col[..bytes],
                    Some(_) => &spec_col[..bytes],
                    None => &bg[..bytes],
                }
            };
            for sx in 0..scale {
                let at = top + ((dx * scale + sx) as usize) * bytes;
                out[at..at + bytes].copy_from_slice(color);
            }
        }
        for sy in 1..scale {
            let dst = top + pitch * sy as usize;
            out.copy_within(top..top + pitch, dst);
        }
    }
    out
}

struct Ring {
    format: DXGI_FORMAT,
    scale: u32,
    pitch: u32,
    uploads: Vec<(ID3D12Resource, *mut u8)>,
    turn: usize,
    ready: bool,
}

// SAFETY: upload buffers and mapped pointers are only accessed under Presenter's Mutex.
unsafe impl Send for Ring {}

impl Ring {
    fn new(host: &Host, format: DXGI_FORMAT, scale: u32) -> Option<Self> {
        let bytes_per_px = bpp(format)? as u32;
        let (w, h) = box_pixels(scale);
        let pitch = (w * bytes_per_px).next_multiple_of(256);
        let total = u64::from(pitch) * u64::from(h);
        let uploads = (0..UPLOADS)
            .map(|_| host.upload_buffer(total))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            format,
            scale,
            pitch,
            uploads,
            turn: 0,
            ready: false,
        })
    }

    fn write(&mut self, lines: &[String; LINES], accent: [u8; 3], vram_hot: bool) {
        let img = paint_hud(lines, accent, vram_hot, self.scale, self.format, self.pitch as usize);
        self.turn = (self.turn + 1) % UPLOADS;
        // SAFETY: mapped upload buffer of exact size `img.len()`.
        unsafe {
            std::ptr::copy_nonoverlapping(img.as_ptr(), self.uploads[self.turn].1, img.len());
        }
        self.ready = true;
    }
}

pub(crate) struct FpsHud {
    since: Instant,
    real_frames: u32,
    gen_frames: u32,
    ema_real_fps: f64,
    ema_shown_fps: f64,
    ema_ms: f64,
    lines: [String; LINES],
    accent: [u8; 3],
    vram_hot: bool,
    gpu_label: Option<String>,
    gpu_total_gb: f64,
    dirty: bool,
    ring: Option<Ring>,
    failed: bool,
}

impl FpsHud {
    pub(crate) fn new() -> Self {
        let fg = crate::framegen::enabled();
        let line0 = if fg {
            "FPS -- FG --  --MS".to_owned()
        } else {
            "FPS --      --.-MS".to_owned()
        };
        let cpu_line = cached_cpu_label().to_owned();
        Self {
            since: Instant::now(),
            real_frames: 0,
            gen_frames: 0,
            ema_real_fps: 0.0,
            ema_shown_fps: 0.0,
            ema_ms: 0.0,
            lines: [
                line0,
                "VRAM -.--/-.-GB".to_owned(),
                "RAM  --.-/--.-GB".to_owned(),
                "GPU DETECTING...".to_owned(),
                cpu_line,
            ],
            accent: [92, 255, 122],
            vram_hot: false,
            gpu_label: None,
            gpu_total_gb: 0.0,
            dirty: true,
            ring: None,
            failed: false,
        }
    }

    pub(crate) fn active(&self) -> bool {
        shown_flag().load(Ordering::Relaxed) && !self.failed
    }

    /// Updates FPS and hardware telemetry once per real game frame (`gen` =
    /// generated frames presented alongside this real frame).
    pub(crate) fn tick(&mut self, host: &Host, generated: u32) {
        if hotkey_pressed() {
            shown_flag().fetch_xor(true, Ordering::Relaxed);
            self.dirty = true;
        }
        if !shown_flag().load(Ordering::Relaxed) {
            self.real_frames = 0;
            self.gen_frames = 0;
            self.since = Instant::now();
            return;
        }
        if self.gpu_label.is_none() {
            if let Some((raw, vram_bytes)) = host.adapter_info() {
                self.gpu_label = Some(format_gpu_label(&raw, vram_bytes));
                self.gpu_total_gb = vram_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                self.lines[3] = self.gpu_label.clone().unwrap_or_default();
                self.dirty = true;
            }
        }
        self.real_frames += 1;
        self.gen_frames += generated;
        let elapsed = self.since.elapsed();
        if elapsed < REFRESH {
            return;
        }
        let dt = elapsed.as_secs_f64();
        let inst_real = f64::from(self.real_frames) / dt;
        let inst_shown = f64::from(self.real_frames + self.gen_frames) / dt;
        let inst_ms = dt * 1000.0 / f64::from(self.real_frames).max(1.0);
        if self.ema_real_fps <= 0.0 {
            self.ema_real_fps = inst_real;
            self.ema_shown_fps = inst_shown;
            self.ema_ms = inst_ms;
        } else {
            self.ema_real_fps = self.ema_real_fps * 0.55 + inst_real * 0.45;
            self.ema_shown_fps = self.ema_shown_fps * 0.55 + inst_shown * 0.45;
            self.ema_ms = self.ema_ms * 0.55 + inst_ms * 0.45;
        }
        self.real_frames = 0;
        self.gen_frames = 0;
        self.since = Instant::now();

        let (vram_gb, vram_hot) = match host.video_memory() {
            Some([budget, used, ..]) => {
                let gb = used as f64 / (1024.0 * 1024.0 * 1024.0);
                let hot = budget > 0 && u128::from(used) * 100 >= u128::from(budget) * 95;
                (gb, hot)
            }
            None => (0.0, false),
        };
        self.vram_hot = vram_hot;
        let display_fps = if crate::framegen::enabled() {
            self.ema_shown_fps
        } else {
            self.ema_real_fps
        };
        self.accent = if display_fps >= 30.0 {
            [92, 255, 122] // bright green
        } else if display_fps >= 15.0 {
            [255, 210, 76] // warm amber
        } else {
            [255, 107, 74] // vivid coral-orange
        };
        if crate::framegen::enabled() {
            self.lines[0] = format!(
                "FPS {:<2.0} FG {:<2.0} {:>4.0}MS",
                self.ema_real_fps.round(),
                self.ema_shown_fps.round(),
                self.ema_ms.min(9999.0)
            );
        } else {
            self.lines[0] = format!(
                "FPS {:<3.0}   {:>6.1}MS",
                self.ema_real_fps.round(),
                self.ema_ms.min(9999.9)
            );
        }
        self.lines[1] = if self.gpu_total_gb >= 0.5 {
            format!("VRAM {:.2}/{:.1}GB", vram_gb, self.gpu_total_gb)
        } else {
            format!("VRAM {:.2}GB", vram_gb)
        };
        self.lines[2] = match system_ram_gb() {
            Some((used, total)) => format!("RAM  {:.1}/{:.0}GB", used, total.round()),
            None => "RAM  --/--GB".to_owned(),
        };
        if let Some(gpu) = &self.gpu_label {
            self.lines[3] = gpu.clone();
        }
        self.lines[4] = cached_cpu_label().to_owned();
        self.dirty = true;
    }

    /// Copies the compact FPS & specs badge directly onto `back` (must be in
    /// `D3D12_RESOURCE_STATE_COPY_DEST`), calling `keep` with the upload buffer
    /// so it stays alive until GPU execution finishes.
    pub(crate) fn draw(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
        keep: impl FnOnce(&ID3D12Resource),
    ) {
        if !self.active() || width < 64 || height < 64 {
            return;
        }
        let scale = hud_scale(height);
        if self.ring.as_ref().is_none_or(|r| (r.format, r.scale) != (format, scale)) {
            self.ring = Ring::new(host, format, scale);
            self.failed = self.ring.is_none();
            self.dirty = true;
        }
        let Some(ring) = self.ring.as_mut() else {
            return;
        };
        if self.dirty || !ring.ready {
            ring.write(&self.lines, self.accent, self.vram_hot);
            self.dirty = false;
        }
        let (bw, bh) = box_pixels(scale);
        let cw = bw.min(width);
        let ch = bh.min(height);
        let corner = match config_corner() {
            Corner::Off => Corner::TopRight,
            c => c,
        };
        let dst_x = match corner {
            Corner::TopRight | Corner::MidRight | Corner::BottomRight => width.saturating_sub(cw + MARGIN),
            Corner::TopLeft | Corner::MidLeft | Corner::BottomLeft | Corner::Off => MARGIN.min(width.saturating_sub(cw)),
        };
        let dst_y = match corner {
            Corner::BottomRight | Corner::BottomLeft => height.saturating_sub(ch + MARGIN),
            Corner::MidRight | Corner::MidLeft => (height / 5).min(height.saturating_sub(ch + MARGIN)),
            Corner::TopRight | Corner::TopLeft | Corner::Off => MARGIN.min(height.saturating_sub(ch)),
        };

        let src_buf = &ring.uploads[ring.turn].0;
        keep(src_buf);
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(src_buf) },
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
            right: cw,
            bottom: ch,
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
    fn hardware_labels_format_cleanly_within_hud_width() {
        assert_eq!(
            format_gpu_label("NVIDIA GeForce GTX 1650", 4u64 << 30),
            "GPU GTX 1650 4G"
        );
        assert_eq!(
            format_gpu_label("AMD Radeon RX 7900 XTX", 24u64 << 30),
            "GPU RX 7900 XTX 24G"
        );
        assert_eq!(
            format_cpu_label("AMD Ryzen 5 5500                               ", 12),
            "CPU RYZEN 5 5500 12T"
        );
        assert_eq!(
            format_cpu_label("AMD Ryzen 5 5500 6-Core Processor", 12),
            "CPU RYZEN 5 5500 12T"
        );
        assert_eq!(
            format_cpu_label("13th Gen Intel(R) Core(TM) i7-13700K @ 3.40GHz", 24),
            "CPU 13TH I7-13700K"
        );
        for line in [
            format_gpu_label("NVIDIA GeForce RTX 4090 Laptop GPU", 16u64 << 30),
            format_cpu_label("AMD Ryzen 9 7950X3D 16-Core Processor", 32),
        ] {
            assert!(line.len() <= COLS as usize, "exceeded {COLS} cols: {line:?}");
        }
        assert_eq!(parse_corner(Some("side")), Corner::MidRight);
        assert_eq!(parse_corner(Some("right")), Corner::TopRight);
        assert_eq!(parse_corner(Some("0")), Corner::Off);
    }

    #[test]
    fn full_alphanumeric_font_has_visible_glyphs() {
        for ch in (b'0'..=b'9').chain(b'A'..=b'Z') {
            assert!(
                glyph(ch).iter().any(|&row| row != 0),
                "missing 5x7 bitmap glyph for '{}'",
                ch as char
            );
        }
    }

    #[test]
    fn fps_hud_copies_onto_backbuffer_cleanly_on_gpu() {
        let Some(host) = crate::host::host() else {
            eprintln!("skipping FPS HUD GPU test: no D3D12 device");
            return;
        };
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
        let mut hud = FpsHud::new();
        hud.lines = [
            "FPS 30     33.3MS".to_owned(),
            "VRAM 2.74/4.0GB".to_owned(),
            "RAM  9.8/16GB".to_owned(),
            "GPU GTX 1650 4G".to_owned(),
            "CPU RYZEN 5 5500 12T".to_owned(),
        ];
        hud.accent = [92, 255, 122];
        hud.dirty = true;

        let list = host.new_list().unwrap();
        hud.draw(host, &list.list, &tex, (w, h), fmt, |_| {});
        list.close();
        host.execute(&[&list]);

        // Read back the accent bar pixel to prove the HUD rendered cleanly.
        let (bw, bh) = box_pixels(hud_scale(h));
        let dst_x = w - bw - MARGIN;
        let dst_y = match config_corner() {
            Corner::MidRight => (h / 5).min(h.saturating_sub(bh + MARGIN)),
            _ => MARGIN,
        };
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
        assert_eq!(px, &[92, 255, 122, 255], "expected green HUD accent bar at ({dst_x}, {dst_y})");
        unsafe { rb.Unmap(0, None) };
    }
}
