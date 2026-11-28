//! Host work that a host library (or the built-in Series S FidelityFX TAAU
//! upscaler) records into a title's command list: the host device, a title
//! texture's host copy in the requested state, the host list itself, and a
//! built-in D3D12 temporal super-resolution compute upscaler (Halton sub-pixel
//! jitter + motion-vector reprojection + YCoCg variance clipping + Karis HDR
//! accumulation + 5-tap Catmull-Rom bicubic + RCAS sharpening; compatible with
//! both normal and `VICTORY_THIN=1` lists).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::object::Object;

/// Inputs forwarded from `amd_fidelityfx_loader_xbox.dll` (`ffxDispatch`) to
/// the built-in GPU compute upscaler.
#[derive(Clone, Copy)]
pub struct UpscaleInput {
    pub color: *const Object,
    pub color_state: u32,
    pub motion: *const Object,
    pub motion_state: u32,
    pub reactive: *const Object,
    pub reactive_state: u32,
    pub output: *const Object,
    pub output_state: u32,
    pub render: [u32; 2],
    pub out_size: [u32; 2],
    pub jitter: [f32; 2],
    pub motion_scale: [f32; 2],
    pub flags: u32,
    pub reset: bool,
}

/// The host D3D12 device as a raw COM pointer (`ID3D12Device`), created on
/// first use; `None` without a host GPU.
pub fn host_device_raw() -> Option<*mut c_void> {
    crate::host::host().map(|h| h.device.as_raw())
}

/// A title texture's host copy, moved to the host resource `state` in the
/// title list `list`, and the state it is in (`state` without bits the host
/// copy cannot take). Returns the raw `ID3D12Resource*` and its legal host state.
pub fn host_texture_raw(
    list: *const Object,
    resource: *const Object,
    state: u32,
) -> Option<(*mut c_void, u32)> {
    let host = crate::list::host_list(list)?;
    let texture = crate::resource::host_resource(resource)?;
    crate::list::ensure_state(host, resource, state);
    let state = crate::host::legal_state(texture, state);
    Some((texture.as_raw(), state))
}

/// Runs `f` with the raw `ID3D12GraphicsCommandList*` behind title list `list`.
pub fn record_raw<R>(list: *const Object, f: impl FnOnce(*mut c_void) -> R) -> Option<R> {
    let mut out = None;
    crate::list::with_state(list, |host, state| {
        out = Some(f(host.list.as_raw()));
        state.host_state_lost();
    });
    out
}

fn taau_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_FSR_TAAU").is_none_or(|v| v != "0"))
}

