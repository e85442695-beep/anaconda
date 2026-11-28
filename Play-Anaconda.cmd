@echo off
setlocal EnableExtensions DisableDelayedExpansion

if not defined VICTORY_GAME_DIR set "VICTORY_GAME_DIR=%~dp0..\GTA6.Xbox.v1.0.2.1"
if not exist "%VICTORY_GAME_DIR%\game_scarlett_master.exe" (
  echo Set VICTORY_GAME_DIR to your own dump folder containing game_scarlett_master.exe. 1>&2
  echo Example: set "VICTORY_GAME_DIR=D:\Games\MyDump" ^&^& Play-Anaconda.cmd 1>&2
  exit /b 2
)

set "runner=%~dp0target\release\victory.exe"
if not exist "%runner%" set "runner=%~dp0victory.exe"
if not exist "%runner%" (
  echo Build first with: cargo build --release --locked -p victory 1>&2
  exit /b 2
)

if not defined VICTORY_CACHE_DIR set "VICTORY_CACHE_DIR=%LOCALAPPDATA%\Anaconda\cache"
if not defined VICTORY_DATA_DIR set "VICTORY_DATA_DIR=%LOCALAPPDATA%\Anaconda\data-s"

rem 1. Strict <= 4 GB VRAM budget (eliminates WDDM PCIe paging & reserves room for world RTs + card buffers)
if not defined VICTORY_DISPLAY_MODE set "VICTORY_DISPLAY_MODE=1280x720"
if not defined VICTORY_VRAM_MB set "VICTORY_VRAM_MB=3050"
if not defined VICTORY_LEVELS_ON_DEMAND set "VICTORY_LEVELS_ON_DEMAND=1"
if not defined VICTORY_LEVELS_MIN_KB set "VICTORY_LEVELS_MIN_KB=128"
if not defined VICTORY_TILES_HEAP_MB set "VICTORY_TILES_HEAP_MB=16"
if not defined VICTORY_POOL_BLOCK_MB set "VICTORY_POOL_BLOCK_MB=16"
if not defined VICTORY_TEXTURE_BUDGET_MB set "VICTORY_TEXTURE_BUDGET_MB=550"
if not defined VICTORY_TEXTURE_MAX_DIM set "VICTORY_TEXTURE_MAX_DIM=256"
if not defined VICTORY_TEXTURE_AWAY_MS set "VICTORY_TEXTURE_AWAY_MS=2500"
if not defined VICTORY_TILES_DROP set "VICTORY_TILES_DROP=0"
if not defined VICTORY_TILES_TAIL_ALWAYS set "VICTORY_TILES_TAIL_ALWAYS=1"
if not defined VICTORY_TEXTURE_DROP set "VICTORY_TEXTURE_DROP=0"
if not defined VICTORY_RIPE_BLOCKS set "VICTORY_RIPE_BLOCKS=1"
if not defined VICTORY_CARD_ROOM_FOR_TARGETS_MB set "VICTORY_CARD_ROOM_FOR_TARGETS_MB=1150"
if not defined VICTORY_CARD_ROOM_FOR_BUFFERS set "VICTORY_CARD_ROOM_FOR_BUFFERS=1"
if not defined VICTORY_CARD_BUFFERS_MIN_KB set "VICTORY_CARD_BUFFERS_MIN_KB=64"
if not defined VICTORY_CARD_BUFFERS_MB set "VICTORY_CARD_BUFFERS_MB=512"
if not defined VICTORY_CARD_WINDOWS_MB set "VICTORY_CARD_WINDOWS_MB=0"

rem 2. Turing/NVIDIA shader & descriptor optimizations (Early-Z, Wave32 tickets, raw-view clamp, safe pipeline library)
if not defined VICTORY_SHADER_OPT set "VICTORY_SHADER_OPT=63"
if not defined VICTORY_EARLY_DEPTH set "VICTORY_EARLY_DEPTH=1"
if not defined VICTORY_THREAD_TICKET set "VICTORY_THREAD_TICKET=1"
if not defined VICTORY_RAW_VIEW_CLAMP set "VICTORY_RAW_VIEW_CLAMP=1"
if not defined VICTORY_LIBRARY_LOCK set "VICTORY_LIBRARY_LOCK=1"
if not defined VICTORY_LIBRARY_MAP set "VICTORY_LIBRARY_MAP=0"
if not defined VICTORY_VSYNC set "VICTORY_VSYNC=0"

