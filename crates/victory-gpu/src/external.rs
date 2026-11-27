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

// Treat invalid components independently so an overflow in one channel cannot
// wipe out the colour in the other two. Keep finite scene values above 1 for
// the game's subsequent exposure and tone mapping passes.
float3 safe_hdr(float3 value, float3 fallback) {
    return isfinite(value) ? clamp(value, 0.0, 65504.0) : fallback;
}

// Bilinear sampling may already have propagated one invalid texel into the
// sample. Reconstruct the affected channels from the finite taps, keeping
// the original weights. This path is only needed on invalid centre samples.
float3 recover_center(float2 uv) {
    int2 last = int2(round(uv_scale / inv_src)) - 1;
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

// Fast FSR-inspired 5-tap RCAS contrast-adaptive sharpening + NaN guard
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= out_size.x || id.y >= out_size.y) return;
    float2 uv = ((float2(id.xy) + 0.5) / float2(out_size)) * uv_scale;
    float2 min_uv = inv_src * 0.5;
    float2 max_uv = max(uv_scale - inv_src * 0.5, min_uv);
    uv = clamp(uv, min_uv, max_uv);

    float4 c = src.SampleLevel(samp, uv, 0.0);
    float3 fallback = 0.0;
    [branch] if (!all(isfinite(c.rgb))) fallback = recover_center(uv);
    c.rgb = safe_hdr(c.rgb, fallback);
    c.a = isfinite(c.a) ? saturate(c.a) : 1.0;
    if (sharpness > 0.001) {
        float3 n = safe_hdr(src.SampleLevel(samp, clamp(uv + float2(0.0, -inv_src.y), min_uv, max_uv), 0.0).rgb, c.rgb);
        float3 s = safe_hdr(src.SampleLevel(samp, clamp(uv + float2(0.0,  inv_src.y), min_uv, max_uv), 0.0).rgb, c.rgb);
        float3 w = safe_hdr(src.SampleLevel(samp, clamp(uv + float2(-inv_src.x, 0.0), min_uv, max_uv), 0.0).rgb, c.rgb);
        float3 e = safe_hdr(src.SampleLevel(samp, clamp(uv + float2( inv_src.x, 0.0), min_uv, max_uv), 0.0).rgb, c.rgb);
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

    fn record(
        &self,
        device: &ID3D12Device,
        cmd: &ID3D12GraphicsCommandList,
        (src, src_format): (&ID3D12Resource, DXGI_FORMAT),
        (out, out_format): (&ID3D12Resource, DXGI_FORMAT),
        params: &[u32; 8],
    ) {
        let slot = self.next.fetch_add(1, Ordering::Relaxed) % UPSCALE_SLOTS;
        // SAFETY: an open list and live textures in their shader access states.
        unsafe {
            let mut cpu_srv = self.heap.GetCPUDescriptorHandleForHeapStart();
            cpu_srv.ptr += (slot * 2 * self.step) as usize;
            let cpu_uav = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu_srv.ptr + self.step as usize,
            };
            let mut gpu_base = self.heap.GetGPUDescriptorHandleForHeapStart();
            gpu_base.ptr += u64::from(slot * 2 * self.step);
            let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: src_format,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            let uav = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: out_format,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_UAV::default(),
                },
            };
            device.CreateShaderResourceView(src, Some(&srv), cpu_srv);
            device.CreateUnorderedAccessView(out, None, Some(&uav), cpu_uav);
            cmd.SetPipelineState(&self.pso);
            cmd.SetComputeRootSignature(&self.root);
            cmd.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            cmd.SetComputeRoot32BitConstants(0, 8, params.as_ptr().cast(), 0);
            cmd.SetComputeRootDescriptorTable(1, gpu_base);
            cmd.Dispatch(params[2].div_ceil(8), params[3].div_ceil(8), 1);
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

    static SHARPEN: OnceLock<f32> = OnceLock::new();
    let sharpness = *SHARPEN.get_or_init(|| {
        std::env::var("VICTORY_FSR_SHARPEN")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(0.45)
            .clamp(0.0, 1.0)
    });
    let params = [
        (ren_w as f32 / src_w as f32).to_bits(),
        (ren_h as f32 / src_h as f32).to_bits(),
        out_w,
        out_h,
        (1.0f32 / src_w as f32).to_bits(),
        (1.0f32 / src_h as f32).to_bits(),
        sharpness.to_bits(),
        0,
    ];
    crate::list::with_state(list, |hlist, state| {
        let cmd = &hlist.list;
        up.record(
            &host.device,
            cmd,
            (color_tex, view_format(src_desc.Format)),
            (out_tex, view_format(out_desc.Format)),
            &params,
        );
        // SAFETY: an open list and a live UAV texture.
        unsafe {
            cmd.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                        pResource: std::mem::transmute_copy(out_tex),
                    }),
                },
            }]);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_upscaler_shader_compiles_on_host_device() {
        let Some(host) = crate::host::host() else {
            return;
        };
        BuiltinUpscaler::new(&host.device).expect("built-in FSR+RCAS shader must compile");
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn builtin_upscaler_preserves_hdr_near_non_finite_texels() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let up = BuiltinUpscaler::new(&host.device).unwrap();
        let format = DXGI_FORMAT_R16G16B16A16_FLOAT;
        let make_texture = |width, flags, state| {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: width,
                Height: 1,
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
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: valid default heap and single-mip texture description.
            unsafe {
                host.device.CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    state,
                    None,
                    &mut texture,
                )
            }
            .unwrap();
            texture.unwrap()
        };
        let src = make_texture(2, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST);
        let out = make_texture(
            4,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
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
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let list = host.new_list().unwrap();
        let cmd = &list.list;
        // SAFETY: the mapped upload has room for both texels; the list is open.
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr().cast::<u8>(), data, size_of_val(&input));
            cmd.CopyTextureRegion(&src_location, 0, 0, 0, &upload_location, None);
            cmd.ResourceBarrier(&[crate::host::transition(
                &src,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            )]);
        }
        let one = 1.0f32.to_bits();
        let half = 0.5f32.to_bits();
        up.record(
            &host.device,
            cmd,
            (&src, format),
            (&out, format),
            &[one, one, 4, 1, half, one, half, 0],
        );
        // SAFETY: the shader dispatch ended on this list and the texture is live.
        unsafe {
            cmd.ResourceBarrier(&[crate::host::transition(
                &out,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
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
}
