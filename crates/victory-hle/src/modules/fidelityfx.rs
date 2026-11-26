//! `amd_fidelityfx_loader_xbox.dll`: AMD FidelityFX's API for Series S mode.
//!
//! Serves either:
//! - an external PC build (`amd_fidelityfx_upscaler_dx12.dll` in `VICTORY_FIDELITYFX_DIR`), OR
//! - a built-in zero-dependency D3D12 compute bilinear upscaler (active automatically
//!   in Series S mode when `VICTORY_FIDELITYFX_DIR` is not set, or when
//!   `VICTORY_FIDELITYFX_BUILTIN=1`).
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CStr, c_char, c_void};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use victory_gpu::object::Object;

use crate::{HleModule, hle_export};

const DLL: &str = "amd_fidelityfx_upscaler_dx12.dll";

// Return codes.
const OK: u32 = 0;
const UNKNOWN_TYPE: u32 = 2;
const NO_PROVIDER: u32 = 4;
const PARAMETER: u32 = 6;

// Description types.
const GET_VERSIONS: u64 = 4;
const GET_PROVIDER_VERSION: u64 = 6;
const CREATE_UPSCALE: u64 = 0x1_0000;
const DISPATCH_UPSCALE: u64 = 0x1_0001;
const QUERY_UPSCALE_RATIO: u64 = 0x1_0002;
const QUERY_RENDER_RESOLUTION: u64 = 0x1_0003;
const QUERY_JITTER_PHASE_COUNT: u64 = 0x1_0004;
const QUERY_JITTER_OFFSET: u64 = 0x1_0005;
const QUERY_GPU_MEMORY_USAGE: u64 = 0x1_0008;
const MEMORY_USAGE_WITH_DEVICE: u64 = 0x1_0009;
const CONSOLE_BACKEND: u64 = 0x100_0002;
const HOST_BACKEND: u64 = 2;

const BUILTIN_PROVIDER_ID: u64 = 0x0001_0000;
const BUILTIN_PROVIDER_NAME: &CStr = c"VIctory Built-in Upscaler";
const BUILTIN_CTX: usize = 0x4658_0001;

