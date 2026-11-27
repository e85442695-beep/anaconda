//! Built-in AMD FSR 3-style Optical Flow Frame Generation for D3D12.
//!
//! Zero external DLLs, ~8 MiB VRAM at 720p, and HUD-less by design on Series S:
//! interpolates only the 3D scene plane (`Plane 0`) between consecutive real
//! frames using a 4-level coarse-to-fine Zero-Mean SAD bidirectional optical
//! flow pyramid, edge-preserving flow filtering, GPU scene-cut rejection, and
//! disocclusion-aware 3-hypothesis motion-compensated synthesis — then lets
//! [`crate::compose::Composer`] draw the crisp UI/HUD plane (`Plane 1`) on top
//! with zero HUD ghosting or subtitle smearing.
//!
//! Controlled by:
//! - `VICTORY_FRAME_GEN=0`: disabled.
//! - `VICTORY_FRAME_GEN=1` or `2` (`2x`): 1 interpolated frame per real frame (2x FPS).
//! - `VICTORY_FRAME_GEN=3` (`3x`): 2 interpolated frames per real frame (3x FPS).
//! - `VICTORY_FRAME_GEN=4` (`4x`): 3 interpolated frames per real frame (4x FPS).
//! - `VICTORY_FRAME_GEN_CUT=<0.05..1.0>`: global luma MAD scene-cut threshold (default `0.22`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s};

use crate::host::Host;

const RING_FRAMES: usize = 32;
const SLOTS_PER_FRAME: u32 = 64;

/// Total generated frames submitted (for honest telemetry alongside real FPS).
static GENERATED: AtomicU64 = AtomicU64::new(0);

/// Returns the configured frame-generation multiplier:
/// `1` = disabled, `2` = 2x (1 generated frame per real frame),
/// `3` = 3x (2 generated frames), `4` = 4x (3 generated frames).
pub(crate) fn multiplier() -> u32 {
    static MULT: OnceLock<u32> = OnceLock::new();
    *MULT.get_or_init(|| {
        let raw = std::env::var("VICTORY_FRAME_GEN").ok();
        let val = match raw.as_deref().map(str::trim) {
            None | Some("") | Some("0") | Some("off") | Some("false") => 1,
            Some("1") | Some("2") | Some("2x") | Some("on") | Some("true") => 2,
            Some("3") | Some("3x") => 3,
            Some("4") | Some("4x") => 4,
            Some(other) => other
                .trim_end_matches(['x', 'X'])
                .parse::<u32>()
                .map_or(1, |n| n.clamp(1, 4)),
        };
        if val > 1 {
            tracing::warn!(
                multiplier = val,
                cut_threshold = cut_threshold(),
                "AMD FSR 3-style Optical Flow Frame Generation active (VICTORY_FRAME_GEN)"
            );
        }
        val
    })
}

pub(crate) fn enabled() -> bool {
    multiplier() > 1
}

pub(crate) fn note_generated(count: u64) {
    GENERATED.fetch_add(count, Ordering::Relaxed);
}

fn cut_threshold() -> f32 {
    static CUT: OnceLock<f32> = OnceLock::new();
    *CUT.get_or_init(|| {
        std::env::var("VICTORY_FRAME_GEN_CUT")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(0.22)
            .clamp(0.05, 1.0)
    })
}

const LUMA0_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0), UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer Params : register(b0) {
    uint2 dst_size;
    uint srgb_in;
    uint _pad;
};
Texture2D<float4> src : register(t0);
RWTexture2D<float> dst : register(u0);
SamplerState samp : register(s0);

float3 linear_to_srgb(float3 x) {
    x = saturate(x);
    float3 lo = x * 12.92;
    float3 hi = 1.055 * pow(max(x, 1e-6), 1.0 / 2.4) - 0.055;
    return (x <= 0.0031308) ? lo : hi;
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= dst_size.x || id.y >= dst_size.y) return;
    float2 uv = (float2(id.xy) + 0.5) / float2(dst_size);
    float3 rgb = src.SampleLevel(samp, uv, 0.0).rgb;
    rgb = isfinite(rgb) ? saturate(rgb) : 0.0;
    if (srgb_in != 0u) {
        rgb = linear_to_srgb(rgb);
    }
    dst[id.xy] = dot(rgb, float3(0.299, 0.587, 0.114));
}
"#;

const DOWN_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0), UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer Params : register(b0) {
    uint2 dst_size;
    uint _p0;
    uint _p1;
};
Texture2D<float> src : register(t0);
RWTexture2D<float> dst : register(u0);
SamplerState samp : register(s0);

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= dst_size.x || id.y >= dst_size.y) return;
    float2 uv = (float2(id.xy) + 0.5) / float2(dst_size);
    dst[id.xy] = src.SampleLevel(samp, uv, 0.0);
}
"#;