const UPSCALE_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=16, b0), DescriptorTable(SRV(t0, numDescriptors=4), UAV(u0, numDescriptors=2)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer Params : register(b0) {
    float2 uv_scale;
    uint2  out_size;
    float2 inv_src;
    float2 inv_out;
    float2 jitter_uv;
    float2 mv_uv_scale;
    float2 mv_uv_max;
    float  sharpness;
    uint   flags; // bit 0: has_history, bit 1: has_motion, bit 2: has_reactive
};
Texture2D<float4> src          : register(t0);
Texture2D<float4> prev_hist    : register(t1);
Texture2D<float4> motion_tex   : register(t2);
Texture2D<float4> reactive_tex : register(t3);
RWTexture2D<float4> dst        : register(u0);
RWTexture2D<float3> hist_out   : register(u1);
SamplerState samp              : register(s0);

// Treat invalid components independently so an overflow in one channel cannot
// wipe out the colour in the other two. Keep finite scene values above 1 for
// the game's subsequent exposure and tone mapping passes.
float3 safe_hdr(float3 value, float3 fallback) {
    return isfinite(value) ? clamp(value, 0.0, 65504.0) : fallback;
}

float luma_rgb(float3 c) {
    return dot(c, float3(0.299, 0.587, 0.114));
}

// Karis tone-map for flicker-free HDR temporal blending
float3 tonemap(float3 c) {
    float l = max(luma_rgb(c), 0.0);
    return c / (1.0 + l);
}

float3 inv_tonemap(float3 c) {
    float l = clamp(luma_rgb(c), 0.0, 0.9995);
    return clamp(c / max(1.0 - l, 1e-4), 0.0, 65504.0);
}

float3 rgb_to_ycocg(float3 c) {
    float y  =  0.25 * c.r + 0.50 * c.g + 0.25 * c.b;
    float co =  0.50 * c.r              - 0.50 * c.b;
    float cg = -0.25 * c.r + 0.50 * c.g - 0.25 * c.b;
    return float3(y, co, cg);
}

float3 ycocg_to_rgb(float3 ycc) {
    float t = ycc.x - ycc.z;
    return max(float3(t + ycc.y, ycc.x + ycc.z, t - ycc.y), 0.0);
}

// Bilinear sampling may already have propagated one invalid texel into the
// sample. Reconstruct the affected channels from the finite taps, keeping
// the original weights. This path is only needed on invalid centre samples.
float3 recover_center(float2 uv) {
    int2 last = max(int2(round(uv_scale / inv_src)) - 1, int2(0, 0));
    float2 at = uv / inv_src - 0.5;
    int2 base = int2(floor(at));
    float2 weight = frac(at);
    float3 sum = 0.0;
    float3 total = 0.0;
    [unroll] for (uint y = 0; y < 2; ++y) {
        [unroll] for (uint x = 0; x < 2; ++x) {
            int2 pos = clamp(base + int2(x, y), int2(0, 0), last);
            float3 value = src.Load(int3(pos, 0)).rgb;
            float w = (x == 0 ? 1.0 - weight.x : weight.x)
                    * (y == 0 ? 1.0 - weight.y : weight.y);
            float3 valid_weight = isfinite(value) ? w.xxx : float3(0.0, 0.0, 0.0);
            sum += safe_hdr(value, 0.0) * valid_weight;
            total += valid_weight;
        }
    }
    return sum / max(total, 1e-6);
}

// Filmic 5-bilinear-tap Catmull-Rom bicubic reconstruction
bool sample_catmull_rom(Texture2D<float4> tex, float2 uv, float2 inv_dim, float2 min_uv, float2 max_uv, out float3 out_rgb) {
    float2 pos = uv / inv_dim;
    float2 tc1 = floor(pos - 0.5) + 0.5;
    float2 f = pos - tc1;
    float2 f2 = f * f;
    float2 f3 = f2 * f;
    float2 w0 = f2 - 0.5 * (f3 + f);
    float2 w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
    float2 w3 = 0.5 * (f3 - f2);
    float2 w2 = 1.0 - w0 - w1 - w3;
    float2 w12 = w1 + w2;
    float2 off12 = w2 / max(w12, 1e-6);

    float2 uv0  = clamp((tc1 - 1.0) * inv_dim, min_uv, max_uv);
    float2 uv3  = clamp((tc1 + 2.0) * inv_dim, min_uv, max_uv);
    float2 uv12 = clamp((tc1 + off12) * inv_dim, min_uv, max_uv);

    float3 t0 = tex.SampleLevel(samp, float2(uv12.x, uv0.y), 0.0).rgb;
    float3 t1 = tex.SampleLevel(samp, float2(uv0.x, uv12.y), 0.0).rgb;
    float3 t2 = tex.SampleLevel(samp, float2(uv12.x, uv12.y), 0.0).rgb;
    float3 t3 = tex.SampleLevel(samp, float2(uv3.x, uv12.y), 0.0).rgb;
    float3 t4 = tex.SampleLevel(samp, float2(uv12.x, uv3.y), 0.0).rgb;
    if (!(all(isfinite(t0)) && all(isfinite(t1)) && all(isfinite(t2)) && all(isfinite(t3)) && all(isfinite(t4)))) {
        out_rgb = 0.0;
        return false;
    }
    float wt0 = w12.x * w0.y;
    float wt1 = w0.x * w12.y;
    float wt2 = w12.x * w12.y;
    float wt3 = w3.x * w12.y;
    float wt4 = w12.x * w3.y;
    float wsum = wt0 + wt1 + wt2 + wt3 + wt4;
    out_rgb = (t0 * wt0 + t1 * wt1 + t2 * wt2 + t3 * wt3 + t4 * wt4) / max(wsum, 1e-6);
    return true;
}

// Temporal Super-Resolution (TAAU): Halton unjitter + 5-tap Catmull-Rom +
// Karis HDR YCoCg variance clipping + dilated motion reprojection + RCAS
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= out_size.x || id.y >= out_size.y) return;

    float2 out_uv = (float2(id.xy) + 0.5) * inv_out;
    float2 min_uv = inv_src * 0.5;
    float2 max_uv = max(uv_scale - inv_src * 0.5, min_uv);
    float2 src_uv = clamp(out_uv * uv_scale + jitter_uv, min_uv, max_uv);

    float4 bilin = src.SampleLevel(samp, src_uv, 0.0);
    float3 fallback = 0.0;
    bool center_finite = all(isfinite(bilin.rgb));
    [branch] if (!center_finite) fallback = recover_center(src_uv);
    float3 base_rgb = safe_hdr(bilin.rgb, fallback);
    float base_a = isfinite(bilin.a) ? saturate(bilin.a) : 1.0;

    // 3x3 source neighborhood for anti-ringing bounds, RCAS, and YCoCg variance AABB
    float3 n  = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2( 0.0,      -inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);
    float3 s  = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2( 0.0,       inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);
    float3 w  = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2(-inv_src.x,  0.0     ), min_uv, max_uv), 0.0).rgb, base_rgb);
    float3 e  = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2( inv_src.x,  0.0     ), min_uv, max_uv), 0.0).rgb, base_rgb);
    float3 mn = min(base_rgb, min(min(n, s), min(w, e)));
    float3 mx = max(base_rgb, max(max(n, s), max(w, e)));

    float3 curr_rgb = base_rgb;
    if (center_finite) {
        float3 bicubic;
        if (sample_catmull_rom(src, src_uv, inv_src, min_uv, max_uv, bicubic)) {
            curr_rgb = clamp(safe_hdr(bicubic, base_rgb), mn, mx);
        }
    }

    float3 accum_rgb = curr_rgb;
    if ((flags & 1u) != 0u) {
        float3 nw = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2(-inv_src.x, -inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);
        float3 ne = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2( inv_src.x, -inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);
        float3 sw = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2(-inv_src.x,  inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);
        float3 se = safe_hdr(src.SampleLevel(samp, clamp(src_uv + float2( inv_src.x,  inv_src.y), min_uv, max_uv), 0.0).rgb, base_rgb);

        float3 y0 = rgb_to_ycocg(tonemap(base_rgb));
        float3 y1 = rgb_to_ycocg(tonemap(n));
        float3 y2 = rgb_to_ycocg(tonemap(s));
        float3 y3 = rgb_to_ycocg(tonemap(w));
        float3 y4 = rgb_to_ycocg(tonemap(e));
        float3 y5 = rgb_to_ycocg(tonemap(nw));
        float3 y6 = rgb_to_ycocg(tonemap(ne));
        float3 y7 = rgb_to_ycocg(tonemap(sw));
        float3 y8 = rgb_to_ycocg(tonemap(se));

        float3 m1 = (y0 + y1 + y2 + y3 + y4 + y5 + y6 + y7 + y8) * (1.0 / 9.0);
        float3 m2 = (y0*y0 + y1*y1 + y2*y2 + y3*y3 + y4*y4 + y5*y5 + y6*y6 + y7*y7 + y8*y8) * (1.0 / 9.0);
        float3 sigma = sqrt(max(m2 - m1 * m1, 0.0));
        float3 box_min = min(y0, min(min(min(y1, y2), min(y3, y4)), min(min(y5, y6), min(y7, y8))));
        float3 box_max = max(y0, max(max(max(y1, y2), max(y3, y4)), max(max(y5, y6), max(y7, y8))));
        const float GAMMA = 1.15;
        float3 clip_min = max(box_min, m1 - GAMMA * sigma);
        float3 clip_max = min(box_max, m1 + GAMMA * sigma);

        // 5-tap cross dilated motion vector (picks largest velocity to avoid moving-edge trails)
        float2 mv_uv = 0.0;
        float2 mv_uv_sample = clamp(out_uv * mv_uv_max, 0.0, mv_uv_max);
        if ((flags & 2u) != 0u) {
            float2 mv_step = inv_src;
            float2 mv_c = motion_tex.SampleLevel(samp, mv_uv_sample, 0.0).xy;
            float2 mv_n = motion_tex.SampleLevel(samp, clamp(mv_uv_sample + float2(0.0, -mv_step.y), 0.0, mv_uv_max), 0.0).xy;
            float2 mv_s = motion_tex.SampleLevel(samp, clamp(mv_uv_sample + float2(0.0,  mv_step.y), 0.0, mv_uv_max), 0.0).xy;
            float2 mv_w = motion_tex.SampleLevel(samp, clamp(mv_uv_sample + float2(-mv_step.x, 0.0), 0.0, mv_uv_max), 0.0).xy;
            float2 mv_e = motion_tex.SampleLevel(samp, clamp(mv_uv_sample + float2( mv_step.x, 0.0), 0.0, mv_uv_max), 0.0).xy;
            float2 best = all(isfinite(mv_c)) ? mv_c : 0.0;
            float best_sq = dot(best, best);
            if (all(isfinite(mv_n)) && dot(mv_n, mv_n) > best_sq) { best = mv_n; best_sq = dot(mv_n, mv_n); }
            if (all(isfinite(mv_s)) && dot(mv_s, mv_s) > best_sq) { best = mv_s; best_sq = dot(mv_s, mv_s); }
            if (all(isfinite(mv_w)) && dot(mv_w, mv_w) > best_sq) { best = mv_w; best_sq = dot(mv_w, mv_w); }
            if (all(isfinite(mv_e)) && dot(mv_e, mv_e) > best_sq) { best = mv_e; }
            mv_uv = best * mv_uv_scale;
        }

        float2 prev_uv = out_uv + mv_uv;
        if (all(prev_uv >= 0.0) && all(prev_uv <= 1.0)) {
            float2 h_min = inv_out * 0.5;
            float2 h_max = max(1.0 - h_min, h_min);
            float3 hist_rgb;
            if (!sample_catmull_rom(prev_hist, clamp(prev_uv, h_min, h_max), inv_out, h_min, h_max, hist_rgb)) {
                hist_rgb = safe_hdr(prev_hist.SampleLevel(samp, clamp(prev_uv, h_min, h_max), 0.0).rgb, curr_rgb);
            }
            hist_rgb = safe_hdr(hist_rgb, curr_rgb);

            float3 hist_ycc = rgb_to_ycocg(tonemap(hist_rgb));
            float3 clamped_ycc = clamp(hist_ycc, clip_min, clip_max);
            float clip_dist = length(hist_ycc - clamped_ycc);

            float speed_px = length(mv_uv * float2(out_size));
            float alpha = lerp(0.10, 0.28, saturate(speed_px / 12.0));
            alpha = saturate(alpha + 0.15 * saturate(clip_dist * 4.0));
            if ((flags & 4u) != 0u) {
                float react = reactive_tex.SampleLevel(samp, mv_uv_sample, 0.0).r;
                if (isfinite(react)) {
                    alpha = lerp(alpha, 0.85, saturate(react));
                }
            }

            float3 hist_tm = ycocg_to_rgb(clamped_ycc);
            float3 curr_tm = tonemap(curr_rgb);
            float3 blended_tm = lerp(hist_tm, curr_tm, alpha);
            accum_rgb = inv_tonemap(blended_tm);
        }
    }

    // Store unsharpened temporal history to avoid recursive ringing across frames
    hist_out[id.xy] = safe_hdr(accum_rgb, curr_rgb);

    float3 out_rgb = accum_rgb;
    if (sharpness > 0.001) {
        float3 avg = (n + s + w + e) * 0.25;
        float3 rc_mn = min(accum_rgb, mn);
        float3 rc_mx = max(accum_rgb, mx);
        float gain = ((flags & 1u) != 0u) ? (sharpness * 0.90) : (sharpness * 1.25);
        float3 sharp = accum_rgb + (curr_rgb - avg) * gain;
        out_rgb = clamp(sharp, rc_mn, rc_mx);
    }
    dst[id.xy] = float4(out_rgb, base_a);
}
"#;

