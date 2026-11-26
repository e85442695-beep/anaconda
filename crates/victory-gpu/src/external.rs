//! Host work that a host library (or the built-in Series S FidelityFX upscaler)
//! records into a title's command list: the host device, a title texture's host
//! copy in the requested state, the host list itself, and a built-in D3D12
//! compute upscaler (compatible with both normal and `VICTORY_THIN=1` lists).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::object::Object;

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

const UPSCALE_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=8, b0), DescriptorTable(SRV(t0), UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer Params : register(b0) {
    float2 uv_scale;
    uint2 out_size;
    float2 inv_src;
    float sharpness;
    uint _pad;
};
Texture2D<float4> src : register(t0);
RWTexture2D<float4> dst : register(u0);
SamplerState samp : register(s0);

// Fast FSR-inspired 5-tap Catmull-Rom / bicubic-lite + RCAS contrast-adaptive sharpening
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= out_size.x || id.y >= out_size.y) return;
    float2 uv = ((float2(id.xy) + 0.5) / float2(out_size)) * uv_scale;
    float2 min_uv = inv_src * 0.5;
    float2 max_uv = max(uv_scale - inv_src * 0.5, min_uv);
    uv = clamp(uv, min_uv, max_uv);

    float4 c = src.SampleLevel(samp, uv, 0.0);
    if (sharpness > 0.001) {
        float3 n = src.SampleLevel(samp, clamp(uv + float2(0.0, -inv_src.y), min_uv, max_uv), 0.0).rgb;
        float3 s = src.SampleLevel(samp, clamp(uv + float2(0.0,  inv_src.y), min_uv, max_uv), 0.0).rgb;
        float3 w = src.SampleLevel(samp, clamp(uv + float2(-inv_src.x, 0.0), min_uv, max_uv), 0.0).rgb;
        float3 e = src.SampleLevel(samp, clamp(uv + float2( inv_src.x, 0.0), min_uv, max_uv), 0.0).rgb;
        float3 mn = min(c.rgb, min(min(n, s), min(w, e)));
        float3 mx = max(c.rgb, max(max(n, s), max(w, e)));
        float3 avg = (n + s + w + e) * 0.25;
        // RCAS-style clamped unsharp mask (prevents ringing/halos while restoring detail)
        float3 sharp = c.rgb + (c.rgb - avg) * (sharpness * 1.25);
        c.rgb = clamp(sharp, mn, mx);
    }
    dst[id.xy] = c;
}
"#;

const UPSCALE_SLOTS: u32 = 128;

struct BuiltinUpscaler {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}

// SAFETY: D3D12 PSO, root signature and descriptor heap are free-threaded;
// `next` allocates distinct descriptor pairs per dispatch.
unsafe impl Send for BuiltinUpscaler {}
unsafe impl Sync for BuiltinUpscaler {}

impl BuiltinUpscaler {
    fn new(device: &ID3D12Device) -> windows::core::Result<Self> {
        use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
        use windows::Win32::Graphics::Direct3D::ID3DBlob;
        let mut blob: Option<ID3DBlob> = None;
        // SAFETY: static HLSL source and entry names outlive D3DCompile.
        unsafe {
            D3DCompile(
                UPSCALE_HLSL.as_ptr().cast(),
                UPSCALE_HLSL.len(),
                windows::core::s!("ffx_builtin_upscale"),
                None,
                None,
                windows::core::s!("main"),
                windows::core::s!("cs_5_1"),
                0,
                0,
                &mut blob,
                None,
            )?;
        }
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
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
                    NumDescriptors: UPSCALE_SLOTS * 2,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            let step = device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            Ok(Self {
                root,
                pso,
                heap,
                step,
                next: AtomicU32::new(0),
            })
        }
    }
}

fn view_format(fmt: DXGI_FORMAT) -> DXGI_FORMAT {
    match fmt {
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        other => other,
    }
}

/// Built-in zero-dependency D3D12 compute bilinear upscaler for FidelityFX
/// `ffxDispatch(DISPATCH_UPSCALE)`. Works with both immediate recording and
/// `VICTORY_THIN=1` deferred multi-threaded list translation.
pub fn upscale_builtin(
    list: *const Object,
    color: *const Object,
    color_state: u32,
    output: *const Object,
    output_state: u32,
    render: [u32; 2],
    out_size: [u32; 2],
) -> bool {
    if list.is_null() || color.is_null() || output.is_null() {
        return false;
    }
    if crate::list::thin::record_upscale(list, color, color_state, output, output_state, render, out_size) {
        return true;
    }
    upscale_builtin_now(list, color, color_state, output, output_state, render, out_size)
}