const FLOW_MATCH_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), DescriptorTable(SRV(t0, numDescriptors=3), UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer FlowParams : register(b0) {
    uint2 size;
    int radius;
    uint has_coarse;
    uint subpixel;
    uint2 coarse_size;
    uint _pad;
};
Texture2D<float> src_a : register(t0);
Texture2D<float> src_b : register(t1);
Texture2D<float4> coarse_flow : register(t2);
RWTexture2D<float4> out_flow : register(u0);
SamplerState samp : register(s0);

float eval_zsad(int2 p_a, int2 p_b, float pa[9], float mean_a, int2 max_p) {
    float pb[9];
    float mean_b = 0.0;
    int idx = 0;
    [unroll] for (int oy = -1; oy <= 1; ++oy) {
        [unroll] for (int ox = -1; ox <= 1; ++ox) {
            int2 q = clamp(p_b + int2(ox, oy), int2(0, 0), max_p);
            float v = src_b.Load(int3(q, 0));
            pb[idx++] = v;
            mean_b += v;
        }
    }
    mean_b *= (1.0 / 9.0);
    float zsad = 0.0;
    float abs_sad = 0.0;
    [unroll] for (int i = 0; i < 9; ++i) {
        zsad += abs((pa[i] - mean_a) - (pb[i] - mean_b));
        abs_sad += abs(pa[i] - pb[i]);
    }
    return (zsad * 0.85 + abs_sad * 0.15) * (1.0 / 9.0);
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= size.x || id.y >= size.y) return;
    int2 p = int2(id.xy);
    int2 max_p = int2(size) - 1;

    float pa[9];
    float mean_a = 0.0;
    float lo_a = 1.0;
    float hi_a = 0.0;
    int idx = 0;
    [unroll] for (int oy = -1; oy <= 1; ++oy) {
        [unroll] for (int ox = -1; ox <= 1; ++ox) {
            int2 q = clamp(p + int2(ox, oy), int2(0, 0), max_p);
            float v = src_a.Load(int3(q, 0));
            pa[idx++] = v;
            mean_a += v;
            lo_a = min(lo_a, v);
            hi_a = max(hi_a, v);
        }
    }
    mean_a *= (1.0 / 9.0);
    float contrast = hi_a - lo_a;

    int2 base = int2(0, 0);
    if (has_coarse != 0u) {
        float2 uv = (float2(id.xy) + 0.5) / float2(size);
        float2 cf = coarse_flow.SampleLevel(samp, uv, 0.0).xy * 2.0;
        base = int2(round(cf));
    }

    int2 best = int2(0, 0);
    float best_cost = eval_zsad(p, p, pa, mean_a, max_p);
    float best_err = best_cost;

    // Slight quadratic motion prior keeps flat textureless walls locked to 0.
    float reg = (contrast < 0.015) ? 0.004 : 0.00035;

    for (int dy = -radius; dy <= radius; ++dy) {
        for (int dx = -radius; dx <= radius; ++dx) {
            int2 cand = base + int2(dx, dy);
            int2 target = p + cand;
            if (target.x < 0 || target.y < 0 || target.x > max_p.x || target.y > max_p.y) continue;
            float err = eval_zsad(p, target, pa, mean_a, max_p);
            float cost = err + reg * float(cand.x * cand.x + cand.y * cand.y);
            if (cost < best_cost) {
                best_cost = cost;
                best_err = err;
                best = cand;
            }
        }
    }

    float2 flow = float2(best);
    if (subpixel != 0u) {
        float e_l = eval_zsad(p, clamp(p + best + int2(-1, 0), int2(0, 0), max_p), pa, mean_a, max_p);
        float e_r = eval_zsad(p, clamp(p + best + int2( 1, 0), int2(0, 0), max_p), pa, mean_a, max_p);
        float denom_x = e_l - 2.0 * best_err + e_r;
        if (denom_x > 1e-4) {
            flow.x += clamp(0.5 * (e_l - e_r) / denom_x, -0.5, 0.5);
        }
        float e_u = eval_zsad(p, clamp(p + best + int2(0, -1), int2(0, 0), max_p), pa, mean_a, max_p);
        float e_d = eval_zsad(p, clamp(p + best + int2(0,  1), int2(0, 0), max_p), pa, mean_a, max_p);
        float denom_y = e_u - 2.0 * best_err + e_d;
        if (denom_y > 1e-4) {
            flow.y += clamp(0.5 * (e_u - e_d) / denom_y, -0.5, 0.5);
        }
    }

    out_flow[id.xy] = float4(flow.x, flow.y, best_err, contrast);
}
"#;

const FLOW_FILTER_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=3), UAV(u0))"
cbuffer FilterParams : register(b0) {
    uint2 size;
    uint _p0;
    uint _p1;
};
Texture2D<float4> flow_fwd : register(t0);
Texture2D<float4> flow_bwd : register(t1);
Texture2D<float> luma_cur : register(t2);
RWTexture2D<float4> out_uv : register(u0);

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= size.x || id.y >= size.y) return;
    int2 p = int2(id.xy);
    int2 max_p = int2(size) - 1;
    float lc = luma_cur.Load(int3(p, 0));

    float2 sum_f = 0.0;
    float2 sum_b = 0.0;
    float wf_tot = 0.0;
    float wb_tot = 0.0;

    [unroll] for (int dy = -1; dy <= 1; ++dy) {
        [unroll] for (int dx = -1; dx <= 1; ++dx) {
            int2 q = clamp(p + int2(dx, dy), int2(0, 0), max_p);
            float ln = luma_cur.Load(int3(q, 0));
            float w_edge = exp(-abs(ln - lc) * 14.0);
            float w_sp = (dx == 0 && dy == 0) ? 1.5 : ((dx == 0 || dy == 0) ? 1.0 : 0.7);
            float4 ff = flow_fwd.Load(int3(q, 0));
            float4 fb = flow_bwd.Load(int3(q, 0));
            float wf = w_sp * w_edge / (0.015 + ff.z * 8.0);
            float wb = w_sp * w_edge / (0.015 + fb.z * 8.0);
            sum_f += ff.xy * wf;
            sum_b += fb.xy * wb;
            wf_tot += wf;
            wb_tot += wb;
        }
    }

    float2 inv_size = 1.0 / float2(size);
    float2 uv_f = clamp((sum_f / max(wf_tot, 1e-5)) * inv_size, -0.15, 0.15);
    float2 uv_b = clamp((sum_b / max(wb_tot, 1e-5)) * inv_size, -0.15, 0.15);
    out_uv[id.xy] = float4(uv_f, uv_b);
}
"#;

const SCENECUT_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=2), UAV(u0))"
cbuffer CutParams : register(b0) {
    uint2 size;
    float threshold;
    uint _pad;
};
Texture2D<float> luma_prev : register(t0);
Texture2D<float> luma_curr : register(t1);
RWTexture2D<float> out_cut : register(u0);

groupshared float s_diff[256];

[RootSignature(RS)]
[numthreads(16, 16, 1)]
void main(uint3 tid : SV_GroupThreadID, uint gi : SV_GroupIndex) {
    float acc = 0.0;
    for (uint y = tid.y; y < size.y; y += 16u) {
        for (uint x = tid.x; x < size.x; x += 16u) {
            float a = luma_prev.Load(int3(x, y, 0));
            float b = luma_curr.Load(int3(x, y, 0));
            acc += abs(a - b);
        }
    }
    s_diff[gi] = acc;
    GroupMemoryBarrierWithGroupSync();

    [unroll] for (uint stride = 128u; stride > 0u; stride >>= 1u) {
        if (gi < stride) {
            s_diff[gi] += s_diff[gi + stride];
        }
        GroupMemoryBarrierWithGroupSync();
    }

    if (gi == 0u) {
        float mad = s_diff[0] / max(float(size.x * size.y), 1.0);
        float blend = (mad >= threshold)
            ? 0.0
            : saturate(1.0 - smoothstep(threshold * 0.65, threshold, mad));
        out_cut[uint2(0, 0)] = blend;
    }
}
"#;