const UPSCALE_SLOTS: u32 = 128;
const SLOTS_PER_DISPATCH: u32 = 6; // t0..t3 + u0..u1
const HISTORY_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R11G11B10_FLOAT;

struct HistoryPair {
    size: [u32; 2],
    tex: [ID3D12Resource; 2],
    cur: usize,
    valid: bool,
}

struct BuiltinUpscaler {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    dummy_zero: ID3D12Resource,
    dummy_out: ID3D12Resource,
    history: Mutex<Option<HistoryPair>>,
    step: u32,
    next: AtomicU32,
}

// SAFETY: D3D12 PSO, root signature, resources, and descriptor heap are
// free-threaded; `next` allocates distinct descriptor tables per dispatch,
// and `history` is serialized behind a Mutex.
unsafe impl Send for BuiltinUpscaler {}
unsafe impl Sync for BuiltinUpscaler {}

fn make_default_tex2d(
    device: &ID3D12Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    initial: D3D12_RESOURCE_STATES,
) -> windows::core::Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(width.max(1)),
        Height: height.max(1),
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: flags,
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: valid default heap and 2D texture description.
    unsafe {
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, initial, None, &mut out)?;
    }
    out.ok_or_else(windows::core::Error::empty)
}

impl BuiltinUpscaler {
    fn new(device: &ID3D12Device) -> windows::core::Result<Self> {
        use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
        use windows::Win32::Graphics::Direct3D::ID3DBlob;
        let mut blob: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        // SAFETY: static HLSL source and entry names outlive D3DCompile.
        let compiled = unsafe {
            D3DCompile(
                UPSCALE_HLSL.as_ptr().cast(),
                UPSCALE_HLSL.len(),
                windows::core::s!("ffx_builtin_taau"),
                None,
                None,
                windows::core::s!("main"),
                windows::core::s!("cs_5_1"),
                0,
                0,
                &mut blob,
                Some(&mut err),
            )
        };
        if let Err(e) = compiled {
            if let Some(err) = err {
                // SAFETY: compiler diagnostic blob.
                let msg = unsafe {
                    std::slice::from_raw_parts(
                        err.GetBufferPointer().cast::<u8>(),
                        err.GetBufferSize(),
                    )
                };
                tracing::warn!(
                    hlsl_error = %String::from_utf8_lossy(msg),
                    "FidelityFX built-in TAAU HLSL compile error"
                );
            }
            return Err(e);
        }
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        let dummy_zero = make_default_tex2d(
            device,
            1,
            1,
            DXGI_FORMAT_R16G16_FLOAT,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        )?;
        let dummy_out = make_default_tex2d(
            device,
            1,
            1,
            HISTORY_FORMAT,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        )?;
        // SAFETY: compiled bytecode blob owned for this call.
        unsafe {
            let bytes = std::slice::from_raw_parts(
                blob.GetBufferPointer().cast::<u8>(),
                blob.GetBufferSize(),
            );
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes)?;
            let pso: ID3D12PipelineState =
                device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })?;
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: UPSCALE_SLOTS * SLOTS_PER_DISPATCH,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let step = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            Ok(Self {
                root,
                pso,
                heap,
                dummy_zero,
                dummy_out,
                history: Mutex::new(None),
                step,
                next: AtomicU32::new(0),
            })
        }
    }

    /// Acquires the `(prev_history, next_history, prev_is_valid)` textures for
    /// `out_size`, creating or resizing the ping-pong `R11G11B10_FLOAT` pair
    /// on demand (~7 MiB at 720p, ~15.8 MiB at 1080p).
    fn acquire_history(
        &self,
        device: &ID3D12Device,
        out_size: [u32; 2],
        reset: bool,
    ) -> Option<(ID3D12Resource, ID3D12Resource, bool)> {
        let mut guard = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let needs_new = guard.as_ref().is_none_or(|h| h.size != out_size);
        if needs_new {
            let t0 = make_default_tex2d(
                device,
                out_size[0],
                out_size[1],
                HISTORY_FORMAT,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )
            .ok()?;
            let t1 = make_default_tex2d(
                device,
                out_size[0],
                out_size[1],
                HISTORY_FORMAT,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )
            .ok()?;
            if let Some(old) = guard.replace(HistoryPair {
                size: out_size,
                tex: [t0, t1],
                cur: 0,
                valid: false,
            }) {
                let [a, b] = old.tex;
                crate::host::retire_resource(a);
                crate::host::retire_resource(b);
            }
        }
        let pair = guard.as_mut()?;
        let prev_valid = pair.valid && !reset;
        let prev_idx = pair.cur;
        let next_idx = 1 - prev_idx;
        pair.cur = next_idx;
        pair.valid = true;
        Some((pair.tex[prev_idx].clone(), pair.tex[next_idx].clone(), prev_valid))
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        device: &ID3D12Device,
        cmd: &ID3D12GraphicsCommandList,
        (src, src_format): (&ID3D12Resource, DXGI_FORMAT),
        (prev_hist, prev_hist_format): (&ID3D12Resource, DXGI_FORMAT),
        (motion, motion_format): (&ID3D12Resource, DXGI_FORMAT),
        (reactive, reactive_format): (&ID3D12Resource, DXGI_FORMAT),
        (out, out_format): (&ID3D12Resource, DXGI_FORMAT),
        (next_hist, next_hist_format): (&ID3D12Resource, DXGI_FORMAT),
        params: &[u32; 16],
    ) {
        let slot = self.next.fetch_add(1, Ordering::Relaxed) % UPSCALE_SLOTS;
        // SAFETY: an open list and live textures in their shader access states.
        unsafe {
            let base_ptr = self.heap.GetCPUDescriptorHandleForHeapStart().ptr
                + (slot * SLOTS_PER_DISPATCH * self.step) as usize;
            let cpu = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: base_ptr + i * self.step as usize,
            };
            let mut gpu_base = self.heap.GetGPUDescriptorHandleForHeapStart();
            gpu_base.ptr += u64::from(slot * SLOTS_PER_DISPATCH * self.step);

            let srv_desc = |format: DXGI_FORMAT| D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            let uav_desc = |format: DXGI_FORMAT| D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: format,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_UAV::default(),
                },
            };

            device.CreateShaderResourceView(src, Some(&srv_desc(src_format)), cpu(0));
            device.CreateShaderResourceView(prev_hist, Some(&srv_desc(prev_hist_format)), cpu(1));
            device.CreateShaderResourceView(motion, Some(&srv_desc(motion_format)), cpu(2));
            device.CreateShaderResourceView(reactive, Some(&srv_desc(reactive_format)), cpu(3));
            device.CreateUnorderedAccessView(out, None, Some(&uav_desc(out_format)), cpu(4));
            device.CreateUnorderedAccessView(
                next_hist,
                None,
                Some(&uav_desc(next_hist_format)),
                cpu(5),
            );

            cmd.SetPipelineState(&self.pso);
            cmd.SetComputeRootSignature(&self.root);
            cmd.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            cmd.SetComputeRoot32BitConstants(0, 16, params.as_ptr().cast(), 0);
            cmd.SetComputeRootDescriptorTable(1, gpu_base);
            cmd.Dispatch(params[2].div_ceil(8), params[3].div_ceil(8), 1);
        }
    }
}