#[repr(C)]
#[derive(Clone, Copy)]
struct Header {
    kind: u64,
    next: *mut Header,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CreateUpscale {
    header: Header,
    flags: u32,
    max_render: [u32; 2],
    max_output: [u32; 2],
    message: usize,
}

#[repr(C)]
struct Backend {
    header: Header,
    device: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: [u32; 8],
    state: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DispatchUpscale {
    header: Header,
    list: *mut c_void,
    resources: [Resource; 7],
    jitter: [f32; 2],
    motion_scale: [f32; 2],
    render: [u32; 2],
    output: [u32; 2],
    sharpen: u32,
    sharpness: f32,
    frame_ms: f32,
    pre_exposure: f32,
    reset: u32,
    near: f32,
    far: f32,
    fov: f32,
    meters: f32,
    flags: u32,
}

const _: () = assert!(size_of::<CreateUpscale>() == 0x30);
const _: () = assert!(size_of::<Resource>() == 0x30);
const _: () = assert!(size_of::<DispatchUpscale>() == 0x1b0);

#[repr(C)]
struct GetVersions {
    header: Header,
    create_kind: u64,
    device: *mut c_void,
    count: *mut u64,
    ids: *mut u64,
    names: *mut *const c_char,
}

#[repr(C)]
struct ProviderVersion {
    header: Header,
    id: u64,
    name: *const c_char,
}

#[repr(C)]
struct UpscaleRatio {
    header: Header,
    quality_mode: u32,
    ratio: *mut f32,
}

#[repr(C)]
struct RenderResolution {
    header: Header,
    display_width: u32,
    display_height: u32,
    quality_mode: u32,
    render_width: *mut u32,
    render_height: *mut u32,
}

#[repr(C)]
struct PhaseCount {
    header: Header,
    render_width: u32,
    output_width: u32,
    count: *mut i32,
}

#[repr(C)]
struct JitterOffset {
    header: Header,
    index: i32,
    phases: i32,
    x: *mut f32,
    y: *mut f32,
}

#[repr(C)]
struct MemoryUsage {
    header: Header,
    usage: *mut [u64; 2],
}

#[repr(C)]
struct MemoryUsageWithDevice {
    header: Header,
    device: *mut c_void,
    max_render: [u32; 2],
    max_output: [u32; 2],
    usage: *mut [u64; 2],
}

type Create = unsafe extern "C" fn(*mut *mut c_void, *mut Header, *const c_void) -> u32;
type Destroy = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> u32;
type Call = unsafe extern "C" fn(*mut *mut c_void, *mut Header) -> u32;

struct ExternalHost {
    create: Create,
    destroy: Destroy,
    configure: Call,
    query: Call,
    dispatch: Call,
    device: *mut c_void,
}

enum Mode {
    External(ExternalHost),
    Builtin,
}

// SAFETY: D3D12 devices and loaded DLL code pointers are thread-safe.
unsafe impl Send for Mode {}
unsafe impl Sync for Mode {}

static MODE: OnceLock<Mode> = OnceLock::new();

macro_rules! once {
    ($level:ident, $($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::$level!($($arg)*));
    }};
}

/// Initializes FidelityFX serving before the import registry is first built.
/// Uses `VICTORY_FIDELITYFX_DIR` if it offers a host PC upscaler DLL; else
/// enables the built-in GPU compute upscaler automatically on Series S (or when
/// `VICTORY_FIDELITYFX_BUILTIN=1`).
pub fn init() {
    let dir = std::env::var_os("VICTORY_FIDELITYFX_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = dir.as_deref() {
        match load_external(dir) {
            Ok((host, versions)) => {
                tracing::warn!(folder = %dir.display(), versions, "FidelityFX: host PC upscaler serves the title");
                let _ = MODE.set(Mode::External(host));
                return;
            }
            Err(why) => {
                tracing::warn!(folder = %dir.display(), "FidelityFX: {why}; falling back");
            }
        }
    }
    let builtin_env = std::env::var("VICTORY_FIDELITYFX_BUILTIN").ok();
    let want_builtin = match builtin_env.as_deref().map(str::trim) {
        Some("0") => false,
        Some("1") => true,
        _ => victory_gpu::console::current() == victory_gpu::console::Console::SeriesS,
    };
    if want_builtin {
        tracing::warn!("FidelityFX: built-in GPU compute upscaler active for amd_fidelityfx_loader_xbox.dll");
        let _ = MODE.set(Mode::Builtin);
    }
}

pub(crate) fn serving() -> bool {
    MODE.get().is_some()
}

fn load_external(dir: &Path) -> Result<(ExternalHost, String), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    };
    let path = std::path::absolute(dir.join(DLL)).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {DLL} there"));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let search = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    // SAFETY: NUL-terminated path to the user's FidelityFX DLL.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), null_mut(), search) };
    if module.is_null() {
        return Err(format!("{DLL} did not load ({})", std::io::Error::last_os_error()));
    }
    let find = |name: &CStr| {
        // SAFETY: loaded module and NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }
            .map(|f| f as *const c_void)
            .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))
    };
    let device = victory_gpu::external::host_device_raw().ok_or("there is no host GPU")?;
    // SAFETY: FidelityFX API exports.
    let host = unsafe {
        ExternalHost {
            create: std::mem::transmute::<*const c_void, Create>(find(c"ffxCreateContext")?),
            destroy: std::mem::transmute::<*const c_void, Destroy>(find(c"ffxDestroyContext")?),
            configure: std::mem::transmute::<*const c_void, Call>(find(c"ffxConfigure")?),
            query: std::mem::transmute::<*const c_void, Call>(find(c"ffxQuery")?),
            dispatch: std::mem::transmute::<*const c_void, Call>(find(c"ffxDispatch")?),
            device,
        }
    };
    let mut count = 0u64;
    let mut query = GetVersions {
        header: Header { kind: GET_VERSIONS, next: null_mut() },
        create_kind: CREATE_UPSCALE,
        device: host.device,
        count: &mut count,
        ids: null_mut(),
        names: null_mut(),
    };
    // SAFETY: version query against host device.
    let code = unsafe { (host.query)(null_mut(), &mut query.header) };
    if code != OK || count == 0 {
        return Err(format!("it offers no upscaler for the host GPU (code {code})"));
    }
    let mut names = vec![null::<c_char>(); count as usize];
    query.names = names.as_mut_ptr();
    // SAFETY: room for `count` names.
    unsafe { (host.query)(null_mut(), &mut query.header) };
    let versions = names
        .iter()
        .filter(|n| !n.is_null())
        // SAFETY: NUL-terminated names owned by DLL.
        .map(|&n| unsafe { CStr::from_ptr(n) }.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ");
    Ok((host, versions))
}