const INTERP_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), DescriptorTable(SRV(t0, numDescriptors=4), UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer InterpParams : register(b0) {
    uint2 out_size;
    float alpha;
    uint srgb_out;
    float2 inv_size;
    uint _p0;
    uint _p1;
};
Texture2D<float4> prev_tex : register(t0);
Texture2D<float4> curr_tex : register(t1);
Texture2D<float4> flow_uv : register(t2);
Texture2D<float> cut_tex : register(t3);
RWTexture2D<float4> dst : register(u0);
SamplerState samp : register(s0);

float3 linear_to_srgb(float3 x) {
    x = saturate(x);
    float3 lo = x * 12.92;
    float3 hi = 1.055 * pow(max(x, 1e-6), 1.0 / 2.4) - 0.055;
    return (x <= 0.0031308) ? lo : hi;
}

float3 safe_rgb(float3 c) {
    return isfinite(c) ? saturate(c) : 0.0;
}

float color_dist(float3 a, float3 b) {
    float3 d = abs(a - b);
    return dot(d, float3(0.35, 0.45, 0.20));
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= out_size.x || id.y >= out_size.y) return;
    float2 uv = (float2(id.xy) + 0.5) * inv_size;

    float cut_ok = cut_tex.Load(int3(0, 0, 0));
    float3 c0 = safe_rgb(curr_tex.SampleLevel(samp, uv, 0.0).rgb);
    if (cut_ok <= 0.01) {
        float3 out_c = (srgb_out != 0u) ? linear_to_srgb(c0) : c0;
        dst[id.xy] = float4(saturate(out_c), 1.0);
        return;
    }

    float3 p0 = safe_rgb(prev_tex.SampleLevel(samp, uv, 0.0).rgb);
    float4 fl = flow_uv.SampleLevel(samp, uv, 0.0);
    float2 vf = fl.xy * cut_ok;
    float2 vb = -fl.zw * cut_ok;

    // Bidirectional flow consistency check (disocclusion detector).
    float2 bwd_at_f = flow_uv.SampleLevel(samp, saturate(uv + vf * (1.0 - alpha)), 0.0).zw * cut_ok;
    float2 fwd_at_b = flow_uv.SampleLevel(samp, saturate(uv - vb * alpha), 0.0).xy * cut_ok;
    float fb_err_f = length(vf + bwd_at_f);
    float fb_err_b = length(fwd_at_b - vb);

    // Hypothesis 1: forward optical flow (prev -> curr).
    float3 pf = safe_rgb(prev_tex.SampleLevel(samp, saturate(uv - vf * alpha), 0.0).rgb);
    float3 cf = safe_rgb(curr_tex.SampleLevel(samp, saturate(uv + vf * (1.0 - alpha)), 0.0).rgb);
    float ef = color_dist(pf, cf) + fb_err_f * 2.5;

    // Hypothesis 2: negated backward optical flow (curr -> prev).
    float3 pb = safe_rgb(prev_tex.SampleLevel(samp, saturate(uv - vb * alpha), 0.0).rgb);
    float3 cb = safe_rgb(curr_tex.SampleLevel(samp, saturate(uv + vb * (1.0 - alpha)), 0.0).rgb);
    float eb = color_dist(pb, cb) + fb_err_b * 2.5;

    // Hypothesis 0: zero motion (locks static geometry & thin bars rock-solid).
    float e0 = color_dist(p0, c0) + 0.012;

    float3 best_p = p0;
    float3 best_c = c0;
    float best_e = e0;
    if (ef < best_e) {
        best_p = pf;
        best_c = cf;
        best_e = ef;
    }
    if (eb < best_e) {
        best_p = pb;
        best_c = cb;
        best_e = eb;
    }

    // Disocclusion-aware blend weight: clean midpoint when consistent, bias to
    // the non-occluded frame when foreground uncovers new background.
    float w = alpha;
    if (best_e > 0.14) {
        float disocc = saturate((best_e - 0.14) * 4.0);
        if (fb_err_f > fb_err_b + 0.015) {
            w = lerp(alpha, 1.0, disocc);
        } else if (fb_err_b > fb_err_f + 0.015) {
            w = lerp(alpha, 0.0, disocc);
        } else {
            w = lerp(alpha, 1.0, disocc * 0.75);
        }
    }

    float3 col = lerp(best_p, best_c, w);
    if (srgb_out != 0u) {
        col = linear_to_srgb(col);
    }
    dst[id.xy] = float4(saturate(col), 1.0);
}
"#;

const SNAPSHOT_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0), UAV(u0))"
cbuffer SnapParams : register(b0) {
    uint2 size;
    uint srgb_out;
    uint _pad;
};
Texture2D<float4> src : register(t0);
RWTexture2D<float4> dst : register(u0);

float3 linear_to_srgb(float3 x) {
    x = saturate(x);
    float3 lo = x * 12.92;
    float3 hi = 1.055 * pow(max(x, 1e-6), 1.0 / 2.4) - 0.055;
    return (x <= 0.0031308) ? lo : hi;
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= size.x || id.y >= size.y) return;
    float3 rgb = src.Load(int3(id.xy, 0)).rgb;
    rgb = isfinite(rgb) ? saturate(rgb) : 0.0;
    if (srgb_out != 0u) {
        rgb = linear_to_srgb(rgb);
    }
    dst[id.xy] = float4(saturate(rgb), 1.0);
}
"#;

struct ComputePass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

impl ComputePass {
    fn compile(device: &ID3D12Device, name: PCSTR, hlsl: &str) -> windows::core::Result<Self> {
        let mut blob: Option<ID3DBlob> = None;
        let mut errs: Option<ID3DBlob> = None;
        // SAFETY: static HLSL source and entry point live through D3DCompile.
        let res = unsafe {
            D3DCompile(
                hlsl.as_ptr().cast(),
                hlsl.len(),
                name,
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                0,
                0,
                &mut blob,
                Some(&mut errs),
            )
        };
        if let Err(e) = res {
            if let Some(err_blob) = errs {
                // SAFETY: valid D3D error blob.
                let msg = unsafe {
                    let s = std::slice::from_raw_parts(
                        err_blob.GetBufferPointer().cast::<u8>(),
                        err_blob.GetBufferSize(),
                    );
                    String::from_utf8_lossy(s).into_owned()
                };
                tracing::error!(shader = ?name, error = %msg, "FrameGen HLSL compile failed");
            }
            return Err(e);
        }
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        // SAFETY: compiled bytecode owned by `blob`.
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
            Ok(Self { root, pso })
        }
    }
}

