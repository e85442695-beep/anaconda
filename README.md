<p align="center">
  <img src="docs/anaconda-logo.jpg" alt="Anaconda" width="460" />
</p>

<h1 align="center">Anaconda</h1>

> [!CAUTION]
> # ⚠️ ВНИМАНИЕ / WARNING
>
> ## 🇷🇺 Сборка «ВЕНОМ» (Venom) использует код и наработки проекта **Anaconda** и выдаёт себя за автора этого проекта!
> **Официальный и единственный оригинальный репозиторий Anaconda находится здесь:** `https://github.com/e85442695-beep/anaconda`.
> Сборка **«ВЕНОМ» / Venom** не является автором этих изменений (4 GB VRAM / No-RT, встроенный FSR/TAAU, Optical Flow Frame Generation, HUD, Input Glyphs и т.д.) — она просто использует наработки этого репозитория без указания авторства, распространяет неграмотные и неточные описания/правки и выдаёт чужую работу за свою. За актуальными, проверенными сборками и обновлениями следите только в этом репозитории.
>
> ---
>
> ## 🇬🇧 The "VENOM" build uses **Anaconda**'s code and work while presenting itself as the author of the project!
> **The official and only original Anaconda repository is here:** `https://github.com/e85442695-beep/anaconda`.
> The **"VENOM"** build is **NOT** the author of these features (4 GB VRAM / No-RT optimizations, built-in FSR/TAAU upscaler, Optical Flow Frame Generation, HUD, Input Glyphs, etc.) — it uses this repository's work without attribution, publishes careless/inaccurate descriptions and unverified repackages, and claims authorship. Always get verified builds and updates directly from this official repository.

<p align="center">
  <strong>Low-VRAM (4 GB) &amp; No-RT fork of VIctory / cool_fox with built-in Series S FSR + RCAS upscaling and AMD FSR 3-style Optical Flow Frame Generation.</strong>
</p>

<p align="center">
  <a href="https://e85442695-beep.github.io/anaconda/"><strong>Project Website &amp; Live Video</strong></a>
  &nbsp;&middot;&nbsp;
  <a href="https://github.com/e85442695-beep/anaconda/releases/tag/v0.1-demo"><strong>GTX 1650 4 GB Capture (Release v0.1-demo)</strong></a>
</p>

> **This repository contains no game files, code, or assets.** You must supply your own Xbox Series dump at runtime. Not affiliated with or endorsed by any game developer or publisher.

---

## Live Gameplay Capture (GTX 1650 4 GB, No-RT, Native FPS)