fn link(upscale: &mut CreateUpscale, backend: &mut Backend) {
    let after = upscale.header.next;
    // SAFETY: title's chain.
    backend.header.next = match unsafe { after.as_ref() } {
        Some(h) if h.kind == CONSOLE_BACKEND => h.next,
        _ => after,
    };
    backend.header.kind = HOST_BACKEND;
    upscale.header.next = &raw mut backend.header;
}

fn host_state(state: u32) -> Option<u32> {
    Some(match state {
        0x0 | 0x1 | 0x80 => 0, // common, present
        0x2 => 0x8,            // unordered access
        0x4 => 0x40,           // compute read
        0x8 => 0x80,           // pixel read
        0xc | 0x40 => 0xc0,    // shader read / generic read
        0x10 => 0x800,         // copy source
        0x20 => 0x400,         // copy destination
        0x100 => 0x4,          // render target
        0x200 => 0x10,         // depth write
        _ => return None,
    })
}

fn quality_ratio(mode: u32) -> f32 {
    static FLOOR: OnceLock<f32> = OnceLock::new();
    let min_scale = *FLOOR.get_or_init(|| {
        std::env::var("VICTORY_FSR_SCALE")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map_or(1.0, |s| s.clamp(1.0, 3.0))
    });
    let base: f32 = match mode {
        0 => 1.0, // NativeAA
        1 => 1.5, // Quality
        2 => 1.7, // Balanced
        3 => 2.0, // Performance
        4 => 3.0, // Ultra Performance
        _ => 1.5,
    };
    base.max(min_scale)
}

extern "C" fn ffxCreateContext(
    context: *mut *mut c_void,
    desc: *mut Header,
    memory: *const c_void,
) -> u32 {
    let Some(mode) = MODE.get() else {
        return NO_PROVIDER;
    };
    if context.is_null() || desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: title's description chain.
    let kind = unsafe { (*desc).kind };
    if kind != CREATE_UPSCALE {
        once!(warn, kind = format_args!("{kind:#x}"), "FidelityFX: non-upscaler context refused");
        return NO_PROVIDER;
    }
    // SAFETY: CreateUpscale header.
    let mut upscale = unsafe { desc.cast::<CreateUpscale>().read() };
    match mode {
        Mode::Builtin => {
            // SAFETY: checked non-null.
            unsafe { *context = BUILTIN_CTX as *mut c_void };
            tracing::warn!(
                render = ?upscale.max_render,
                output = ?upscale.max_output,
                flags = format_args!("{:#x}", upscale.flags),
                "FidelityFX: built-in upscaler context created"
            );
            OK
        }
        Mode::External(host) => {
            let mut backend = Backend {
                header: Header { kind: HOST_BACKEND, next: null_mut() },
                device: host.device,
            };
            link(&mut upscale, &mut backend);
            // SAFETY: PC chain.
            let code = unsafe { (host.create)(context, &mut upscale.header, memory) };
            if code != OK {
                tracing::error!(code, render = ?upscale.max_render, output = ?upscale.max_output, "FidelityFX: host refused upscaler");
            }
            code
        }
    }
}