pub(crate) fn upscale_builtin_now(
    list: *const Object,
    color: *const Object,
    color_state: u32,
    output: *const Object,
    output_state: u32,
    render: [u32; 2],
    out_size: [u32; 2],
) -> bool {
    let Some(host) = crate::host::host() else {
        return false;
    };
    static UPSCALER: OnceLock<Option<BuiltinUpscaler>> = OnceLock::new();
    let Some(up) = UPSCALER
        .get_or_init(|| match BuiltinUpscaler::new(&host.device) {
            Ok(u) => {
                tracing::warn!("FidelityFX built-in GPU compute upscaler ready");
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
    let Some(color_tex) = crate::resource::host_resource(color) else {
        return false;
    };
    let Some(out_tex) = crate::resource::host_resource(output) else {
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
    let ren_w = if render[0] > 0 { render[0].min(src_w) } else { src_w };
    let ren_h = if render[1] > 0 { render[1].min(src_h) } else { src_h };
    let out_w = if out_size[0] > 0 {
        out_size[0].min(out_desc.Width as u32)
    } else {
        out_desc.Width as u32
    }
    .max(1);
    let out_h = if out_size[1] > 0 {
        out_size[1].min(out_desc.Height)
    } else {
        out_desc.Height
    }
    .max(1);

    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        tracing::warn!(
            src_fmt = src_desc.Format.0,
            out_fmt = out_desc.Format.0,
            src = ?[src_w, src_h],
            render = ?[ren_w, ren_h],
            output = ?[out_w, out_h],
            out_flags = format_args!("{:#x}", out_desc.Flags.0),
            "FidelityFX built-in upscaler executing on GPU"
        );
    });

    const SRV_CS: u32 = 0x40;
    const UAV: u32 = 0x8;
    let src_wanted = if color_state & SRV_CS != 0 { color_state } else { SRV_CS };
    crate::list::ensure_state(host_list, color, src_wanted);
    crate::list::ensure_state(host_list, output, UAV);

    let slot = up.next.fetch_add(1, Ordering::Relaxed) % UPSCALE_SLOTS;
    // SAFETY: descriptor heap and live resources; recording on open list.
    crate::list::with_state(list, |hlist, state| unsafe {
        let mut cpu_srv = up.heap.GetCPUDescriptorHandleForHeapStart();
        cpu_srv.ptr += (slot * 2 * up.step) as usize;
        let cpu_uav = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu_srv.ptr + up.step as usize,
        };
        let mut gpu_base = up.heap.GetGPUDescriptorHandleForHeapStart();
        gpu_base.ptr += u64::from(slot * 2 * up.step);

        let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: view_format(src_desc.Format),
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        let uav_desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: view_format(out_desc.Format),
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_UAV {
                    MipSlice: 0,
                    PlaneSlice: 0,
                },
            },
        };
        host.device.CreateShaderResourceView(color_tex, Some(&srv_desc), cpu_srv);
        host.device.CreateUnorderedAccessView(out_tex, None, Some(&uav_desc), cpu_uav);

        static SHARPEN: OnceLock<f32> = OnceLock::new();
        let sharpness = *SHARPEN.get_or_init(|| {
            std::env::var("VICTORY_FSR_SHARPEN")
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .unwrap_or(0.45)
                .clamp(0.0, 1.0)
        });
        let params: [u32; 8] = [
            (ren_w as f32 / src_w as f32).to_bits(),
            (ren_h as f32 / src_h as f32).to_bits(),
            out_w,
            out_h,
            (1.0f32 / src_w as f32).to_bits(),
            (1.0f32 / src_h as f32).to_bits(),
            sharpness.to_bits(),
            0,
        ];
        let cmd = &hlist.list;
        cmd.SetPipelineState(&up.pso);
        cmd.SetComputeRootSignature(&up.root);
        cmd.SetDescriptorHeaps(&[Some(up.heap.clone())]);
        cmd.SetComputeRoot32BitConstants(0, 8, params.as_ptr().cast(), 0);
        cmd.SetComputeRootDescriptorTable(1, gpu_base);
        cmd.Dispatch(out_w.div_ceil(8), out_h.div_ceil(8), 1);

        let uav_barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::transmute_copy(out_tex),
                }),
            },
        };
        cmd.ResourceBarrier(&[uav_barrier]);
        state.host_state_lost();
    });

    if color_state != src_wanted {
        crate::list::ensure_state(host_list, color, color_state);
    }
    if output_state != UAV {
        crate::list::ensure_state(host_list, output, output_state);
    }
    true
}