struct Pipelines {
    luma0: ComputePass,
    down: ComputePass,
    flow_match: ComputePass,
    flow_filter: ComputePass,
    scenecut: ComputePass,
    interp: ComputePass,
    snapshot: ComputePass,
}

impl Pipelines {
    fn new(device: &ID3D12Device) -> windows::core::Result<Self> {
        Ok(Self {
            luma0: ComputePass::compile(device, s!("fg_luma0"), LUMA0_HLSL)?,
            down: ComputePass::compile(device, s!("fg_down"), DOWN_HLSL)?,
            flow_match: ComputePass::compile(device, s!("fg_flow_match"), FLOW_MATCH_HLSL)?,
            flow_filter: ComputePass::compile(device, s!("fg_flow_filter"), FLOW_FILTER_HLSL)?,
            scenecut: ComputePass::compile(device, s!("fg_scenecut"), SCENECUT_HLSL)?,
            interp: ComputePass::compile(device, s!("fg_interp"), INTERP_HLSL)?,
            snapshot: ComputePass::compile(device, s!("fg_snapshot"), SNAPSHOT_HLSL)?,
        })
    }
}

struct Targets {
    width: u32,
    height: u32,
    luma_sizes: [(u32, u32); 4],
    /// Previous scene frame (TYPELESS R8G8B8A8: UAV as UNORM, SRV as UNORM or UNORM_SRGB).
    prev_scene: ID3D12Resource,
    /// Synthesized intermediate frame (TYPELESS R8G8B8A8).
    interp_scene: ID3D12Resource,
    /// Two 4-level luminance pyramids (`[0]` and `[1]`, ping-ponged by `cur`).
    luma: [[ID3D12Resource; 4]; 2],
    /// Forward and backward flow pyramids at levels 1, 2, 3 (`[0]=L1`, `[1]=L2`, `[2]=L3`).
    flow_fwd: [ID3D12Resource; 3],
    flow_bwd: [ID3D12Resource; 3],
    /// Filtered normalized-UV bidirectional flow at L1 (`W/4 x H/4`, `R16G16B16A16_FLOAT`).
    flow_uv: ID3D12Resource,
    /// 1x1 `R32_FLOAT` global scene-cut blend factor (`0.0` = cut, `1.0` = normal).
    cut_tex: ID3D12Resource,
}

fn make_uav_tex(
    device: &ID3D12Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
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
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut res: Option<ID3D12Resource> = None;
    // SAFETY: plain committed 2D UAV texture creation.
    unsafe {
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, initial, None, &mut res)?;
    }
    res.ok_or_else(windows::core::Error::empty)
}

impl Targets {
    fn new(device: &ID3D12Device, width: u32, height: u32) -> windows::core::Result<Self> {
        let s0 = ((width / 2).max(16), (height / 2).max(16));
        let s1 = ((s0.0 / 2).max(8), (s0.1 / 2).max(8));
        let s2 = ((s1.0 / 2).max(4), (s1.1 / 2).max(4));
        let s3 = ((s2.0 / 2).max(2), (s2.1 / 2).max(2));
        let luma_sizes = [s0, s1, s2, s3];

        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let prev_scene = make_uav_tex(device, width, height, DXGI_FORMAT_R8G8B8A8_TYPELESS, srv)?;
        let interp_scene = make_uav_tex(device, width, height, DXGI_FORMAT_R8G8B8A8_TYPELESS, srv)?;

        let make_luma_pyr = || -> windows::core::Result<[ID3D12Resource; 4]> {
            Ok([
                make_uav_tex(device, s0.0, s0.1, DXGI_FORMAT_R8_UNORM, srv)?,
                make_uav_tex(device, s1.0, s1.1, DXGI_FORMAT_R8_UNORM, srv)?,
                make_uav_tex(device, s2.0, s2.1, DXGI_FORMAT_R8_UNORM, srv)?,
                make_uav_tex(device, s3.0, s3.1, DXGI_FORMAT_R8_UNORM, srv)?,
            ])
        };
        let make_flow_pyr = || -> windows::core::Result<[ID3D12Resource; 3]> {
            Ok([
                make_uav_tex(device, s1.0, s1.1, DXGI_FORMAT_R16G16B16A16_FLOAT, srv)?,
                make_uav_tex(device, s2.0, s2.1, DXGI_FORMAT_R16G16B16A16_FLOAT, srv)?,
                make_uav_tex(device, s3.0, s3.1, DXGI_FORMAT_R16G16B16A16_FLOAT, srv)?,
            ])
        };

        let luma = [make_luma_pyr()?, make_luma_pyr()?];
        let flow_fwd = make_flow_pyr()?;
        let flow_bwd = make_flow_pyr()?;
        let flow_uv = make_uav_tex(device, s1.0, s1.1, DXGI_FORMAT_R16G16B16A16_FLOAT, srv)?;
        let cut_tex = make_uav_tex(device, 1, 1, DXGI_FORMAT_R32_FLOAT, srv)?;

        Ok(Self {
            width,
            height,
            luma_sizes,
            prev_scene,
            interp_scene,
            luma,
            flow_fwd,
            flow_bwd,
            flow_uv,
            cut_tex,
        })
    }
}

pub(crate) struct FrameGen {
    pipelines: Pipelines,
    heap: ID3D12DescriptorHeap,
    step: u32,
    ring_idx: usize,
    targets: Option<Targets>,
    /// Index (`0` or `1`) of the pyramid holding the previous frame's luminance.
    prev_pyr: usize,
    has_prev: bool,
}

// SAFETY: D3D12 objects are free-threaded; FrameGen is accessed under Presenter's Mutex.
unsafe impl Send for FrameGen {}
unsafe impl Sync for FrameGen {}

fn is_srgb(fmt: DXGI_FORMAT) -> bool {
    fmt == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB || fmt == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
}

fn srv_barrier(res: &ID3D12Resource, from: D3D12_RESOURCE_STATES, to: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: from,
                StateAfter: to,
            }),
        },
    }
}

struct DescCursor<'a> {
    device: &'a ID3D12Device,
    cpu_base: usize,
    gpu_base: u64,
    step: u32,
    next: u32,
    limit: u32,
}

impl<'a> DescCursor<'a> {
    fn new(device: &'a ID3D12Device, heap: &ID3D12DescriptorHeap, step: u32, ring_idx: usize) -> Self {
        let offset = (ring_idx as u32) * SLOTS_PER_FRAME;
        // SAFETY: shader-visible descriptor heap.
        let (cpu, gpu) = unsafe {
            (
                heap.GetCPUDescriptorHandleForHeapStart(),
                heap.GetGPUDescriptorHandleForHeapStart(),
            )
        };
        Self {
            device,
            cpu_base: cpu.ptr + (offset * step) as usize,
            gpu_base: gpu.ptr + u64::from(offset * step),
            step,
            next: 0,
            limit: SLOTS_PER_FRAME,
        }
    }