rem 3. No hardware Ray Tracing: zero-VRAM No-RT + diffuse GI + specular reflections stand-in + clean indoor fog cap
if not defined VICTORY_RT_NO_ARENAS set "VICTORY_RT_NO_ARENAS=1"
if not defined VICTORY_MEMORY_CARD_MB set "VICTORY_MEMORY_CARD_MB=0"
if not defined VICTORY_AMBIENT set "VICTORY_AMBIENT=0.90"
if not defined VICTORY_AMBIENT_SPECULAR set "VICTORY_AMBIENT_SPECULAR=1"
if not defined VICTORY_AMBIENT_FAST set "VICTORY_AMBIENT_FAST=1"
if not defined VICTORY_AMBIENT_FLOOR set "VICTORY_AMBIENT_FLOOR=0.035"
if not defined VICTORY_AMBIENT_AIR set "VICTORY_AMBIENT_AIR=1.2"

rem 4. Async Stage-2 relay, parallel landing, card steps & 6-core CPU command-list pipeline
if not defined VICTORY_HELPER_PRIORITY set "VICTORY_HELPER_PRIORITY=1"
if not defined VICTORY_THIN set "VICTORY_THIN=1"
if not defined VICTORY_JOIN_PARTS set "VICTORY_JOIN_PARTS=8"
if not defined VICTORY_TEXTURE_NO_WAIT set "VICTORY_TEXTURE_NO_WAIT=1"
if not defined VICTORY_UPLOAD_AHEAD set "VICTORY_UPLOAD_AHEAD=1"
if not defined VICTORY_RECHECK_SOON set "VICTORY_RECHECK_SOON=0"
if not defined VICTORY_RELAY set "VICTORY_RELAY=2"
if not defined VICTORY_RELAY_COPY set "VICTORY_RELAY_COPY=1"
if not defined VICTORY_PLACED_ORDER set "VICTORY_PLACED_ORDER=1"
if not defined VICTORY_LAND_SKIP set "VICTORY_LAND_SKIP=1"
if not defined VICTORY_LAND_THREADS set "VICTORY_LAND_THREADS=4"
if not defined VICTORY_WIDE_BACK set "VICTORY_WIDE_BACK=1"
if not defined VICTORY_WIDE_MERGE set "VICTORY_WIDE_MERGE=1"
if not defined VICTORY_CARD_REHASH_RUNS set "VICTORY_CARD_REHASH_RUNS=1"
if not defined VICTORY_JOIN_COPIES set "VICTORY_JOIN_COPIES=1"
if not defined VICTORY_INDIRECT_WINDOW set "VICTORY_INDIRECT_WINDOW=1"
if not defined VICTORY_CARD_STEPS set "VICTORY_CARD_STEPS=2"
if not defined VICTORY_INDIRECT_BATCH set "VICTORY_INDIRECT_BATCH=2"
if not defined VICTORY_MAP_THREADS set "VICTORY_MAP_THREADS=1"
if not defined VICTORY_CARD_AWAKE set "VICTORY_CARD_AWAKE=1"
if not defined VICTORY_SHOW_AFTER set "VICTORY_SHOW_AFTER=1"

rem 5. Built-in Series S FSR Quality + RCAS GPU compute upscaler + optional AMD FSR 3-style Frame Generation + on-screen FPS HUD (F4 toggle)
if not defined VICTORY_FSR_SCALE set "VICTORY_FSR_SCALE=1.35"
if not defined VICTORY_FSR_SHARPEN set "VICTORY_FSR_SHARPEN=0.55"
if not defined VICTORY_FRAME_GEN set "VICTORY_FRAME_GEN=0"
if not defined VICTORY_FPS_HUD set "VICTORY_FPS_HUD=right"
if not defined VICTORY_INPUT_GLYPHS set "VICTORY_INPUT_GLYPHS=auto"

if not defined VICTORY_LOG if not defined RUST_LOG set "VICTORY_LOG=warn,victory_gpu::performance=info,victory_gpu::ambient=info,victory_gpu::tiles=info"

"%runner%" run "%VICTORY_GAME_DIR%\game_scarlett_master.exe" --console series-s --execute
exit /b %errorlevel%