fn view_format(fmt: DXGI_FORMAT) -> DXGI_FORMAT {
    match fmt {
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        DXGI_FORMAT_R32G32B32A32_TYPELESS => DXGI_FORMAT_R32G32B32A32_FLOAT,
        DXGI_FORMAT_R16G16_TYPELESS => DXGI_FORMAT_R16G16_FLOAT,
        DXGI_FORMAT_R32G32_TYPELESS => DXGI_FORMAT_R32G32_FLOAT,
        DXGI_FORMAT_R16_TYPELESS => DXGI_FORMAT_R16_FLOAT,
        DXGI_FORMAT_R32_TYPELESS => DXGI_FORMAT_R32_FLOAT,
        DXGI_FORMAT_R8_TYPELESS => DXGI_FORMAT_R8_UNORM,
        DXGI_FORMAT_R8G8_TYPELESS => DXGI_FORMAT_R8G8_UNORM,
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            DXGI_FORMAT_R8G8B8A8_UNORM
        }
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            DXGI_FORMAT_B8G8R8A8_UNORM
        }
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        other => other,
    }
}

fn srv_compatible(fmt: DXGI_FORMAT) -> bool {
    !matches!(
        fmt,
        DXGI_FORMAT_UNKNOWN
            | DXGI_FORMAT_D16_UNORM
            | DXGI_FORMAT_D24_UNORM_S8_UINT
            | DXGI_FORMAT_D32_FLOAT
            | DXGI_FORMAT_D32_FLOAT_S8X24_UINT
    )
}