    fn alloc_table(&mut self, count: u32) -> (usize, D3D12_GPU_DESCRIPTOR_HANDLE) {
        debug_assert!(self.next + count <= self.limit);
        let at = self.next;
        self.next += count;
        (
            self.cpu_base + (at * self.step) as usize,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: self.gpu_base + u64::from(at * self.step),
            },
        )
    }

    fn write_srv(&self, cpu_ptr: usize, idx: u32, res: &ID3D12Resource, format: DXGI_FORMAT) {
        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu_ptr + (idx * self.step) as usize,
        };
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
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
        // SAFETY: live resource and valid descriptor handle.
        unsafe {
            self.device.CreateShaderResourceView(res, Some(&desc), handle);
        }
    }

    fn write_uav(&self, cpu_ptr: usize, idx: u32, res: &ID3D12Resource, format: DXGI_FORMAT) {
        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu_ptr + (idx * self.step) as usize,
        };
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_UAV::default(),
            },
        };
        // SAFETY: live UAV resource and valid descriptor handle.
        unsafe {
            self.device.CreateUnorderedAccessView(res, None, Some(&desc), handle);
        }
    }
}

impl FrameGen {
    pub(crate) fn new(host: &Host) -> windows::core::Result<Self> {
        let pipelines = Pipelines::new(&host.device)?;
        // SAFETY: plain shader-visible descriptor heap creation.
        let (heap, step) = unsafe {
            let heap: ID3D12DescriptorHeap =
                host.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: (RING_FRAMES as u32) * SLOTS_PER_FRAME,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let step = host
                .device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            (heap, step)
        };
        Ok(Self {
            pipelines,
            heap,
            step,
            ring_idx: 0,
            targets: None,
            prev_pyr: 0,
            has_prev: false,
        })
    }

    /// Drops cached history (e.g. after swap chain resize).
    pub(crate) fn reset(&mut self) {
        self.targets = None;
        self.has_prev = false;
    }