[![Watch Anaconda running on GTX 1650 4 GB](https://e85442695-beep.github.io/anaconda/media/readme-video-thumb.jpg)](https://e85442695-beep.github.io/anaconda/#demo)

*Click the preview above to watch the full 1080p capture in your browser on the [Anaconda site](https://e85442695-beep.github.io/anaconda/#demo), or download the raw MP4 directly from [Release v0.1-demo](https://github.com/e85442695-beep/anaconda/releases/download/v0.1-demo/anaconda-gtx1650-demo.mp4).*

- **Test rig:** NVIDIA GeForce GTX 1650 4 GB GDDR5 (Turing TU117, no hardware RT cores) + AMD Ryzen 5 5500 + 16 GB RAM, Windows 11.
- **Measured progress (Frame Generation OFF):** `2.9–3.3 FPS` (`300–345 ms/frame`) → **`4.0–5.1 FPS`** (`196–250 ms/frame`), tiled texture heaps reduced from `832 MiB` → **`480 MiB`** (`-352 MiB` VRAM via 16 MiB best-fit heaps).

---

## Key Features (4 GB VRAM / No-RT Edition)

Built on top of `codec-d/cool_fox` (`pareto-residency`) and `VIctory`, tuned specifically for **4 GB VRAM GPUs without hardware Ray Tracing** (tested on **GTX 1650 4 GB + Ryzen 5 5500 + 16 GB RAM**):

1. **<= 4 GB VRAM Cap (`VICTORY_VRAM_MB`)**
   - Hard-caps the host DXGI local VRAM budget (`VICTORY_VRAM_MB=3050`) across texture residency, staging pools, and buffer mirroring (`crates/victory-gpu/src/host.rs`).
   - Triple-buffered swap chain (`BUFFERS=3`) with configurable VSync (`VICTORY_VSYNC=1`) to prevent screen tearing/flickering.
2. **Built-in Series S FSR + RCAS GPU Compute Upscaler & AMD FSR 3-Style Frame Generation**
   - Implements `amd_fidelityfx_loader_xbox.dll` directly in Rust (`crates/victory-hle/src/modules/fidelityfx.rs`) backed by a zero-dependency D3D12 `cs_5_1` compute shader with 5-tap clamped RCAS sharpening and per-channel NaN/Inf recovery (`crates/victory-gpu/src/external.rs`).
   - Full `VICTORY_THIN=1` multi-threaded command list translation support (`BUILTIN_UPSCALE` op in `crates/victory-gpu/src/list/thin.rs`).
   - Configurable render scale (`VICTORY_FSR_SCALE=2.0` -> `540p` internal -> `720p/1080p` output) and sharpness (`VICTORY_FSR_SHARPEN=0.55`).
   - **Built-in AMD FSR 3-Style Optical Flow Frame Generation (`VICTORY_FRAME_GEN=1` / `2x`, `3x`, `4x`):** zero external DLLs, ~8 MiB VRAM at 720p (`crates/victory-gpu/src/framegen.rs`). Runs a 4-level coarse-to-fine Zero-Mean SAD bidirectional optical flow pyramid, edge-preserving flow filter, GPU scene-cut detector, and disocclusion-aware 3-hypothesis synthesis strictly on the **HUD-less 3D scene plane (`Plane 0`)**, then composites the crisp UI overlay (`Plane 1`) on top with zero HUD ghosting. Window title and performance logs honestly report both native real FPS and generated output FPS.
3. **Works Without Hardware Ray Tracing**
   - Software interior ambient lighting compensation (`VICTORY_AMBIENT=0.8`, `VICTORY_AMBIENT_AIR=1.5`) paired with `rtDynamicShadows=false` / `rtLODLightShadows=false` in the Very Low preset.
4. **Optimized Shader & Texture Streaming**
   - NVIDIA Wave32 lowering + DXIL optimization passes (`VICTORY_SHADER_OPT=63`) + safe NVIDIA pipeline library locking (`VICTORY_LIBRARY_LOCK=1`, `VICTORY_LIBRARY_MAP=0`).
   - On-demand tiled texture residency (`VICTORY_LEVELS_ON_DEMAND=1`), async copy-queue uploads (`VICTORY_UPLOAD_AHEAD=1`, `VICTORY_TEXTURE_NO_WAIT=1`), plus optional placed-texture mip-drop (`VICTORY_TEXTURE_DROP`) with safe `CopyTextureRegion` subresource remapping.
5. **Very Low Graphics Preset**
   - Ready-to-use Series S Very Low / No-RT / FSR Performance config template in [`docs/settings-very-low.xml`](docs/settings-very-low.xml) (`manualMipBias=2.0`, `grassLod=0.2`, `lodScale=0.5`, `waterReflectionSSR=false`, `motionBlur=false`).

---

## Quick Start

### Requirements
- Windows 10/11 x64
- CPU with AVX2
- Direct3D 12 GPU with Shader Model 6.6 & Mesh Shader support (4 GB+ VRAM, no hardware RT required)
- Pinned Rust toolchain (`rust-toolchain.toml`) + MSVC C++ Build Tools + Windows SDK
- Your own Xbox Series game dump

### Build & Run

```powershell
cargo build --release --locked -p victory
```

Point `VICTORY_GAME_DIR` at your dump folder and launch with the 4 GB preset:

```powershell
$env:VICTORY_GAME_DIR = 'D:\Path\To\YourDump'
.\Play-Anaconda.cmd
```

Optionally copy [`docs/settings-very-low.xml`](docs/settings-very-low.xml) into:
`%LOCALAPPDATA%\Anaconda\data-s\7952987A\storage\settings\settings.xml`

---

## Key Environment Switches

| Variable | Default in `Play-Anaconda.cmd` | Purpose |
|---|---|---|
| `VICTORY_DISPLAY_MODE` | `1280x720` | Display mode reported to the title via DXGI |
| `VICTORY_VRAM_MB` | `3050` | Caps DXGI local VRAM budget (MiB) for 4 GB cards |
| `VICTORY_TILES_HEAP_MB` | `16` | Granularity of tiled resource pools (auto 16 MiB + best-fit on `<= 6 GiB` GPUs) |
| `VICTORY_TEXTURE_BUDGET_MB` | `900` | Upper cap on texture VRAM without bypassing DXGI budget or RT/buffer reserves |
| `VICTORY_TEXTURE_MAX_DIM` | `256` | Caps tiled mip uploads & staging buffers at 256 px (~62× smaller staging RAM) |
| `VICTORY_CARD_BUFFERS_MIN_KB` / `_MB` | `64` / `448` | Keeps shader-written title buffers resident in VRAM instead of PCIe Shared RAM |
| `VICTORY_RELAY` | `2` | Stage-2 per-range async relay (avoids full-flight GPU stalls) |
| `VICTORY_FSR_SCALE` | `1.35` | Internal FSR render scale (`1.35`–`2.0`) |
| `VICTORY_FSR_SHARPEN` | `0.55` | RCAS contrast-adaptive sharpening strength (`0.0`–`1.0`) |
| `VICTORY_FRAME_GEN` | `0` (set `1` for `2x`) | AMD FSR 3-style Optical Flow Frame Generation (`0`=off, `1`/`2`=2x, `3`=3x, `4`=4x; HUD-less) |
| `VICTORY_FPS_HUD` | `right` | On-screen FPS / FG / frametime / VRAM overlay (`right`, `left`, `bottom-right`, `bottom-left`, `0`=off; toggle live with `F4`) |
| `VICTORY_VSYNC` | `0` | Swap-chain sync interval (`0`=off, `1`=VSync, `2`=half-rate) |
| `VICTORY_SHADER_OPT` | `63` | Proven DXIL shader optimization passes |
| `VICTORY_AMBIENT` / `VICTORY_AMBIENT_SPECULAR` | `0.90` / `1` | Software diffuse GI + specular reflections stand-in without hardware RT |

---

## Credits

Anaconda builds directly on the shoulders of the VIctory community:

- **[codec-d/cool_fox](https://git.gtahub.st/github/codec-d--cool_fox)** (`pareto-residency`): NVIDIA Wave32 lowering, Series S two-plane UI compositor, thin command lists, and residency work.
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and contributors: core loader, HLE services, D3D12 backend, and shader recompiler.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**, **[DexNom/VIctory](https://github.com/DexNom/VIctory)**, **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**, **[myokex/victory-amd](https://github.com/myokex/victory-amd)**, **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**, **Geckomacabre**, and **cxy0072019-sketch**.

## License

GPL-3.0-or-later, matching VIctory. See [`LICENSE`](LICENSE) and [`docs/asset-policy.md`](docs/asset-policy.md).