extern "C" fn ffxDestroyContext(context: *mut *mut c_void, memory: *const c_void) -> u32 {
    match MODE.get() {
        Some(Mode::Builtin) => {
            if !context.is_null() {
                // SAFETY: checked non-null.
                unsafe { *context = null_mut() };
            }
            OK
        }
        Some(Mode::External(host)) => unsafe { (host.destroy)(context, memory) },
        None => NO_PROVIDER,
    }
}

extern "C" fn ffxConfigure(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    match MODE.get() {
        Some(Mode::Builtin) => OK,
        Some(Mode::External(host)) => unsafe { (host.configure)(context, desc) },
        None => NO_PROVIDER,
    }
}

extern "C" fn ffxQuery(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(mode) = MODE.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: title's description.
    let kind = unsafe { (*desc).kind };
    match mode {
        Mode::External(host) => {
            if kind == GET_VERSIONS || kind == MEMORY_USAGE_WITH_DEVICE {
                return PARAMETER;
            }
            // SAFETY: forwarded to external PC DLL.
            unsafe { (host.query)(context, desc) }
        }
        Mode::Builtin => match kind {
            GET_VERSIONS => {
                // SAFETY: GetVersions layout.
                let q = unsafe { &mut *desc.cast::<GetVersions>() };
                if !q.count.is_null() {
                    unsafe { *q.count = 1 };
                }
                if !q.ids.is_null() {
                    unsafe { *q.ids = BUILTIN_PROVIDER_ID };
                }
                if !q.names.is_null() {
                    unsafe { *q.names = BUILTIN_PROVIDER_NAME.as_ptr() };
                }
                OK
            }
            GET_PROVIDER_VERSION => {
                // SAFETY: ProviderVersion layout.
                let q = unsafe { &mut *desc.cast::<ProviderVersion>() };
                q.id = BUILTIN_PROVIDER_ID;
                q.name = BUILTIN_PROVIDER_NAME.as_ptr();
                OK
            }
            QUERY_UPSCALE_RATIO => {
                // SAFETY: UpscaleRatio layout.
                let q = unsafe { &mut *desc.cast::<UpscaleRatio>() };
                if !q.ratio.is_null() {
                    unsafe { *q.ratio = quality_ratio(q.quality_mode) };
                }
                OK
            }
            QUERY_RENDER_RESOLUTION => {
                // SAFETY: RenderResolution layout.
                let q = unsafe { &mut *desc.cast::<RenderResolution>() };
                let r = quality_ratio(q.quality_mode);
                if !q.render_width.is_null() {
                    unsafe { *q.render_width = ((q.display_width as f32 / r).round() as u32).max(1) };
                }
                if !q.render_height.is_null() {
                    unsafe { *q.render_height = ((q.display_height as f32 / r).round() as u32).max(1) };
                }
                OK
            }
            QUERY_JITTER_PHASE_COUNT => {
                // SAFETY: PhaseCount layout.
                let q = unsafe { &mut *desc.cast::<PhaseCount>() };
                if !q.count.is_null() {
                    unsafe { *q.count = 1 };
                }
                OK
            }
            QUERY_JITTER_OFFSET => {
                // SAFETY: JitterOffset layout.
                let q = unsafe { &mut *desc.cast::<JitterOffset>() };
                if !q.x.is_null() {
                    unsafe { *q.x = 0.0 };
                }
                if !q.y.is_null() {
                    unsafe { *q.y = 0.0 };
                }
                OK
            }
            QUERY_GPU_MEMORY_USAGE => {
                // SAFETY: MemoryUsage layout.
                let q = unsafe { &mut *desc.cast::<MemoryUsage>() };
                if !q.usage.is_null() {
                    unsafe { *q.usage = [65_536, 0] };
                }
                OK
            }
            MEMORY_USAGE_WITH_DEVICE => {
                // SAFETY: MemoryUsageWithDevice layout.
                let q = unsafe { &mut *desc.cast::<MemoryUsageWithDevice>() };
                if !q.usage.is_null() {
                    unsafe { *q.usage = [65_536, 0] };
                }
                OK
            }
            _ => {
                once!(warn, kind = format_args!("{kind:#x}"), "FidelityFX builtin: unknown query kind");
                UNKNOWN_TYPE
            }
        },
    }
}