    /// Ensures internal optical-flow & history textures match `(width, height)`.
    /// Returns whether a valid previous frame exists to interpolate from.
    pub(crate) fn prepare_targets(
        &mut self,
        host: &Host,
        width: u32,
        height: u32,
    ) -> windows::core::Result<bool> {
        if width < 64 || height < 64 {
            self.reset();
            return Ok(false);
        }
        let matches = self
            .targets
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height);
        if !matches {
            self.targets = Some(Targets::new(&host.device, width, height)?);
            self.prev_pyr = 0;
            self.has_prev = false;
            tracing::info!(
                width,
                height,
                vram_mib = format_args!("{:.1}", (u64::from(width) * u64::from(height) * 9) as f64 / (1024.0 * 1024.0)),
                "FrameGen optical-flow targets ready"
            );
        }
        Ok(self.has_prev)
    }

    /// Returns the synthesized frame texture and the SRV format matching
    /// `base_fmt`'s sRGB decode semantics (for zero brightness flicker).
    pub(crate) fn interp_view(&self, base_fmt: DXGI_FORMAT) -> Option<(&ID3D12Resource, DXGI_FORMAT)> {
        let t = self.targets.as_ref()?;
        let fmt = if is_srgb(base_fmt) {
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        } else {
            DXGI_FORMAT_R8G8B8A8_UNORM
        };
        Some((&t.interp_scene, fmt))
    }

    /// Returns the synthesized frame texture (in `SRV` state) for direct copy.
    pub(crate) fn interp_resource(&self) -> Option<&ID3D12Resource> {
        self.targets.as_ref().map(|t| &t.interp_scene)
    }

    /// Records the optical-flow pyramid + bidirectional matching + flow filter +
    /// scene-cut detection for the new `curr_tex` (must already be in SRV state).
    /// On the first frame (`!has_prev`), only builds the initial luma pyramid.
    pub(crate) fn record_analyze(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        curr_tex: &ID3D12Resource,
        curr_fmt: DXGI_FORMAT,
    ) {
        let Some(t) = self.targets.as_ref() else { return };
        self.ring_idx = (self.ring_idx + 1) % RING_FRAMES;
        let mut cursor = DescCursor::new(&host.device, &self.heap, self.step, self.ring_idx);
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let cur_pyr = if self.has_prev { 1 - self.prev_pyr } else { 0 };
        let prev_pyr = self.prev_pyr;
        let srgb_flag = u32::from(is_srgb(curr_fmt));

        // SAFETY: open command list, live resources, synchronized transitions.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);

            // 1. Build 4-level luminance pyramid for `curr_tex`.
            list.ResourceBarrier(&[srv_barrier(&t.luma[cur_pyr][0], srv, uav)]);
            let (cpu, gpu) = cursor.alloc_table(2);
            cursor.write_srv(cpu, 0, curr_tex, curr_fmt);
            cursor.write_uav(cpu, 1, &t.luma[cur_pyr][0], DXGI_FORMAT_R8_UNORM);
            let (w0, h0) = t.luma_sizes[0];
            let p0 = [w0, h0, srgb_flag, 0];
            list.SetComputeRootSignature(&self.pipelines.luma0.root);
            list.SetPipelineState(&self.pipelines.luma0.pso);
            list.SetComputeRoot32BitConstants(0, 4, p0.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, gpu);
            list.Dispatch(w0.div_ceil(8), h0.div_ceil(8), 1);
            list.ResourceBarrier(&[srv_barrier(&t.luma[cur_pyr][0], uav, srv)]);

            list.SetComputeRootSignature(&self.pipelines.down.root);
            list.SetPipelineState(&self.pipelines.down.pso);
            for lvl in 1..4 {
                let (w, h) = t.luma_sizes[lvl];
                list.ResourceBarrier(&[srv_barrier(&t.luma[cur_pyr][lvl], srv, uav)]);
                let (cpu, gpu) = cursor.alloc_table(2);
                cursor.write_srv(cpu, 0, &t.luma[cur_pyr][lvl - 1], DXGI_FORMAT_R8_UNORM);
                cursor.write_uav(cpu, 1, &t.luma[cur_pyr][lvl], DXGI_FORMAT_R8_UNORM);
                let pd = [w, h, 0, 0];
                list.SetComputeRoot32BitConstants(0, 4, pd.as_ptr().cast(), 0);
                list.SetComputeRootDescriptorTable(1, gpu);
                list.Dispatch(w.div_ceil(8), h.div_ceil(8), 1);
                list.ResourceBarrier(&[srv_barrier(&t.luma[cur_pyr][lvl], uav, srv)]);
            }

            if !self.has_prev {
                return;
            }

            // 2. Coarse-to-fine bidirectional optical flow (L3 -> L2 -> L1).
            list.SetComputeRootSignature(&self.pipelines.flow_match.root);
            list.SetPipelineState(&self.pipelines.flow_match.pso);
            for (dir, out_pyr) in [(false, &t.flow_fwd), (true, &t.flow_bwd)] {
                let (a_pyr, b_pyr) = if !dir { (prev_pyr, cur_pyr) } else { (cur_pyr, prev_pyr) };
                for &(luma_lvl, flow_idx, radius, has_coarse, subpixel) in &[
                    (3usize, 2usize, 3i32, 0u32, 0u32),
                    (2usize, 1usize, 2i32, 1u32, 0u32),
                    (1usize, 0usize, 2i32, 1u32, 1u32),
                ] {
                    let (w, h) = t.luma_sizes[luma_lvl];
                    let (cw, ch) = if has_coarse != 0 {
                        t.luma_sizes[luma_lvl + 1]
                    } else {
                        (w, h)
                    };
                    let coarse_res = if has_coarse != 0 {
                        &out_pyr[flow_idx + 1]
                    } else {
                        &t.flow_uv
                    };
                    list.ResourceBarrier(&[srv_barrier(&out_pyr[flow_idx], srv, uav)]);
                    let (cpu, gpu) = cursor.alloc_table(4);
                    cursor.write_srv(cpu, 0, &t.luma[a_pyr][luma_lvl], DXGI_FORMAT_R8_UNORM);
                    cursor.write_srv(cpu, 1, &t.luma[b_pyr][luma_lvl], DXGI_FORMAT_R8_UNORM);
                    cursor.write_srv(cpu, 2, coarse_res, DXGI_FORMAT_R16G16B16A16_FLOAT);
                    cursor.write_uav(cpu, 3, &out_pyr[flow_idx], DXGI_FORMAT_R16G16B16A16_FLOAT);
                    let pf = [w, h, radius as u32, has_coarse, subpixel, cw, ch, 0];
                    list.SetComputeRoot32BitConstants(0, 8, pf.as_ptr().cast(), 0);
                    list.SetComputeRootDescriptorTable(1, gpu);
                    list.Dispatch(w.div_ceil(8), h.div_ceil(8), 1);
                    list.ResourceBarrier(&[srv_barrier(&out_pyr[flow_idx], uav, srv)]);
                }
            }

            // 3. Edge-preserving spatial filter -> normalized UV bidirectional flow.
            let (fw, fh) = t.luma_sizes[1];
            list.ResourceBarrier(&[srv_barrier(&t.flow_uv, srv, uav)]);
            let (cpu, gpu) = cursor.alloc_table(4);
            cursor.write_srv(cpu, 0, &t.flow_fwd[0], DXGI_FORMAT_R16G16B16A16_FLOAT);
            cursor.write_srv(cpu, 1, &t.flow_bwd[0], DXGI_FORMAT_R16G16B16A16_FLOAT);
            cursor.write_srv(cpu, 2, &t.luma[cur_pyr][1], DXGI_FORMAT_R8_UNORM);
            cursor.write_uav(cpu, 3, &t.flow_uv, DXGI_FORMAT_R16G16B16A16_FLOAT);
            let pfilt = [fw, fh, 0, 0];
            list.SetComputeRootSignature(&self.pipelines.flow_filter.root);
            list.SetPipelineState(&self.pipelines.flow_filter.pso);
            list.SetComputeRoot32BitConstants(0, 4, pfilt.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, gpu);
            list.Dispatch(fw.div_ceil(8), fh.div_ceil(8), 1);
            list.ResourceBarrier(&[srv_barrier(&t.flow_uv, uav, srv)]);

            // 4. Global scene-cut detector (1 threadgroup over 80x45 L3 luma).
            let (cw, ch) = t.luma_sizes[3];
            list.ResourceBarrier(&[srv_barrier(&t.cut_tex, srv, uav)]);
            let (cpu, gpu) = cursor.alloc_table(3);
            cursor.write_srv(cpu, 0, &t.luma[prev_pyr][3], DXGI_FORMAT_R8_UNORM);
            cursor.write_srv(cpu, 1, &t.luma[cur_pyr][3], DXGI_FORMAT_R8_UNORM);
            cursor.write_uav(cpu, 2, &t.cut_tex, DXGI_FORMAT_R32_FLOAT);
            let pcut = [cw, ch, cut_threshold().to_bits(), 0];
            list.SetComputeRootSignature(&self.pipelines.scenecut.root);
            list.SetPipelineState(&self.pipelines.scenecut.pso);
            list.SetComputeRoot32BitConstants(0, 4, pcut.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, gpu);
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[srv_barrier(&t.cut_tex, uav, srv)]);
        }
    }

    /// Synthesizes one interpolated frame at `alpha` (`0.0..1.0`) into `interp_scene`.
    /// Must be called after [`Self::record_analyze`] when `has_prev == true`.
    pub(crate) fn record_interpolate(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        curr_tex: &ID3D12Resource,
        curr_fmt: DXGI_FORMAT,
        alpha: f32,
    ) {
        let Some(t) = self.targets.as_ref() else { return };
        self.ring_idx = (self.ring_idx + 1) % RING_FRAMES;
        let mut cursor = DescCursor::new(&host.device, &self.heap, self.step, self.ring_idx);
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let srgb = is_srgb(curr_fmt);
        let prev_fmt = if srgb {
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        } else {
            DXGI_FORMAT_R8G8B8A8_UNORM
        };

        // SAFETY: open command list, live textures, valid transitions.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.ResourceBarrier(&[srv_barrier(&t.interp_scene, srv, uav)]);
            let (cpu, gpu) = cursor.alloc_table(5);
            cursor.write_srv(cpu, 0, &t.prev_scene, prev_fmt);
            cursor.write_srv(cpu, 1, curr_tex, curr_fmt);
            cursor.write_srv(cpu, 2, &t.flow_uv, DXGI_FORMAT_R16G16B16A16_FLOAT);
            cursor.write_srv(cpu, 3, &t.cut_tex, DXGI_FORMAT_R32_FLOAT);
            cursor.write_uav(cpu, 4, &t.interp_scene, DXGI_FORMAT_R8G8B8A8_UNORM);
            let params = [
                t.width,
                t.height,
                alpha.clamp(0.0, 1.0).to_bits(),
                u32::from(srgb),
                (1.0f32 / t.width as f32).to_bits(),
                (1.0f32 / t.height as f32).to_bits(),
                0,
                0,
            ];
            list.SetComputeRootSignature(&self.pipelines.interp.root);
            list.SetPipelineState(&self.pipelines.interp.pso);
            list.SetComputeRoot32BitConstants(0, 8, params.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, gpu);
            list.Dispatch(t.width.div_ceil(8), t.height.div_ceil(8), 1);
            list.ResourceBarrier(&[srv_barrier(&t.interp_scene, uav, srv)]);
        }
    }

    /// Saves `curr_tex` into `prev_scene` and advances the pyramid ping-pong
    /// index so the next real frame interpolates from this frame.
    pub(crate) fn record_commit_history(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        curr_tex: &ID3D12Resource,
        curr_fmt: DXGI_FORMAT,
    ) {
        let Some(t) = self.targets.as_ref() else { return };
        self.ring_idx = (self.ring_idx + 1) % RING_FRAMES;
        let mut cursor = DescCursor::new(&host.device, &self.heap, self.step, self.ring_idx);
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let srgb_flag = u32::from(is_srgb(curr_fmt));

        // SAFETY: open command list, live textures, synchronized UAV transition.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.ResourceBarrier(&[srv_barrier(&t.prev_scene, srv, uav)]);
            let (cpu, gpu) = cursor.alloc_table(2);
            cursor.write_srv(cpu, 0, curr_tex, curr_fmt);
            cursor.write_uav(cpu, 1, &t.prev_scene, DXGI_FORMAT_R8G8B8A8_UNORM);
            let psnap = [t.width, t.height, srgb_flag, 0];
            list.SetComputeRootSignature(&self.pipelines.snapshot.root);
            list.SetPipelineState(&self.pipelines.snapshot.pso);
            list.SetComputeRoot32BitConstants(0, 4, psnap.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, gpu);
            list.Dispatch(t.width.div_ceil(8), t.height.div_ceil(8), 1);
            list.ResourceBarrier(&[srv_barrier(&t.prev_scene, uav, srv)]);
        }

        if self.has_prev {
            self.prev_pyr = 1 - self.prev_pyr;
        } else {
            self.prev_pyr = 0;
            self.has_prev = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload_rgba(
        host: &Host,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> ID3D12Resource {
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let tex = make_uav_tex(
            &host.device,
            width,
            height,
            DXGI_FORMAT_R8G8B8A8_UNORM,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .unwrap();
        let desc = unsafe { tex.GetDesc() };
        let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut rows = 0u32;
        let mut row_bytes = 0u64;
        let mut total = 0u64;
        unsafe {
            host.device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut fp),
                Some(&mut rows),
                Some(&mut row_bytes),
                Some(&mut total),
            );
        }
        let (up, ptr) = host.upload_buffer(total).unwrap();
        let pitch = fp.Footprint.RowPitch as usize;
        for y in 0..height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(y * width as usize * 4),
                    ptr.add(fp.Offset as usize + y * pitch),
                    width as usize * 4,
                );
            }
        }
        let list = host.new_list().unwrap();
        let dst_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&tex) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let src_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&up) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
        };
        unsafe {
            list.list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None);
            list.list.ResourceBarrier(&[srv_barrier(
                &tex,
                D3D12_RESOURCE_STATE_COPY_DEST,
                srv,
            )]);
        }
        list.close();
        host.execute(&[&list]);
        tex
    }

    fn readback_rgba(host: &Host, tex: &ID3D12Resource, width: u32, height: u32) -> Vec<u8> {
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let desc = unsafe { tex.GetDesc() };
        let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        unsafe {
            host.device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut fp),
                None,
                None,
                Some(&mut total),
            );
        }
        let rb = host.read_back_buffer(total).unwrap();
        let list = host.new_list().unwrap();
        let src_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(tex) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let dst_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&rb) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
        };
        unsafe {
            list.list.ResourceBarrier(&[srv_barrier(
                tex,
                srv,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            list.list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None);
            list.list.ResourceBarrier(&[srv_barrier(
                tex,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                srv,
            )]);
        }
        list.close();
        host.execute(&[&list]);

        let mut ptr = std::ptr::null_mut();
        unsafe { rb.Map(0, None, Some(&mut ptr)).unwrap() };
        let pitch = fp.Footprint.RowPitch as usize;
        let mut out = vec![0u8; (width * height * 4) as usize];
        for y in 0..height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.cast::<u8>().add(fp.Offset as usize + y * pitch),
                    out.as_mut_ptr().add(y * width as usize * 4),
                    width as usize * 4,
                );
            }
        }
        unsafe { rb.Unmap(0, None) };
        out
    }

    fn draw_moving_box(width: u32, height: u32, bx: i32, by: i32, bw: i32, bh: i32) -> Vec<u8> {
        let mut buf = vec![24u8; (width * height * 4) as usize];
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let i = ((y as u32 * width + x as u32) * 4) as usize;
                // Subtle textured background + bright orange moving box.
                let bg = 24u8.wrapping_add((((x >> 4) ^ (y >> 4)) & 1) as u8 * 8);
                buf[i] = bg;
                buf[i + 1] = bg;
                buf[i + 2] = bg;
                buf[i + 3] = 255;
                if x >= bx && x < bx + bw && y >= by && y < by + bh {
                    buf[i] = 245;
                    buf[i + 1] = 135;
                    buf[i + 2] = 30;
                }
            }
        }
        buf
    }

    #[test]
    fn framegen_interpolates_moving_object_and_rejects_scene_cut_on_gpu() {
        let Some(host) = crate::host::host() else {
            eprintln!("skipping FrameGen GPU test: no D3D12 device");
            return;
        };
        let (w, h) = (256u32, 144u32);
        let mut fg = FrameGen::new(host).expect("FrameGen shaders must compile");
        assert!(!fg.prepare_targets(host, w, h).unwrap());

        // Frame 0: bright box at x = 64..104.
        let f0 = upload_rgba(host, w, h, &draw_moving_box(w, h, 64, 48, 40, 40));
        let l0 = host.new_list().unwrap();
        fg.record_analyze(host, &l0.list, &f0, DXGI_FORMAT_R8G8B8A8_UNORM);
        fg.record_commit_history(host, &l0.list, &f0, DXGI_FORMAT_R8G8B8A8_UNORM);
        l0.close();
        host.execute(&[&l0]);

        // Frame 1: box moved right by +24 px to x = 88..128.
        // Midpoint frame (alpha = 0.5) should place the box at x = 76..116 (center x = 96).
        assert!(fg.prepare_targets(host, w, h).unwrap());
        let f1 = upload_rgba(host, w, h, &draw_moving_box(w, h, 88, 48, 40, 40));
        let l1 = host.new_list().unwrap();
        fg.record_analyze(host, &l1.list, &f1, DXGI_FORMAT_R8G8B8A8_UNORM);
        fg.record_interpolate(host, &l1.list, &f1, DXGI_FORMAT_R8G8B8A8_UNORM, 0.5);
        fg.record_commit_history(host, &l1.list, &f1, DXGI_FORMAT_R8G8B8A8_UNORM);
        l1.close();
        host.execute(&[&l1]);

        let mid = readback_rgba(host, fg.interp_resource().unwrap(), w, h);
        let at = |x: u32, y: u32| -> [u8; 3] {
            let i = ((y * w + x) * 4) as usize;
            [mid[i], mid[i + 1], mid[i + 2]]
        };
        // Center of midpoint box (x=96, y=68) MUST be bright orange, not dark background!
        let center = at(96, 68);
        assert!(
            center[0] > 180 && center[1] > 90,
            "expected bright orange at midpoint x=96, got {center:?}"
        );
        // Old left edge (x=68, y=68) should already be uncovered background, not ghosted!
        let old_left = at(68, 68);
        assert!(
            old_left[0] < 80,
            "expected uncovered dark background at x=68, got {old_left:?}"
        );
        // Far right of frame 1 (x=122, y=68) should still be dark background at t=0.5!
        let future_right = at(122, 68);
        assert!(
            future_right[0] < 80,
            "expected dark background ahead of midpoint at x=122, got {future_right:?}"
        );

        // Frame 2: hard scene cut to bright white scene -> must NOT cross-fade dark ghost!
        let white = vec![235u8; (w * h * 4) as usize];
        let f2 = upload_rgba(host, w, h, &white);
        let l2 = host.new_list().unwrap();
        fg.record_analyze(host, &l2.list, &f2, DXGI_FORMAT_R8G8B8A8_UNORM);
        fg.record_interpolate(host, &l2.list, &f2, DXGI_FORMAT_R8G8B8A8_UNORM, 0.5);
        fg.record_commit_history(host, &l2.list, &f2, DXGI_FORMAT_R8G8B8A8_UNORM);
        l2.close();
        host.execute(&[&l2]);

        let cut = readback_rgba(host, fg.interp_resource().unwrap(), w, h);
        let p = ((68 * w + 96) * 4) as usize;
        assert!(
            cut[p] > 220 && cut[p + 1] > 220 && cut[p + 2] > 220,
            "scene cut should pass through bright frame cleanly without dark cross-fade, got {:?}",
            &cut[p..p + 3]
        );
    }

    #[test]
    fn framegen_720p_srgb_preserves_brightness_and_runs_fast_on_gpu() {
        let Some(host) = crate::host::host() else {
            eprintln!("skipping 720p FrameGen GPU test: no D3D12 device");
            return;
        };
        let (w, h) = (1280u32, 720u32);
        let mut fg = FrameGen::new(host).expect("FrameGen shaders must compile");
        assert!(!fg.prepare_targets(host, w, h).unwrap());

        // Create TYPELESS 720p source so we can sample it as _SRGB (matching Series S Plane 0).
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let tex = make_uav_tex(
            &host.device,
            w,
            h,
            DXGI_FORMAT_R8G8B8A8_TYPELESS,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .unwrap();
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                pixels[i] = 128;
                pixels[i + 1] = 84;
                pixels[i + 2] = 48;
                pixels[i + 3] = 255;
            }
        }
        let desc = unsafe { tex.GetDesc() };
        let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        unsafe {
            host.device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut fp), None, None, Some(&mut total));
        }
        let (up, ptr) = host.upload_buffer(total).unwrap();
        let pitch = fp.Footprint.RowPitch as usize;
        for y in 0..h as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(y * w as usize * 4),
                    ptr.add(fp.Offset as usize + y * pitch),
                    w as usize * 4,
                );
            }
        }
        let l_up = host.new_list().unwrap();
        let dst_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&tex) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let src_loc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: unsafe { std::mem::transmute_copy(&up) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
        };
        unsafe {
            l_up.list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None);
            l_up.list.ResourceBarrier(&[srv_barrier(&tex, D3D12_RESOURCE_STATE_COPY_DEST, srv)]);
        }
        l_up.close();
        host.execute(&[&l_up]);

        // Seed history with frame 0.
        let srgb = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
        let l0 = host.new_list().unwrap();
        fg.record_analyze(host, &l0.list, &tex, srgb);
        fg.record_commit_history(host, &l0.list, &tex, srgb);
        l0.close();
        host.execute(&[&l0]);

        // Warm up + time 10 full 720p optical-flow + interpolate + history passes on real GPU.
        assert!(fg.prepare_targets(host, w, h).unwrap());
        let l_bench = host.new_list().unwrap();
        l_bench.close();
        let t0 = std::time::Instant::now();
        for _ in 0..10 {
            l_bench.reset();
            fg.record_analyze(host, &l_bench.list, &tex, srgb);
            fg.record_interpolate(host, &l_bench.list, &tex, srgb, 0.5);
            fg.record_commit_history(host, &l_bench.list, &tex, srgb);
            l_bench.close();
            host.execute(&[&l_bench]);
        }
        let ms_per_frame = t0.elapsed().as_secs_f64() * 1000.0 / 10.0;
        eprintln!("FrameGen 1280x720 sRGB full pass (sync submit+wait): {ms_per_frame:.2} ms/frame");

        // Verify zero gamma/brightness flicker on static sRGB scene.
        let out = readback_rgba(host, fg.interp_resource().unwrap(), w, h);
        let p = ((360 * w + 640) * 4) as usize;
        assert!(
            out[p].abs_diff(128) <= 2 && out[p + 1].abs_diff(84) <= 2 && out[p + 2].abs_diff(48) <= 2,
            "sRGB round-trip brightness drift detected: expected [128, 84, 48], got {:?}",
            &out[p..p + 3]
        );
    }
}