/// Built-in zero-dependency D3D12 compute temporal upscaler (TAAU / FSR-style)
/// for FidelityFX `ffxDispatch(DISPATCH_UPSCALE)`. Works with both immediate
/// recording and `VICTORY_THIN=1` deferred multi-threaded list translation.
pub fn upscale_builtin(list: *const Object, input: &UpscaleInput) -> bool {
    if list.is_null() || input.color.is_null() || input.output.is_null() {
        return false;
    }
    if crate::list::thin::record_upscale(list, input) {
        return true;
    }
    upscale_builtin_now(list, input)
}

pub(crate) fn upscale_builtin_now(list: *const Object, input: &UpscaleInput) -> bool {
    let Some(host) = crate::host::host() else {
        return false;
    };
    static UPSCALER: OnceLock<Option<BuiltinUpscaler>> = OnceLock::new();
    let Some(up) = UPSCALER
        .get_or_init(|| match BuiltinUpscaler::new(&host.device) {
            Ok(u) => {
                tracing::warn!(
                    taau = taau_enabled(),
                    "FidelityFX built-in GPU temporal TAAU upscaler ready"
                );
                Some(u)
            }
            Err(e) => {
                tracing::warn!(error = %e, "FidelityFX built-in upscaler failed to compile");
                None
            }
        })
        .as_ref()
    else {
        return false;
    };

    let Some(host_list) = crate::list::host_list(list) else {
        return false;
    };
    let Some(color_tex) = crate::resource::host_resource(input.color) else {
        return false;
    };
    let Some(out_tex) = crate::resource::host_resource(input.output) else {
        return false;
    };

    // SAFETY: live D3D12 textures and open host command list.
    let (src_desc, out_desc) = unsafe { (color_tex.GetDesc(), out_tex.GetDesc()) };
    if src_desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || out_desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || (out_desc.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0) == 0
    {
        static BAD: std::sync::Once = std::sync::Once::new();
        BAD.call_once(|| {
            tracing::warn!(
                src_dim = src_desc.Dimension.0,
                out_dim = out_desc.Dimension.0,
                src_fmt = src_desc.Format.0,
                out_fmt = out_desc.Format.0,
                out_flags = format_args!("{:#x}", out_desc.Flags.0),
                "FidelityFX built-in upscaler skipped incompatible resource"
            );
        });
        return false;
    }
    let src_w = (src_desc.Width as u32).max(1);
    let src_h = src_desc.Height.max(1);
    let ren_w = if input.render[0] > 0 {
        input.render[0].min(src_w)
    } else {
        src_w
    };
    let ren_h = if input.render[1] > 0 {
        input.render[1].min(src_h)
    } else {
        src_h
    };
    let out_w = if input.out_size[0] > 0 {
        input.out_size[0].min(out_desc.Width as u32)
    } else {
        out_desc.Width as u32
    }
    .max(1);
    let out_h = if input.out_size[1] > 0 {
        input.out_size[1].min(out_desc.Height)
    } else {
        out_desc.Height
    }
    .max(1);

    const SRV_CS: u32 = 0x40;
    const UAV: u32 = 0x8;
    let src_wanted = if input.color_state & SRV_CS != 0 {
        input.color_state
    } else {
        SRV_CS
    };
    crate::list::ensure_state(host_list, input.color, src_wanted);
    crate::list::ensure_state(host_list, input.output, UAV);

    let taau_on = taau_enabled();
    let history = if taau_on {
        up.acquire_history(&host.device, [out_w, out_h], input.reset)
    } else {
        None
    };

    // Optional motion vectors (t2)
    let mut motion_info: Option<(&ID3D12Resource, DXGI_FORMAT, [u32; 2])> = None;
    if taau_on && !input.motion.is_null() {
        if let Some(mtex) = crate::resource::host_resource(input.motion) {
            // SAFETY: live D3D12 resource.
            let mdesc = unsafe { mtex.GetDesc() };
            let mfmt = view_format(mdesc.Format);
            if mdesc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && srv_compatible(mfmt) {
                let mwanted = if input.motion_state & SRV_CS != 0 {
                    input.motion_state
                } else {
                    SRV_CS
                };
                crate::list::ensure_state(host_list, input.motion, mwanted);
                motion_info = Some((mtex, mfmt, [(mdesc.Width as u32).max(1), mdesc.Height.max(1)]));
            }
        }
    }

    // Optional reactive mask (t3)
    let mut reactive_info: Option<(&ID3D12Resource, DXGI_FORMAT)> = None;
    if taau_on && !input.reactive.is_null() {
        if let Some(rtex) = crate::resource::host_resource(input.reactive) {
            // SAFETY: live D3D12 resource.
            let rdesc = unsafe { rtex.GetDesc() };
            let rfmt = view_format(rdesc.Format);
            if rdesc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && srv_compatible(rfmt) {
                let rwanted = if input.reactive_state & SRV_CS != 0 {
                    input.reactive_state
                } else {
                    SRV_CS
                };
                crate::list::ensure_state(host_list, input.reactive, rwanted);
                reactive_info = Some((rtex, rfmt));
            }
        }
    }

    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        tracing::warn!(
            taau = taau_on,
            src_fmt = src_desc.Format.0,
            out_fmt = out_desc.Format.0,
            motion_fmt = motion_info.map(|m| m.1.0),
            reactive_fmt = reactive_info.map(|r| r.1.0),
            src = ?[src_w, src_h],
            render = ?[ren_w, ren_h],
            output = ?[out_w, out_h],
            jitter = ?input.jitter,
            motion_scale = ?input.motion_scale,
            create_flags = format_args!("{:#x}", input.flags),
            "FidelityFX built-in TAAU upscaler executing on GPU"
        );
    });

    static SHARPEN: OnceLock<f32> = OnceLock::new();
    let sharpness = *SHARPEN.get_or_init(|| {
        std::env::var("VICTORY_FSR_SHARPEN")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(0.50)
            .clamp(0.0, 1.0)
    });

    let display_res_mv = (input.flags & 0x02) != 0;
    let (mv_domain_w, mv_domain_h) = if display_res_mv {
        (out_w as f32, out_h as f32)
    } else {
        (ren_w as f32, ren_h as f32)
    };
    let [msx, msy] = input.motion_scale;
    let (msx, msy) = if msx.is_finite() && msy.is_finite() && (msx.abs() > 1e-6 || msy.abs() > 1e-6)
    {
        (msx, msy)
    } else {
        (1.0, 1.0)
    };
    let mv_uv_max = match motion_info {
        Some((_, _, [mw, mh])) => {
            [(mv_domain_w / mw as f32).min(1.0), (mv_domain_h / mh as f32).min(1.0)]
        }
        None => [1.0, 1.0],
    };
    let jx = if input.jitter[0].is_finite() {
        input.jitter[0].clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let jy = if input.jitter[1].is_finite() {
        input.jitter[1].clamp(-1.0, 1.0)
    } else {
        0.0
    };

    let has_history = history.as_ref().is_some_and(|h| h.2);
    let mut shader_flags = 0u32;
    if has_history {
        shader_flags |= 1;
    }
    if motion_info.is_some() {
        shader_flags |= 2;
    }
    if reactive_info.is_some() {
        shader_flags |= 4;
    }

    let params = [
        (ren_w as f32 / src_w as f32).to_bits(),
        (ren_h as f32 / src_h as f32).to_bits(),
        out_w,
        out_h,
        (1.0f32 / src_w as f32).to_bits(),
        (1.0f32 / src_h as f32).to_bits(),
        (1.0f32 / out_w as f32).to_bits(),
        (1.0f32 / out_h as f32).to_bits(),
        (jx / src_w as f32).to_bits(),
        (jy / src_h as f32).to_bits(),
        (msx / mv_domain_w).to_bits(),
        (msy / mv_domain_h).to_bits(),
        mv_uv_max[0].to_bits(),
        mv_uv_max[1].to_bits(),
        sharpness.to_bits(),
        shader_flags,
    ];

    crate::list::with_state(list, |hlist, state| {
        let (prev_res, prev_fmt, next_res, next_fmt) = match &history {
            Some((prev, next, _)) => {
                hlist.keep(prev);
                hlist.keep(next);
                (prev, HISTORY_FORMAT, next, HISTORY_FORMAT)
            }
            None => {
                hlist.keep(&up.dummy_out);
                (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT, &up.dummy_out, HISTORY_FORMAT)
            }
        };
        hlist.keep(&up.dummy_zero);
        let (m_res, m_fmt) = match motion_info {
            Some((r, f, _)) => (r, f),
            None => (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
        };
        let (r_res, r_fmt) = match reactive_info {
            Some((r, f)) => (r, f),
            None => (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
        };

        let to_uav = crate::host::transition(
            next_res,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
        // SAFETY: `next_res` always enters and leaves in NON_PIXEL_SHADER_RESOURCE.
        unsafe { hlist.list.ResourceBarrier(&[to_uav]) };
        up.record(
            &host.device,
            &hlist.list,
            (color_tex, view_format(src_desc.Format)),
            (prev_res, prev_fmt),
            (m_res, m_fmt),
            (r_res, r_fmt),
            (out_tex, view_format(out_desc.Format)),
            (next_res, next_fmt),
            &params,
        );
        let to_srv = crate::host::transition(
            next_res,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        );
        // SAFETY: restore `next_res` to NON_PIXEL_SHADER_RESOURCE for the next frame.
        unsafe { hlist.list.ResourceBarrier(&[to_srv]) };
        state.host_state_lost();
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_upscaler_shader_compiles_on_host_device() {
        let Some(host) = crate::host::host() else {
            return;
        };
        BuiltinUpscaler::new(&host.device).expect("built-in TAAU+RCAS shader must compile");
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn builtin_upscaler_preserves_hdr_near_non_finite_texels() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let up = BuiltinUpscaler::new(&host.device).unwrap();
        let format = DXGI_FORMAT_R16G16B16A16_FLOAT;
        let src = make_default_tex2d(
            &host.device,
            2,
            1,
            format,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .unwrap();
        let out = make_default_tex2d(
            &host.device,
            4,
            1,
            format,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )
        .unwrap();
        let (prev_hist, next_hist, _) = up.acquire_history(&host.device, [4, 1], true).unwrap();
        let (upload, data) = host.upload_buffer(256).unwrap();
        // Two FP16 texels: (NaN, 1/16, 4, NaN), then (1/8, 1/32, 2, 1).
        let input = [
            0x7e00u16, 0x2c00, 0x4400, 0x7e00, 0x3000, 0x2800, 0x4000, 0x3c00,
        ];
        let upload_location = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the location borrows the live upload resource.
            pResource: unsafe { std::mem::transmute_copy(&upload) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format,
                        Width: 2,
                        Height: 1,
                        Depth: 1,
                        RowPitch: 256,
                    },
                },
            },
        };
        let src_location = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the location borrows the live texture.
            pResource: unsafe { std::mem::transmute_copy(&src) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let list = host.new_list().unwrap();
        let cmd = &list.list;
        // SAFETY: the mapped upload has room for both texels; the list is open.
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr().cast::<u8>(), data, size_of_val(&input));
            cmd.CopyTextureRegion(&src_location, 0, 0, 0, &upload_location, None);
            cmd.ResourceBarrier(&[
                crate::host::transition(
                    &src,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                ),
                crate::host::transition(
                    &next_hist,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
        }
        let one = 1.0f32.to_bits();
        let half = 0.5f32.to_bits();
        let quarter = 0.25f32.to_bits();
        let zero = 0.0f32.to_bits();
        let params = [
            one, one, 4, 1, half, one, quarter, one, zero, zero, half, one, one, one, half, 0,
        ];
        up.record(
            &host.device,
            cmd,
            (&src, format),
            (&prev_hist, HISTORY_FORMAT),
            (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
            (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
            (&out, format),
            (&next_hist, HISTORY_FORMAT),
            &params,
        );
        // SAFETY: the shader dispatch ended on this list and the textures are live.
        unsafe {
            cmd.ResourceBarrier(&[
                crate::host::transition(
                    &out,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
                crate::host::transition(
                    &next_hist,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                ),
            ]);
        }
        let (readback, footprint) = crate::capture::read_back(&host.device, cmd, &out).unwrap();
        list.close();
        host.execute(&[&list]);
        let mut mapped = std::ptr::null_mut();
        // SAFETY: execute waited and readback covers four FP16x4 texels.
        let pixels = unsafe {
            readback.Map(0, None, Some(&mut mapped)).unwrap();
            let start = mapped.cast::<u8>().add(footprint.Offset as usize);
            let pixels = start.cast::<[[u16; 4]; 4]>().read_unaligned();
            readback.Unmap(0, None);
            pixels
        };
        for &[r, g, b, a] in &pixels[1..3] {
            assert_eq!(r, 0x3000, "recover red from the finite tap: {pixels:x?}");
            assert!(
                (0x2800..=0x2c00).contains(&g),
                "preserve shadow detail: {pixels:x?}"
            );
            assert!(
                (0x4000..=0x4400).contains(&b),
                "preserve HDR highlights: {pixels:x?}"
            );
            assert_eq!(a, 0x3c00);
        }
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn builtin_taau_accumulates_temporal_history_and_resets_cleanly_on_gpu() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let _ = host.log_messages();
        let up = BuiltinUpscaler::new(&host.device).unwrap();
        let (w, h) = (16u32, 16u32);
        let fmt = DXGI_FORMAT_R8G8B8A8_UNORM;
        let src = make_default_tex2d(
            &host.device,
            w,
            h,
            fmt,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        )
        .unwrap();
        let out = make_default_tex2d(
            &host.device,
            w,
            h,
            fmt,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )
        .unwrap();

        let run_frame = |fill_r: u8, reset: bool| -> u8 {
            let pitch = (w * 4).next_multiple_of(256);
            let (upload, data) = host.upload_buffer(u64::from(pitch) * u64::from(h)).unwrap();
            for y in 0..h {
                for x in 0..w {
                    let off = (y * pitch + x * 4) as usize;
                    unsafe {
                        *data.add(off) = fill_r;
                        *data.add(off + 1) = fill_r;
                        *data.add(off + 2) = fill_r;
                        *data.add(off + 3) = 255;
                    }
                }
            }
            let (prev_hist, next_hist, prev_valid) =
                up.acquire_history(&host.device, [w, h], reset).unwrap();
            let list = host.new_list().unwrap();
            let cmd = &list.list;
            let upload_loc = D3D12_TEXTURE_COPY_LOCATION {
                pResource: unsafe { std::mem::transmute_copy(&upload) },
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: fmt,
                            Width: w,
                            Height: h,
                            Depth: 1,
                            RowPitch: pitch,
                        },
                    },
                },
            };
            let src_loc = D3D12_TEXTURE_COPY_LOCATION {
                pResource: unsafe { std::mem::transmute_copy(&src) },
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            unsafe {
                cmd.ResourceBarrier(&[
                    crate::host::transition(
                        &src,
                        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    ),
                    crate::host::transition(
                        &next_hist,
                        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    ),
                ]);
                cmd.CopyTextureRegion(&src_loc, 0, 0, 0, &upload_loc, None);
                cmd.ResourceBarrier(&[crate::host::transition(
                    &src,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                )]);
            }
            let inv = (1.0f32 / w as f32).to_bits();
            let one = 1.0f32.to_bits();
            let zero = 0.0f32.to_bits();
            let params = [
                one,
                one,
                w,
                h,
                inv,
                inv,
                inv,
                inv,
                zero,
                zero,
                inv,
                inv,
                one,
                one,
                zero,
                u32::from(prev_valid),
            ];
            up.record(
                &host.device,
                cmd,
                (&src, fmt),
                (&prev_hist, HISTORY_FORMAT),
                (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
                (&up.dummy_zero, DXGI_FORMAT_R16G16_FLOAT),
                (&out, fmt),
                (&next_hist, HISTORY_FORMAT),
                &params,
            );
            unsafe {
                cmd.ResourceBarrier(&[
                    crate::host::transition(
                        &out,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    ),
                    crate::host::transition(
                        &next_hist,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    ),
                ]);
            }
            let (rb, fp) = crate::capture::read_back(&host.device, cmd, &out).unwrap();
            unsafe {
                cmd.ResourceBarrier(&[crate::host::transition(
                    &out,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )]);
            }
            list.close();
            host.execute(&[&list]);
            let mut ptr = std::ptr::null_mut();
            unsafe { rb.Map(0, None, Some(&mut ptr)).unwrap() };
            let off = fp.Offset as usize + 8 * fp.Footprint.RowPitch as usize + 8 * 4;
            let r = unsafe { *ptr.cast::<u8>().add(off) };
            unsafe { rb.Unmap(0, None) };
            r
        };

        // Frame 0 (cold start): writes exact value (~200).
        let f0 = run_frame(200, true);
        assert!(f0.abs_diff(200) <= 3, "frame 0 expected ~200, got {f0}");
        // Frame 1 (uniform 200 -> 100): tight 3x3 variance clamp bounds history to neighborhood (~100), preventing ghosting!
        let f1 = run_frame(100, false);
        assert!(f1.abs_diff(100) <= 5, "variance clamp must reject ghosting, got {f1}");
        // Frame 2 (reset=true to 240): instant snap to ~240.
        let f2 = run_frame(240, true);
        assert!(f2.abs_diff(240) <= 3, "reset frame expected ~240, got {f2}");
        assert_eq!(host.log_messages(), None, "TAAU must produce zero D3D12 debug layer errors");
    }
}