extern "C" fn ffxDispatch(context: *mut *mut c_void, desc: *mut Header) -> u32 {
    let Some(mode) = MODE.get() else {
        return NO_PROVIDER;
    };
    if desc.is_null() {
        return PARAMETER;
    }
    // SAFETY: title's description.
    let kind = unsafe { (*desc).kind };
    if kind != DISPATCH_UPSCALE {
        once!(warn, kind = format_args!("{kind:#x}"), "FidelityFX: non-upscale dispatch refused");
        return UNKNOWN_TYPE;
    }
    // SAFETY: DispatchUpscale layout.
    let mut dispatch = unsafe { desc.cast::<DispatchUpscale>().read() };
    let list = dispatch.list.cast::<Object>().cast_const();
    match mode {
        Mode::Builtin => {
            let color = dispatch.resources[0];
            let output = dispatch.resources[6];
            if color.resource.is_null() || output.resource.is_null() {
                once!(warn, "FidelityFX builtin: dispatch missing color or output resource");
                return PARAMETER;
            }
            let Some(cs) = host_state(color.state) else {
                once!(warn, state = format_args!("{:#x}", color.state), "FidelityFX builtin: unknown color state");
                return PARAMETER;
            };
            let Some(os) = host_state(output.state) else {
                once!(warn, state = format_args!("{:#x}", output.state), "FidelityFX builtin: unknown output state");
                return PARAMETER;
            };
            once!(
                warn,
                render = ?dispatch.render,
                output = ?dispatch.output,
                color_state = format_args!("{:#x}", color.state),
                output_state = format_args!("{:#x}", output.state),
                "FidelityFX: first built-in upscaling dispatch"
            );
            if victory_gpu::external::upscale_builtin(
                list,
                color.resource.cast::<Object>(),
                cs,
                output.resource.cast::<Object>(),
                os,
                dispatch.render,
                dispatch.output,
            ) {
                OK
            } else {
                once!(warn, "FidelityFX builtin: upscale_builtin returned false");
                PARAMETER
            }
        }
        Mode::External(host) => {
            for r in dispatch.resources.iter_mut().filter(|r| !r.resource.is_null()) {
                let wanted = host_state(r.state);
                let Some((raw, _)) = wanted.and_then(|w| {
                    victory_gpu::external::host_texture_raw(list, r.resource.cast::<Object>(), w)
                        .filter(|&(_, s)| s == w)
                }) else {
                    return PARAMETER;
                };
                r.resource = raw;
            }
            victory_gpu::external::record_raw(list, |host_list| {
                dispatch.list = host_list;
                // SAFETY: external PC DLL dispatch.
                unsafe { (host.dispatch)(context, &raw mut dispatch.header) }
            })
            .unwrap_or(PARAMETER)
        }
    }
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "fidelityfx",
        dlls: &["amd_fidelityfx_loader_xbox.dll"],
        exports: vec![
            hle_export!(ffxConfigure),
            hle_export!(ffxCreateContext),
            hle_export!(ffxDestroyContext),
            hle_export!(ffxDispatch),
            hle_export!(ffxQuery),
        ],
    }
}
